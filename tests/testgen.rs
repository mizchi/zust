//! The Rust version of the generator gives the same output as bench/gen.py
#[test]
fn matches_python_generator() {
    let py = std::process::Command::new("python3")
        .args(["bench/gen.py", "50", "3"])
        .output();
    let Ok(py) = py else { return }; // skipped where python is not available
    assert_eq!(
        String::from_utf8(py.stdout).unwrap(),
        zustc::testgen::program(50, 3)
    );
}
