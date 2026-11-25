//! Whether the location and message of compile errors (`line:col: message`) are the same as tests/compile_errors/<name>.err.
//! Each of tests/compile_errors/*.rs has one type error (the expected values were made with the first, syn-based driver)
#[test]
fn compile_errors_match_expected() {
    let dir = std::env::temp_dir().join(format!("cerr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir("tests/compile_errors").unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "rs")).collect();
    files.sort();
    let mut bad = vec![];
    for p in &files {
        let src = std::fs::read_to_string(p).unwrap();
        let want = std::fs::read_to_string(p.with_extension("err")).unwrap();
        let got = zustc::driver2::build(&src, &dir.join("x"), false, None).err().unwrap_or_default();
        if got.trim() != want.trim() {
            bad.push(format!("{}: got {got:?}, want {:?}", p.display(), want.trim()));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    assert!(files.len() >= 10);
}
