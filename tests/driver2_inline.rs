//! Builds tests/probe, examples and tests/acg with driver2's `-O` (inline expansion of small fns + skipping minclif's re-reads) and checks
//! that the output (stdout, stderr, exit code) is the same as without `-O`.
//! (`-O` does not check for overflow, so the ones that panic on overflow (those with overflow in their names) are excluded)
#![cfg(feature = "acg")]
use std::path::PathBuf;

#[path = "common/par.rs"]
mod par;

#[test]
fn inline_keeps_behavior() {
    let dir = std::env::temp_dir().join(format!("drv2inl_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<PathBuf> = vec![];
    for d in ["tests/probe", "examples", "tests/acg"] {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "rs") && !p.to_string_lossy().contains("overflow") {
                files.push(p);
            }
        }
    }
    files.sort();
    let results = par::par_map(&files, |p| -> Option<Result<(), String>> {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(p).unwrap();
        // (a directory of its own for each source directory: two of them have a file of the same name)
        let sub = dir.join(p.parent().unwrap().file_name().unwrap());
        std::fs::create_dir_all(&sub).unwrap();
        let (a, b) = (sub.join(format!("{name}.0")), sub.join(format!("{name}.1")));
        if zustc::driver2::build(&src, &a, false, None).is_err() {
            return None;
        }
        if let Err(e) = zustc::driver2::build(&src, &b, true, None) {
            return Some(Err(format!("{name}: -O build failed: {}", e.lines().next().unwrap_or(""))));
        }
        let run = |e: &PathBuf| std::process::Command::new(e).stdin(std::process::Stdio::null()).output().unwrap();
        let (x, y) = (run(&a), run(&b));
        if x.stdout == y.stdout && x.stderr == y.stderr && x.status.code() == y.status.code() {
            Some(Ok(()))
        } else {
            Some(Err(format!("{name}: differs (exit {:?} vs {:?})", x.status.code(), y.status.code())))
        }
    });
    let ok = results.iter().filter(|r| matches!(r, Some(Ok(())))).count();
    let bad: Vec<String> = results.into_iter().filter_map(|r| r?.err()).collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(ok >= 130, "{ok}");
}
