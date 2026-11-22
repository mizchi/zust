//! zsc: the zustscript prototype's compiler.
//!
//!   zsc <input.zsc> [-o out-dir] [--zustc PATH] [--jsx-runtime MODULE] [--wasm-O] [--all-js] [--no-simd] [--inline-runtime]
//!
//! (`--no-simd` compiles `@simd fn`s through zustc like `@wasm` ones (no vectors).
//! `--all-js` keeps `@wasm fn`s in the JS output: the same source, run as JS.
//! `--wasm-O` passes -O to zustc: faster code, but zustc's inlining makes the module larger, so the default is
//! without it)
//!
//! Writes `<name>.js` (ES module: the JS layer, then the glue of the wasm layer), `<name>.d.ts`, and, when the file
//! has `wasm fn`s, `<name>.wasm.rs` (the Rust handed to zustc) and `<name>.wasm`.
//! The helpers and the brand constructors go to `zsc-runtime.js` / `.d.ts` in the same directory, which the modules
//! import (`--inline-runtime` writes them into the module instead).
//!
//! zustc is used only as an external command: this prototype adds nothing to zust's crates.

mod ast;
mod check;
mod js;
mod lower;
mod simd;
mod parse;
mod wasm;

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input = None;
    let mut out_dir = PathBuf::from(".");
    let mut zustc = std::env::var("ZUSTC").ok().map(PathBuf::from);
    let mut jsx_runtime = "react/jsx-runtime".to_string();
    let mut wasm_opt = false;
    let mut all_js = false;
    let mut no_simd = false;
    let mut inline_runtime = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" => out_dir = PathBuf::from(it.next().expect("-o needs a directory")),
            "--zustc" => zustc = Some(PathBuf::from(it.next().expect("--zustc needs a path"))),
            "--jsx-runtime" => jsx_runtime = it.next().expect("--jsx-runtime needs a module").clone(),
            "--wasm-O" => wasm_opt = true,
            "--all-js" => all_js = true,
            "--no-simd" => no_simd = true,
            "--inline-runtime" => inline_runtime = true,
            _ if input.is_none() => input = Some(PathBuf::from(a)),
            _ => usage(),
        }
    }
    let Some(input) = input else { usage() };
    let src = std::fs::read_to_string(&input).unwrap_or_else(|e| die(&format!("{}: {e}", input.display())));
    let file = input.display().to_string();
    let stem = input.file_stem().unwrap().to_string_lossy().to_string();

    let t0 = std::time::Instant::now();
    let items = match parse::Parser::new(&src).file() {
        Ok(i) => i,
        Err((pos, m)) => {
            report(&file, &src, &[(pos, m)]);
            std::process::exit(1);
        }
    };
    let mut ck = check::Checker::new();
    ck.check_file(&items);
    if !ck.errors.is_empty() {
        let mut errs = ck.errors.clone();
        errs.sort_by_key(|e| (e.0.line, e.0.col));
        errs.dedup();
        report(&file, &src, &errs);
        std::process::exit(1);
    }
    let mut warns = ck.warnings.clone();
    warns.sort_by_key(|e| (e.0.line, e.0.col));
    report_warnings(&file, &src, &warns);
    let info = ck.info;
    let mut j = js::Js::new(&info, &jsx_runtime);
    j.shared_runtime = !inline_runtime;
    if !all_js {
        j.wasm_fns = items.iter().filter_map(|it| match it {
            ast::Item::Fn { name, wasm, simd, .. } if *wasm || *simd => Some(name.clone()),
            _ => None,
        }).collect();
    }
    let body = j.items(&items);
    if !j.errors.is_empty() {
        let mut errs = j.errors.clone();
        errs.sort_by_key(|e| (e.0.line, e.0.col));
        errs.dedup();
        report(&file, &src, &errs);
        std::process::exit(1);
    }
    let header = j.header();
    let (dts, brands) = js::dts(&items, &info, !inline_runtime);
    let wasm_name = format!("{stem}.wasm");
    // `@wasm fn`s: lowered to Rust (with --all-js they stay in the JS output)
    // `@simd fn`s: vectorized kernels (with --no-simd they go through zustc as @wasm ones)
    let is_simd = |n: &str| items.iter().any(|it| matches!(it, ast::Item::Fn { name, simd: true, .. } if name == n));
    let (simd_wanted, wasm_wanted): (Vec<_>, Vec<_>) = j.wasm_wanted.iter().cloned().partition(|(n, _)| is_simd(n) && !no_simd);
    let simd_names: Vec<String> = simd_wanted.iter().map(|(n, _)| n.clone()).collect();
    let simd = match simd::generate(&items, &info, &simd_names) {
        Ok(s) => s,
        Err(mut errs) => {
            errs.sort_by_key(|e| (e.0.line, e.0.col));
            report(&file, &src, &errs);
            std::process::exit(1);
        }
    };
    let (lowered, raw) = match lower::lower(&items, &info, &wasm_wanted, no_simd) {
        Ok(x) => x,
        Err(mut errs) => {
            errs.sort_by_key(|e| (e.0.line, e.0.col));
            errs.dedup();
            report(&file, &src, &errs);
            std::process::exit(1);
        }
    };
    let w = wasm::generate(&items, lowered, &raw, &wasm_name);
    let t_front = t0.elapsed();

    std::fs::create_dir_all(&out_dir).unwrap_or_else(|e| die(&format!("{}: {e}", out_dir.display())));
    let mut out = format!("// generated by zsc from {}\n", input.file_name().unwrap().to_string_lossy());
    out.push_str(&header);
    out.push_str(&body);
    let mut t_wasm = None;
    if let Some(w) = &w {
        out.push_str(&w.glue);
        let rs_path = out_dir.join(format!("{stem}.wasm.rs"));
        std::fs::write(&rs_path, &w.rust).unwrap();
        let zustc = zustc.unwrap_or_else(default_zustc);
        let t1 = std::time::Instant::now();
        let mut cmd = Command::new(&zustc);
        cmd.arg("--target=wasm32-unknown-unknown");
        if wasm_opt {
            cmd.arg("-O");
        }
        let st = cmd
            .arg(&rs_path)
            .arg("-o")
            .arg(out_dir.join(&wasm_name))
            .status()
            .unwrap_or_else(|e| die(&format!("running {}: {e} (set --zustc or ZUSTC)", zustc.display())));
        if !st.success() {
            die(&format!("zustc failed on {} (the wasm layer)", rs_path.display()));
        }
        t_wasm = Some(t1.elapsed());
        let _ = &w.names;
    }
    if let Some(s) = &simd {
        out.push_str(&s.glue);
        std::fs::write(out_dir.join(format!("{stem}.simd.wasm")), &s.wasm).unwrap();
    }
    if inline_runtime {
        out.push_str(&js::brand_runtime(&brands));
    } else {
        out.push_str(&js::brand_reexport(&brands));
        std::fs::write(out_dir.join(format!("{}.js", js::RUNTIME)), js::runtime_js()).unwrap();
        std::fs::write(out_dir.join(format!("{}.d.ts", js::RUNTIME)), js::runtime_dts()).unwrap();
    }
    std::fs::write(out_dir.join(format!("{stem}.js")), out).unwrap();
    std::fs::write(out_dir.join(format!("{stem}.d.ts")), dts).unwrap();
    eprint!("zsc: {} → {}/{stem}.js, .d.ts (front {:.1} ms", file, out_dir.display(), t_front.as_secs_f64() * 1e3);
    if let Some(t) = t_wasm {
        let size = std::fs::metadata(out_dir.join(&wasm_name)).map(|m| m.len()).unwrap_or(0);
        eprint!("; wasm layer via zustc {:.1} ms, {size} bytes", t.as_secs_f64() * 1e3);
    }
    eprintln!(")");
}

/// zustc next to this binary's checkout: crates/zustscript/target/<profile>/zsc → <zust>/target/release/zustc
fn default_zustc() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors() {
            let c = anc.join("target/release/zustc");
            if c.is_file() && anc.join("crates/wasm").is_dir() {
                return c;
            }
        }
    }
    PathBuf::from("zustc")
}

fn report(file: &str, src: &str, errs: &[(ast::Pos, String)]) {
    let lines: Vec<&str> = src.lines().collect();
    for (pos, m) in errs {
        eprintln!("error: {m}\n  --> {file}:{}:{}", pos.line, pos.col + 1);
        if let Some(l) = lines.get(pos.line.saturating_sub(1) as usize) {
            eprintln!("   | {l}\n   | {}^", " ".repeat(pos.col as usize));
        }
    }
    eprintln!("{} error(s)", errs.len());
}

fn report_warnings(file: &str, src: &str, ws: &[(ast::Pos, String)]) {
    let lines: Vec<&str> = src.lines().collect();
    for (pos, m) in ws {
        eprintln!("warning: {m}\n  --> {file}:{}:{}", pos.line, pos.col + 1);
        if let Some(l) = lines.get(pos.line.saturating_sub(1) as usize) {
            eprintln!("   | {l}\n   | {}^", " ".repeat(pos.col as usize));
        }
    }
}

fn usage() -> ! {
    die("usage: zsc <input.zsc> [-o out-dir] [--zustc PATH] [--jsx-runtime MODULE] [--wasm-O] [--all-js] [--no-simd] [--inline-runtime]")
}

fn die(m: &str) -> ! {
    eprintln!("zsc: {m}");
    std::process::exit(2)
}
