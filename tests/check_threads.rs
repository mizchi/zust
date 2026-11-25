//! `check` on worker threads (ZUSTC_PAR=threads, crate::par: the prototype of docs/parallel-frontend.md) gives the
//! same diagnostics as in one process, as the forked children do (tests/check_parallel.rs), and it works in a host
//! that has threads of its own, where forking is not done.

#[path = "check_parallel_program.rs"]
mod program;

#[test]
fn threads_give_the_same_diagnostics() {
    let dir = std::env::temp_dir().join(format!("zust_check_threads_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("p.rs");
    std::fs::write(&src, program::program()).unwrap();
    let run = |par: &str, jobs: &str, min: Option<&str>| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_zustc"));
        c.args(["check", "--edition=2021", "-j", jobs]).arg(&src).env("ZUSTC_PAR", par).env_remove("ZUSTC_CHECK_FORK_MIN").env_remove("ZUSTC_SUMMARY_PAR_MIN");
        if let Some(m) = min {
            c.env("ZUSTC_CHECK_FORK_MIN", m).env("ZUSTC_SUMMARY_PAR_MIN", m);
        }
        let o = c.output().unwrap();
        (o.status.code(), String::from_utf8_lossy(&o.stderr).into_owned())
    };
    let one = run("off", "1", None);
    assert!(one.1.lines().count() >= 4, "{}", one.1);
    assert_eq!(run("threads", "4", None), one);
    // (every wave on threads, one fn at a time: the closures', the placeholders')
    assert_eq!(run("threads", "4", Some("1")), one);
    assert_eq!(run("threads", "3", Some("1")), one);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A host with threads of its own asks for 4 jobs on threads (`Options::par`): the summaries and the check run on worker
/// threads (no fork), and the answer is that of one job
#[test]
fn threaded_host_checks_on_threads() {
    let src = program::program();
    let one = zustc::Session::new(zustc::Options { jobs: 1, no_cache: true, par: Some(zustc::par::Mode::Threads), ..zustc::Options::default() }).check(&src);
    let (f0, s0) = (zustc::session::forks(), zustc::par::spawned());
    let many = zustc::Session::new(zustc::Options { jobs: 4, no_cache: true, par: Some(zustc::par::Mode::Threads), ..zustc::Options::default() }).check(&src);
    let (f1, s1) = (zustc::session::forks(), zustc::par::spawned());
    assert_eq!(f1, f0, "forked");
    assert!(s1 > s0, "no worker thread started");
    assert!(one.len() >= 4, "{one:?}");
    assert_eq!(one, many);
}
