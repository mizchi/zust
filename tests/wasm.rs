//! wasm32-unknown-unknown output (`zustc --target wasm32-unknown-unknown`), run in a sandbox: Node's WebAssembly,
//! with only the imports tests/wasm/run.mjs gives. Needs node (skipped without it) and rustc (the oracle).
//!
//! - `wasm_runs_like_rustc`: examples/*.rs and the runtime cases of tests/security (the std CVE cases among them)
//!   build as modules and print what rustc's builds print, with the same exit codes (a panic is 101 in both).
//! - `wasm_module_shape`: a module imports only functions of `env`, and exports only `memory` and `main`.
//! - `wasm_host_denies`: tests/wasm/deny/*.rs ask for what the host does not give (processes, files, the
//!   environment) and get nothing: a module that imports fork / exec is not run, file access fails, no host file is
//!   written, no variable is visible.
//! - `wasm_contains_faults`: tests/wasm/contain/*.rs do what unsafe code should not (addresses past 4 GiB that would
//!   wrap onto other memory, the null page, past the end of memory, an integer called as a function, a call through a
//!   pointer of another signature, a stack overflow, an allocation larger than memory). Each run must end in a trap
//!   or an abort before printing `after`.
#![cfg(feature = "acg")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct PrivateDir(PathBuf);

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn private_dir(prefix: &str) -> PrivateDir {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::temp_dir();
    for k in 0..100u32 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = base.join(format!("{prefix}_{}_{t}_{k}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return PrivateDir(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("{}: {e}", dir.display()),
        }
    }
    panic!("no new directory under {}", base.display());
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn run_limited(mut cmd: Command, limit: Duration) -> Option<Output> {
    use std::os::unix::process::CommandExt;
    // (its own process group, killed as a whole on a timeout: zustc's forked workers hold the pipes too)
    cmd.process_group(0);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let to = std::thread::spawn(move || {
        let mut b = vec![];
        std::io::Read::read_to_end(&mut so, &mut b).ok();
        b
    });
    let te = std::thread::spawn(move || {
        let mut b = vec![];
        std::io::Read::read_to_end(&mut se, &mut b).ok();
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if start.elapsed() > limit {
            let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = to.join().unwrap();
    let stderr = te.join().unwrap();
    status.map(|status| Output { status, stdout, stderr })
}

const RUNNER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/wasm/run.mjs");

/// Builds src as a wasm module at out; Err(zustc's message)
fn build_wasm(src: &Path, out: &Path) -> Result<(), String> {
    let o = Command::new(env!("CARGO_BIN_EXE_zustc")).args(["--target", "wasm32-unknown-unknown", "--no-cache"]).arg(src).arg("-o").arg(out).output().unwrap();
    if o.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

/// Runs a module in the sandbox (in dir, as its working directory)
fn run_wasm(module: &Path, args: &[&str], dir: &Path) -> Option<Output> {
    let mut cmd = Command::new("node");
    cmd.arg(RUNNER).arg(module).args(args).current_dir(dir);
    run_limited(cmd, Duration::from_secs(60))
}

fn ready() -> bool {
    if !have("node") || !have("rustc") {
        eprintln!("wasm: node or rustc not found, skipped");
        return false;
    }
    true
}

fn files(dir: &str, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs") && p.file_name().unwrap().to_string_lossy().starts_with(prefix))
        .collect();
    v.sort();
    v
}

/// (program, edition flags for rustc, the runs' arguments, the cases where rustc may also panic)
struct Prog {
    path: PathBuf,
    edition: Option<&'static str>,
    args: Vec<String>,
    safe: bool,
}

#[test]
fn wasm_runs_like_rustc() {
    if !ready() {
        return;
    }
    let mut progs: Vec<Prog> = vec![];
    for p in files("examples", "") {
        // (no_std programs have no `main` of the usual kind: they are native only)
        if std::fs::read_to_string(&p).unwrap().contains("#![no_std]") {
            continue;
        }
        progs.push(Prog { path: p, edition: None, args: vec![], safe: false });
    }
    // the runtime cases of the security tests (the CVE cases among them), except the TODOs (a `todo(valgrind)` one is
    // run: its failure is a leak, which nothing here sees)
    for p in files("tests/security", "") {
        let src = std::fs::read_to_string(&p).unwrap();
        let expect = src.lines().find_map(|l| l.strip_prefix("//@ expect:")).unwrap_or("").trim().to_string();
        // (threads need swapcontext, which the sandbox does not give: such a module is not run)
        if !matches!(expect.as_str(), "run" | "panic" | "safe") || src.contains("thread::spawn") {
            continue;
        }
        if src.lines().any(|l| l.starts_with("//@ todo:")) {
            continue;
        }
        let args = src.lines().find_map(|l| l.strip_prefix("//@ args:")).map(|a| a.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
        progs.push(Prog { path: p, edition: Some("2024"), args, safe: expect == "safe" });
    }
    let dir = private_dir("wasm_cmp");
    let next = std::sync::atomic::AtomicUsize::new(0);
    let bad = std::sync::Mutex::new(vec![]);
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(8);
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(p) = progs.get(i) else { break };
                    if let Err(e) = compare(p, &dir.0) {
                        bad.lock().unwrap().push(format!("{}: {e}", p.path.display()));
                    }
                }
            });
        }
    });
    let mut bad = bad.into_inner().unwrap();
    bad.sort();
    assert!(bad.is_empty(), "{} of {} differ from rustc:\n{}", bad.len(), progs.len(), bad.join("\n"));
}

fn compare(p: &Prog, dir: &Path) -> Result<(), String> {
    let name = p.path.file_stem().unwrap().to_string_lossy().into_owned();
    let rbin = dir.join(format!("{name}.rustc"));
    let mut rc = Command::new("rustc");
    rc.arg("-Awarnings");
    if let Some(e) = p.edition {
        rc.args(["--edition", e]);
    }
    let r = rc.arg(&p.path).arg("-o").arg(&rbin).output().unwrap();
    if !r.status.success() {
        return Err(format!("rustc: {}", String::from_utf8_lossy(&r.stderr)));
    }
    let module = dir.join(format!("{name}.wasm"));
    build_wasm(&p.path, &module).map_err(|e| format!("zustc: {e}"))?;
    let runs: Vec<Vec<&str>> = if p.args.is_empty() { vec![vec![]] } else { p.args.iter().map(|a| vec![a.as_str()]).collect() };
    for a in runs {
        let want = run_limited({
            let mut c = Command::new(&rbin);
            c.args(&a).current_dir(dir);
            c
        }, Duration::from_secs(60))
        .ok_or("rustc's build timed out")?;
        let got = run_wasm(&module, &a, dir).ok_or("the module timed out")?;
        let (wc, gc) = (want.status.code(), got.status.code());
        if p.safe {
            let ok = matches!(gc, Some(101)) || (gc == Some(0) && String::from_utf8_lossy(&got.stdout).lines().all(|l| l.starts_with("ok")));
            if !ok {
                return Err(format!("args {a:?}: exit {gc:?}, stdout {:?}", String::from_utf8_lossy(&got.stdout)));
            }
            continue;
        }
        if wc != gc || want.stdout != got.stdout {
            return Err(format!(
                "args {a:?}: rustc exit {wc:?}, wasm exit {gc:?}\n  rustc: {:?}\n  wasm:  {:?}\n  stderr: {}",
                String::from_utf8_lossy(&want.stdout),
                String::from_utf8_lossy(&got.stdout),
                String::from_utf8_lossy(&got.stderr).trim()
            ));
        }
    }
    Ok(())
}

// --- the module's interface ----------------------------------------------------------------------------------

fn uleb(b: &[u8], i: &mut usize) -> u64 {
    let (mut v, mut s) = (0u64, 0);
    loop {
        let x = b[*i];
        *i += 1;
        v |= ((x & 0x7F) as u64) << s;
        s += 7;
        if x & 0x80 == 0 {
            return v;
        }
    }
}

fn name(b: &[u8], i: &mut usize) -> String {
    let n = uleb(b, i) as usize;
    let s = String::from_utf8_lossy(&b[*i..*i + n]).into_owned();
    *i += n;
    s
}

/// (imports as (module, name, kind), exports as (name, kind), has a start function)
fn interface(m: &[u8]) -> (Vec<(String, String, u8)>, Vec<(String, u8)>, bool) {
    assert_eq!(&m[..8], b"\0asm\x01\0\0\0");
    let (mut imports, mut exports, mut start) = (vec![], vec![], false);
    let mut i = 8;
    while i < m.len() {
        let id = m[i];
        i += 1;
        let len = uleb(m, &mut i) as usize;
        let end = i + len;
        let mut j = i;
        match id {
            2 => {
                for _ in 0..uleb(m, &mut j) {
                    let md = name(m, &mut j);
                    let nm = name(m, &mut j);
                    let kind = m[j];
                    j += 1;
                    match kind {
                        0 => {
                            uleb(m, &mut j);
                        }
                        // (a table, memory or global import: not expected; stop reading)
                        _ => {
                            imports.push((md, nm, kind));
                            break;
                        }
                    }
                    imports.push((md, nm, kind));
                }
            }
            7 => {
                for _ in 0..uleb(m, &mut j) {
                    let nm = name(m, &mut j);
                    let kind = m[j];
                    j += 1;
                    uleb(m, &mut j);
                    exports.push((nm, kind));
                }
            }
            8 => start = true,
            _ => {}
        }
        i = end;
    }
    (imports, exports, start)
}

/// What tests/wasm/run.mjs gives (a module may import only these to run)
const HOST: &[&str] = &[
    "write", "putchar", "open", "read", "close", "exit", "_exit", "abort", "sigaction", "fflush", "getenv", "strtod", "strtof", "memmem",
];

#[test]
fn wasm_module_shape() {
    if !ready() {
        return;
    }
    let dir = private_dir("wasm_shape");
    for p in ["examples/hello.rs", "examples/collections.rs", "examples/floats.rs", "examples/dyn_trait.rs"] {
        let module = dir.0.join("m.wasm");
        build_wasm(Path::new(p), &module).unwrap();
        let (imports, exports, start) = interface(&std::fs::read(&module).unwrap());
        for (md, nm, kind) in &imports {
            assert!(md == "env" && *kind == 0, "{p}: imports {md}.{nm} (kind {kind}): only functions of `env`");
            let math = nm.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
            assert!(HOST.contains(&nm.as_str()) || math, "{p}: imports env.{nm}, which the sandbox's host does not give");
        }
        let mut ex: Vec<&str> = exports.iter().map(|e| e.0.as_str()).collect();
        ex.sort();
        assert_eq!(ex, ["main", "memory"], "{p}: exports");
        assert!(!start, "{p}: a start function would run before the host calls main");
    }
}

#[test]
fn wasm_host_denies() {
    if !ready() {
        return;
    }
    let dir = private_dir("wasm_deny");
    let want: &[(&str, Option<i32>, &str)] = &[
        // (a module that imports what the host does not give is not run: 126, and nothing printed)
        ("spawn_process", Some(126), ""),
        ("read_host_file", Some(0), "denied\ndenied\n"),
        ("write_host_file", Some(0), "denied\n"),
        ("env_vars", Some(0), "None None\n"),
    ];
    for (n, code, out) in want {
        let module = dir.0.join(format!("{n}.wasm"));
        build_wasm(&Path::new("tests/wasm/deny").join(format!("{n}.rs")), &module).unwrap();
        let o = run_wasm(&module, &[], &dir.0).expect("timed out");
        assert_eq!((o.status.code(), String::from_utf8_lossy(&o.stdout).as_ref()), (*code, *out), "{n}: stderr {}", String::from_utf8_lossy(&o.stderr));
    }
    assert!(!dir.0.join("zust_wasm_sandbox_probe.txt").exists(), "a host file was written from the sandbox");
    // (the module that spawns asks for exactly the process functions)
    let (imports, _, _) = interface(&std::fs::read(dir.0.join("spawn_process.wasm")).unwrap());
    assert!(imports.iter().any(|i| i.1 == "fork") && imports.iter().any(|i| i.1 == "execvp"));
}

#[test]
fn wasm_contains_faults() {
    if !ready() {
        return;
    }
    let dir = private_dir("wasm_contain");
    let mut bad = vec![];
    let cases = files("tests/wasm/contain", "");
    assert!(cases.len() >= 7);
    for p in &cases {
        let n = p.file_stem().unwrap().to_string_lossy().into_owned();
        let module = dir.0.join(format!("{n}.wasm"));
        if let Err(e) = build_wasm(p, &module) {
            bad.push(format!("{n}: zustc: {e}"));
            continue;
        }
        let o = run_wasm(&module, &[], &dir.0).expect("timed out");
        let out = String::from_utf8_lossy(&o.stdout);
        // a trap (101 at unreachable, 139 out of bounds, 134 other traps / abort), before `after`
        if !matches!(o.status.code(), Some(101 | 134 | 139)) || out.contains("after") {
            bad.push(format!("{n}: exit {:?}, stdout {out:?}, stderr {}", o.status.code(), String::from_utf8_lossy(&o.stderr).trim()));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

// --- SIMD (core::arch::wasm32, issue #30) --------------------------------------------------------------------

const WASI_RUNNER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/wasm/wasi.mjs");

/// Whether rustc can build for wasm32-wasip1 (the oracle of the SIMD tests: core::arch::wasm32 exists only on wasm)
fn have_wasip1() -> bool {
    let o = Command::new("rustc").args(["--print", "target-libdir", "--target", "wasm32-wasip1"]).output();
    o.is_ok_and(|o| o.status.success() && Path::new(String::from_utf8_lossy(&o.stdout).trim()).is_dir())
}

/// tests/wasm/simd/*.rs print what rustc's wasm32-wasip1 builds print. rustc's build is unoptimized: with -O, LLVM
/// folds `f32x4_pmin` / `f32x4_pmax` of constants as if the operands were swapped (NaN and ±0 lanes differ from
/// its own definition and from the wasm spec). Relaxed SIMD (relaxed.rs) is checked on inputs where every allowed
/// result agrees, and by whether a result is one of the allowed ones
#[test]
fn wasm_simd_like_rustc() {
    if !ready() {
        return;
    }
    if !have_wasip1() {
        eprintln!("wasm_simd_like_rustc: rustc has no wasm32-wasip1 target (rustup target add wasm32-wasip1), skipped");
        return;
    }
    let dir = private_dir("wasm_simd");
    let progs = files("tests/wasm/simd", "");
    assert!(progs.len() >= 5);
    let mut bad = vec![];
    for p in &progs {
        let n = p.file_stem().unwrap().to_string_lossy().into_owned();
        let rmod = dir.0.join(format!("{n}.rustc.wasm"));
        let r = Command::new("rustc")
            .args(["-Awarnings", "--edition", "2021", "--target", "wasm32-wasip1", "-C", "target-feature=+simd128,+relaxed-simd"])
            .arg(p)
            .arg("-o")
            .arg(&rmod)
            .output()
            .unwrap();
        if !r.status.success() {
            bad.push(format!("{n}: rustc: {}", String::from_utf8_lossy(&r.stderr)));
            continue;
        }
        let zmod = dir.0.join(format!("{n}.wasm"));
        if let Err(e) = build_wasm(p, &zmod) {
            bad.push(format!("{n}: zustc: {e}"));
            continue;
        }
        let mut c = Command::new("node");
        c.arg(WASI_RUNNER).arg(&rmod).current_dir(&dir.0);
        let want = run_limited(c, Duration::from_secs(60)).expect("rustc's module timed out");
        let got = run_wasm(&zmod, &[], &dir.0).expect("the module timed out");
        if want.status.code() != got.status.code() || want.stdout != got.stdout {
            let (w, g) = (String::from_utf8_lossy(&want.stdout).into_owned(), String::from_utf8_lossy(&got.stdout).into_owned());
            let first = w.lines().zip(g.lines()).find(|(a, b)| a != b).map(|(a, b)| format!("rustc: {a}\n  zustc: {b}")).unwrap_or_default();
            bad.push(format!(
                "{n}: rustc exit {:?}, zustc exit {:?}, first difference:\n  {first}\n  stderr: {}",
                want.status.code(),
                got.status.code(),
                String::from_utf8_lossy(&got.stderr).trim()
            ));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// A program with SIMD builds only as wasm so far: the native backends say so (they do not miscompile it)
#[test]
fn simd_native_is_an_error() {
    let dir = private_dir("simd_native");
    for backend in [None, Some("--backend=minclif")] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_zustc"));
        c.arg("--no-cache");
        if let Some(b) = backend {
            c.arg(b);
        }
        let o = c.arg("tests/wasm/simd/kernels.rs").arg("-o").arg(dir.0.join("k")).output().unwrap();
        let e = String::from_utf8_lossy(&o.stderr);
        assert!(!o.status.success() && e.contains("only with --target wasm32-unknown-unknown"), "{backend:?}: {e}");
    }
}

/// The wasm SIMD module has v128 in function signatures and locals, and imports and exports what any module does
#[test]
fn wasm_simd_module_shape() {
    if !ready() {
        return;
    }
    let dir = private_dir("wasm_simd_shape");
    let module = dir.0.join("m.wasm");
    build_wasm(Path::new("tests/wasm/simd/usage.rs"), &module).unwrap();
    let m = std::fs::read(&module).unwrap();
    let (imports, exports, _) = interface(&m);
    assert!(imports.iter().all(|(md, nm, k)| md == "env" && *k == 0 && HOST.contains(&nm.as_str())), "{imports:?}");
    let mut ex: Vec<&str> = exports.iter().map(|e| e.0.as_str()).collect();
    ex.sort();
    assert_eq!(ex, ["main", "memory"]);
    // (a function type with a v128 parameter: 0x60, n params, .. 0x7B ..)
    assert!(m.windows(3).any(|w| w[0] == 0x60 && w[1] >= 1 && w[2] == 0x7B), "no v128 parameter in the type section");
}
