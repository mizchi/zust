//! `std::thread` as OS threads: each program in tests/threads/ prints what its `.out` file holds (the output of
//! rustc's build), built with both backends. They cover threads that wait for one spawned after them or for main
//! (channel, pingpong), a Mutex another thread holds (mutex), a spin on an atomic flag (flag), many and nested threads
//! (nested), a program with no threads at all (single), many threads at once on every primitive, where a total is
//! exact only when each operation is atomic (parallel), a panic in a spawned thread (panic), thread_local! values of
//! each thread (tls), and what a panic, a disconnect or a lock does at the edges (edges).
#![cfg(feature = "acg")]

#[path = "common/par.rs"]
mod par;

fn build(src: &str, out: &std::path::Path, cl: bool) -> Result<zustc::session::BuildReport, String> {
    let mut o = zustc::session::Options { no_cache: true, jobs: 1, ..Default::default() };
    if cl {
        o.backend = Some(zustc::clif_backend::compile);
    }
    zustc::session::Session::new(o).build(src, out)
}

#[test]
fn threads_like_rustc() {
    let dir = std::env::temp_dir().join(format!("threads_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let names = ["channel", "pingpong", "producers", "mutex", "flag", "nested", "single", "parallel", "panic", "tls", "edges"];
    let cases: Vec<(&str, bool)> = names.iter().flat_map(|&n| [(n, false), (n, true)]).collect();
    let bad: Vec<String> = par::par_map(&cases, |&(name, cl)| -> Option<String> {
        let src = std::fs::read_to_string(format!("tests/threads/{name}.rs")).unwrap();
        let want = std::fs::read_to_string(format!("tests/threads/{name}.out")).unwrap();
        let be = if cl { "cranelift" } else { "minclif" };
        let exe = dir.join(format!("{name}_{be}"));
        if let Err(e) = build(&src, &exe, cl) {
            return Some(format!("{name} ({be}): build: {e}"));
        }
        // (a race shows only sometimes: the parallel one runs a few times)
        for _ in 0..if name == "parallel" { 5 } else { 1 } {
            let o = std::process::Command::new(&exe).env_remove("RUST_BACKTRACE").output().unwrap();
            let got = String::from_utf8_lossy(&o.stdout);
            if !o.status.success() || got != want {
                return Some(format!("{name} ({be}): exit {:?}\n--- got\n{got}--- want\n{want}", o.status.code()));
            }
            if name == "panic" {
                // (each spawned thread's panic, under its name; the note once)
                let err = String::from_utf8_lossy(&o.stderr);
                if err.matches("thread '<unnamed>' panicked at src/main.rs:").count() != 3 || err.matches("note: run with").count() != 1 || err.contains("'main'") {
                    return Some(format!("{name} ({be}): stderr\n{err}"));
                }
            }
        }
        None
    })
    .into_iter()
    .flatten()
    .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// What std's threads wait for forever (rustc's build of each hangs): a Mutex locked again by its holder, a park
/// nothing can unpark, a call_once inside its own closure, a Condvar wait nothing can notify, and a notified waiter
/// whose Mutex the notifier keeps while it parks. zustc's wait forever too: each is still running after 2 s, and
/// what it printed is the start of what rustc's printed before it hung (the `.out` file; stdout is not a terminal, so
/// what is still in its buffer is lost to the kill). Never a second guard or a wrong answer
#[test]
fn deadlocks_wait_forever_like_std() {
    let dir = std::env::temp_dir().join(format!("thread_deadlocks_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let names = ["deadlock_relock", "deadlock_park", "deadlock_once", "deadlock_condvar", "deadlock_condvar_relock"];
    let bad: Vec<String> = par::par_map(&names, |&name| -> Option<String> {
        let src = std::fs::read_to_string(format!("tests/threads/{name}.rs")).unwrap();
        let want = std::fs::read_to_string(format!("tests/threads/{name}.out")).unwrap();
        let exe = dir.join(name);
        if let Err(e) = build(&src, &exe, false) {
            return Some(format!("{name}: build: {e}"));
        }
        let mut child = std::process::Command::new(&exe).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        let start = std::time::Instant::now();
        let mut ended = None;
        while start.elapsed() < std::time::Duration::from_secs(2) {
            if let Some(s) = child.try_wait().unwrap() {
                ended = Some(s);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let o = child.wait_with_output().unwrap();
        let (got, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        if ended.is_some() || !want.starts_with(got.as_ref()) {
            return Some(format!("{name}: ended {ended:?}\n--- got\n{got}--- want (a start of it)\n{want}--- stderr\n{err}"));
        }
        None
    })
    .into_iter()
    .flatten()
    .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
