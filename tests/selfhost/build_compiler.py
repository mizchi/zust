"""Bundles the whole compiler that uses neither syn nor Cranelift (session + acg + minclif and the modules they use) into a single-file program.
usage: python3 tests/selfhost/build_compiler.py > out/selfhost/compiler_main.rs
main calls session::build with `compiler <in.rs> -o <out> [-O]` (same as zustc --backend=minclif).
Compiling this with zustc gives zustc compiled by zustc itself (its path that does not use syn).
proc_macro2 is replaced with tests/selfhost/mini_pm2.rs (macros.rs uses it)."""
import re, glob, os, re

# zustc does not expand macro_rules! in item position, so enc_struct! is expanded here
def expand_enc_struct(src):
    def ex(m):
        t, fs = m.group(1), [f.strip() for f in m.group(2).split(",") if f.strip()]
        e = "".join(f"        if !crate::enc::Enc::is_empty_val(&self.{f}) {{ mask |= bit; }} bit <<= 1;\n" for f in fs)
        w = "".join(f"        if mask & bit != 0 {{ crate::enc::Enc::enc(&self.{f}, out); }} bit <<= 1;\n" for f in fs)
        d = "".join(f"        if mask & bit != 0 {{ v.{f} = crate::enc::Enc::dec(r)?; }} bit <<= 1;\n" for f in fs)
        return (f"impl crate::enc::Enc for {t} {{\n    fn enc(&self, out: &mut Vec<u8>) {{\n        let mut mask: u64 = 0;\n        let mut bit = 1u64;\n{e}        let _ = bit;\n"
                f"        crate::enc::put_varint(out, mask);\n        let mut bit = 1u64;\n{w}        let _ = bit;\n    }}\n"
                f"    fn dec(r: &mut crate::enc::Rd) -> Option<Self> {{\n        let mask = r.varint()?;\n        let mut v = {t}::default();\n        let mut bit = 1u64;\n{d}        let _ = bit;\n        Some(v)\n    }}\n}}\n")
    return re.sub(r"crate::enc_struct!\((\w+) \{([^}]*)\}\);", ex, src)

def body(p):
    return open(p).read()

items = max(glob.glob("target/release/build/zust-syntax*/**/prelude_items.rs", recursive=True), key=os.path.getmtime)
prelude = (body("crates/syntax/src/prelude.rs")
    .replace('include_str!("prelude_lib.rs.in")', 'include_str!("../../crates/syntax/src/prelude_lib.rs.in")')
    .replace('include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));', open(items).read()))

# Replace `pub mod clif;` / `mod p1_expr;` in acg/mod.rs with their contents
acg = body("crates/typeck/src/acg/mod.rs")
def inline(m):
    vis, name = m.group(1) or "", m.group(2)
    return f"{vis}mod {name} {{\n" + body(f"crates/typeck/src/acg/{name}.rs") + "\n}\n"
acg = re.sub(r"^(pub )?mod (\w+);\n", inline, acg, flags=re.M)
# (`#![allow(..)]` can only be written at the start of a module: put inside, it would become an inner attribute, so remove it)
acg = re.sub(r"^#!\[allow\([^)]*\)\]\n", "", acg, flags=re.M)

ir = body("crates/ir/src/lib.rs")
# session: put the build part (session/build.rs) inside and remove the feature `build` conditions (the self-host builds too)
session = body("crates/core/src/session.rs").replace("mod build;\n", "mod build {\n" + body("crates/core/src/session/build.rs") + "\n}\n")
# (put session/dead.rs inside too. `//!` cannot be written inside, so it is removed)
session = session.replace("mod dead;\n", "mod dead {\n" + re.sub(r"^//!.*\n", "", body("crates/core/src/session/dead.rs"), flags=re.M) + "\n}\n")
session = re.sub(r'^[ \t]*#\[cfg\(not\(feature = "build"\)\)\]\n.*\n', "", session, flags=re.M)
session = re.sub(r'^[ \t]*#\[cfg\(feature = "build"\)\]\n', "", session, flags=re.M)
session = session.replace('#[cfg(feature = "build")] ', "")

parts = [("key", body("crates/base/src/key.rs")), ("fxhash", body("crates/base/src/fxhash.rs")), ("lexer", body("crates/syntax/src/lexer.rs")),
         ("prelude", prelude), ("types", body("crates/syntax/src/types.rs")), ("parse", body("crates/syntax/src/parse.rs")),
         ("ast_types", body("crates/syntax/src/ast_types.rs")), ("ast_closure", body("crates/syntax/src/ast_closure.rs")),
         ("ast_summary", body("crates/syntax/src/ast_summary.rs")), ("regions", body("crates/syntax/src/regions.rs")), ("wf", body("crates/syntax/src/wf.rs")), ("program", body("crates/sema/src/program.rs")),
         ("ast_consteval", body("crates/sema/src/ast_consteval.rs")), ("solver", body("crates/sema/src/solver.rs")), ("traits", body("crates/sema/src/traits.rs")), ("ctors", body("crates/syntax/src/ctors.rs")),
         ("ast_infer", body("crates/sema/src/ast_infer.rs")), ("simd_sig", body("crates/sema/src/simd_sig.rs")), ("ir", ir), ("minclif", body("crates/minclif/src/lib.rs").replace("zust_ir::", "crate::ir::")), ("lower", body("crates/codegen/src/lower.rs")), ("link", body("crates/codegen/src/link.rs")),
         ("macros", body("crates/syntax/src/macros.rs") + "\nuse crate::proc_macro2;\n"), ("acg", acg), ("enc", body("crates/syntax/src/enc.rs")), ("types_enc", expand_enc_struct(body("crates/syntax/src/types_enc.rs"))), ("build_cache", body("crates/typeck/src/build_cache.rs")), ("cfg", body("crates/syntax/src/cfg.rs")), ("stub", body("crates/syntax/src/stub.rs")), ("srcmap", body("crates/syntax/src/srcmap.rs")), ("prep", body("crates/syntax/src/prep.rs")), ("prof", body("crates/base/src/prof.rs")), ("par", body("tests/selfhost/par_seq.rs")), ("pm", body("tests/selfhost/pm_stub.rs")), ("test_harness", body("crates/syntax/src/test_harness.rs")), ("front", body("crates/syntax/src/front.rs")), ("session", session)]
out = ["// 自動生成: tests/selfhost/build_compiler.py (zustc の src/*.rs をそのまま含む)",
       "mod cache {\n    pub use crate::key::*;\n}\n", body("tests/selfhost/mini_pm2.rs")]
for name, src in parts:
    out.append(f"mod {name} {{\n" + src + "\n}\n")
out.append(r"""
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut opt = false;
    let mut jobs: usize = 0;
    let mut no_cache = false;
    let mut time = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                output = args.get(i).cloned();
            }
            "-O" => opt = true,
            "-j" => {
                i += 1;
                jobs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(1);
            }
            "--no-cache" => no_cache = true,
            "--time" => time = true,
            a => input = Some(a.to_string()),
        }
        i += 1;
    }
    let Some(input) = input else {
        eprintln!("usage: compiler <in.rs> -o <out> [-O]");
        std::process::exit(2);
    };
    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {input}: {e}");
            std::process::exit(1);
        }
    };
    // (`include_str!` / `include!` はここで展開する: main.rs と同じ)
    let dir = std::path::Path::new(&input).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let src = match macros::expand_includes(&src, dir.as_path()) {
        Ok(Some(s)) => s,
        Ok(None) => src,
        Err(e) => {
            eprintln!("error: {input}: {e}");
            std::process::exit(1);
        }
    };
    let out = output.unwrap_or_else(|| input.trim_end_matches(".rs").to_string());
    let mut o = session::Options::default();
    o.opt = opt;
    o.no_cache = no_cache;
    o.time = time;
    if jobs > 0 {
        o.jobs = jobs;
    }
    let mut s = session::Session::new(o);
    let cp = format!("{out}.cache");
    s.load_cache(std::path::Path::new(&cp));
    if let Err(e) = s.build(&src, std::path::Path::new(&out)) {
        eprintln!("error: {input}: {e}");
        std::process::exit(1);
    }
    if let Err(e) = s.save_cache(std::path::Path::new(&cp)) {
        eprintln!("warning: failed to write cache: {e}");
    }
}
""")
print("\n".join(out))
