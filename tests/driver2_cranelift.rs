//! Builds tests/probe, examples and tests/acg with driver2's Cranelift backend (compiles the instruction sequence recorded by minclif with Cranelift)
//! and checks whether the output (stdout, stderr, exit code) is the same as the one built with minclif (without -O and with -O).
//! (-O does no overflow checks, so the ones with overflow in their names are not compared with -O)
#![cfg(feature = "acg")]
use std::path::PathBuf;

#[path = "common/par.rs"]
mod par;

fn build(src: &str, out: &PathBuf, opt: bool, cl: bool) -> Result<zustc::driver2::Report2, String> {
    let mut o = zustc::driver2::Options2 { opt, no_cache: true, jobs: 1, ..Default::default() };
    if cl {
        o.backend = Some(zustc::clif_backend::compile);
    }
    zustc::driver2::Session2::new(o).build(src, out)
}

#[test]
fn cranelift_backend_matches_minclif() {
    let dir = std::env::temp_dir().join(format!("drv2cl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<PathBuf> = vec![];
    for d in ["tests/probe", "examples", "tests/acg"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    files.sort();
    // (one case per file and -O setting, on every core: a case builds with both backends and runs the two)
    let cases: Vec<(&PathBuf, bool)> = files.iter().flat_map(|p| [(p, false), (p, true)]).collect();
    let results = par::par_map(&cases, |&(p, opt)| -> Option<Result<(), String>> {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        if opt && name.contains("overflow") {
            return None;
        }
        let src = std::fs::read_to_string(p).unwrap();
        // (a directory of its own for each source directory: two of them have a file of the same name)
        let sub = dir.join(p.parent().unwrap().file_name().unwrap());
        std::fs::create_dir_all(&sub).unwrap();
        let (a, b) = (sub.join(format!("{name}.{opt}.m")), sub.join(format!("{name}.{opt}.c")));
        if build(&src, &a, opt, false).is_err() {
            return None;
        }
        if let Err(e) = build(&src, &b, opt, true) {
            return Some(Err(format!("{name} (opt {opt}): cranelift build failed: {}", e.lines().next().unwrap_or(""))));
        }
        let run = |e: &PathBuf| std::process::Command::new(e).stdin(std::process::Stdio::null()).output().unwrap();
        let (x, y) = (run(&a), run(&b));
        if x.stdout == y.stdout && x.stderr == y.stderr && x.status.code() == y.status.code() {
            Some(Ok(()))
        } else {
            Some(Err(format!("{name} (opt {opt}): differs (exit {:?} vs {:?})", x.status.code(), y.status.code())))
        }
    });
    let ok = results.iter().filter(|r| matches!(r, Some(Ok(())))).count();
    let bad: Vec<String> = results.into_iter().filter_map(|r| r?.err()).collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(ok >= 260, "{ok}");
}
