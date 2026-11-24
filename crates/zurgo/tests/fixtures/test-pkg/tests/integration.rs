#[test]
fn uses_lib() { assert_eq!(tp::add(40, 2), 42); }

#[test]
fn runs_bin() {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_tp")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "hello world\n");
}

#[test]
fn fails_on_purpose() { assert_eq!(tp::add(1, 1), 3, "one and one"); }
