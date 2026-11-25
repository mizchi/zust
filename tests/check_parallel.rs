//! `check` in forked children (#101) gives the same diagnostics as in one process.
//!
//! zustc runs as its own process: check_many forks only in a process of one thread, which a test harness is not.
#[path = "check_parallel_program.rs"]
mod program;
use program::program;

#[test]
fn forked_check_gives_the_same_diagnostics() {
    let dir = std::env::temp_dir().join(format!("zust_check_parallel_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("p.rs");
    std::fs::write(&src, program()).unwrap();
    let run = |jobs: &str, fork_min: Option<&str>| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_zustc"));
        c.args(["check", "--edition=2021", "-j", jobs]).arg(&src).env_remove("ZUSTC_CHECK_FORK_MIN");
        if let Some(m) = fork_min {
            c.env("ZUSTC_CHECK_FORK_MIN", m);
        }
        let o = c.output().unwrap();
        (o.status.code(), String::from_utf8_lossy(&o.stderr).into_owned())
    };
    let one = run("1", None);
    assert!(one.1.lines().count() >= 4, "{}", one.1);
    // (the default threshold: the first wave's 300 fns go to children)
    assert_eq!(run("4", None), one);
    // (a fork for every wave: the closures', the placeholders')
    assert_eq!(run("4", Some("1")), one);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A host with threads of its own (a test harness is one) that asks Session2 for 4 jobs: neither the item summaries
/// nor the check fork there (a lock another thread holds would stay held in a child), and the answer is the same
#[test]
fn threaded_host_checks_in_process() {
    let src = program();
    let one = zustc::Session2::new(zustc::Options2 { jobs: 1, no_cache: true, ..zustc::Options2::default() }).check(&src);
    // (a thread that lives through the check: it waits until the check is done)
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let (up, started) = std::sync::mpsc::channel::<()>();
    let other = std::thread::spawn(move || {
        up.send(()).unwrap();
        let _ = wait.recv();
    });
    started.recv().unwrap();
    let f0 = zustc::driver2::forks();
    let many = zustc::Session2::new(zustc::Options2 { jobs: 4, no_cache: true, ..zustc::Options2::default() }).check(&src);
    let f1 = zustc::driver2::forks();
    done.send(()).unwrap();
    other.join().unwrap();
    assert_eq!(f1, f0, "a threaded host forked");
    assert!(one.len() >= 4, "{one:?}");
    assert_eq!(one, many);
}
