//! `std::thread` as green threads: each program in tests/green_threads/ prints what its `.out` file holds (the output
//! of rustc's build). They cover what running a spawned closure at once got wrong: a thread that waits for one
//! spawned after it or for main (channel, pingpong), a Mutex another thread holds (mutex), a spin on an atomic flag
//! (flag), many and nested threads (nested), and a program with no threads at all (single).
#![cfg(feature = "acg")]

#[test]
fn green_threads_like_rustc() {
    let dir = std::env::temp_dir().join(format!("green_threads_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut bad = vec![];
    for name in ["channel", "pingpong", "producers", "mutex", "flag", "nested", "single"] {
        let src = std::fs::read_to_string(format!("tests/green_threads/{name}.rs")).unwrap();
        let want = std::fs::read_to_string(format!("tests/green_threads/{name}.out")).unwrap();
        let exe = dir.join(name);
        if let Err(e) = zustc::session::build(&src, &exe, false, None) {
            bad.push(format!("{name}: build: {e}"));
            continue;
        }
        let o = std::process::Command::new(&exe).output().unwrap();
        let got = String::from_utf8_lossy(&o.stdout);
        if !o.status.success() || got != want {
            bad.push(format!("{name}: exit {:?}\n--- got\n{got}--- want\n{want}", o.status.code()));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
