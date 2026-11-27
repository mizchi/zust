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

/// What std's threads would wait for forever (rustc's build of each hangs): a Mutex locked again by its holder, a park
/// nothing can unpark, a call_once inside its own closure, a Condvar wait nothing can notify (also a wait_timeout whose
/// deadline is past what an Instant holds), and a notified waiter whose Mutex the notifier keeps while it parks. Each panics with "deadlock: every thread is waiting" instead (exit
/// 101), after printing what rustc's printed before it hung (the `.out` file); never a second guard or a wrong answer
#[test]
fn deadlocks_panic_where_std_waits_forever() {
    let dir = std::env::temp_dir().join(format!("green_deadlocks_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut bad = vec![];
    for name in ["deadlock_relock", "deadlock_park", "deadlock_once", "deadlock_condvar", "deadlock_condvar_max", "deadlock_condvar_relock"] {
        let src = std::fs::read_to_string(format!("tests/green_threads/{name}.rs")).unwrap();
        let want = std::fs::read_to_string(format!("tests/green_threads/{name}.out")).unwrap();
        let exe = dir.join(name);
        if let Err(e) = zustc::session::build(&src, &exe, false, None) {
            bad.push(format!("{name}: build: {e}"));
            continue;
        }
        let mut child = std::process::Command::new(&exe)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // (a hang is the failure this guards against: killed after 20 s)
        let start = std::time::Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > std::time::Duration::from_secs(20) {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let o = child.wait_with_output().unwrap();
        let (got, err) = (String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        if o.status.code() != Some(101) || got != want || !err.contains("deadlock: every thread is waiting") {
            bad.push(format!("{name}: exit {:?}\n--- got\n{got}--- want\n{want}--- stderr\n{err}", o.status.code()));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
