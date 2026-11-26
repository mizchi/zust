//! The inputs kept in tests/fuzz/ (findings of zust-fuzz, minimized, after their fix) must run through every
//! target without a panic or a mismatch. A file's first line may name its edition: `// edition: 2015`.

#[test]
fn kept_findings_do_not_fail_again() {
    zust_fuzz::install_panic_hook();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fuzz");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "rs")).collect();
    files.sort();
    assert!(!files.is_empty(), "no inputs in {}", dir.display());
    let mut bad = vec![];
    for p in &files {
        let src = std::fs::read_to_string(p).unwrap();
        let edition = src.lines().next().and_then(|l| l.strip_prefix("// edition: ")).and_then(|e| e.trim().parse().ok()).unwrap_or(2021);
        for t in zust_fuzz::targets::ALL {
            if let Err(f) = zust_fuzz::run_one(t, &src, edition) {
                bad.push(format!("{}: {} at {}: {}", p.display(), f.kind, f.place, f.detail));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
