//! `std::thread` as OS threads: each program in tests/threads/ prints what its `.out` file holds (the output of
//! rustc's build), built with both backends. They cover threads that wait for one spawned after them or for main
//! (channel, pingpong), a Mutex another thread holds (mutex), a spin on an atomic flag (flag), many and nested threads
//! (nested), a program with no threads at all (single), many threads at once on every primitive, where a total is
//! exact only when each operation is atomic (parallel), a panic in a spawned thread (panic), thread_local! values of
//! each thread (tls), what a panic, a disconnect or a lock does at the edges (edges), and `thread::Builder`,
//! `recv_timeout`, a `Barrier` and a closure that is Sync through what it borrows (builder), and std's lock results,
//! `try_lock` / `try_read` / `try_write`, a static `AtomicPtr` and `fmt::Error` as an error (locks).
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

/// Runs exe to its end, or kills it after `secs` (a synchronization bug that blocks must fail the test, not hang it):
/// Err holds what it printed before the kill
fn run_with_deadline(exe: &std::path::Path, secs: u64) -> Result<std::process::Output, String> {
    let mut child = std::process::Command::new(exe)
        .env_remove("RUST_BACKTRACE")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // (the pipes are read meanwhile, so a child that prints much never blocks on a full one)
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let ro = std::thread::spawn(move || {
        let mut v = vec![];
        let _ = std::io::Read::read_to_end(&mut out, &mut v);
        v
    });
    let re = std::thread::spawn(move || {
        let mut v = vec![];
        let _ = std::io::Read::read_to_end(&mut err, &mut v);
        v
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if start.elapsed() > std::time::Duration::from_secs(secs) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let (stdout, stderr) = (ro.join().unwrap(), re.join().unwrap());
    match status {
        Some(status) => Ok(std::process::Output { status, stdout, stderr }),
        None => Err(format!("{}--- stderr\n{}", String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr))),
    }
}

#[test]
fn threads_like_rustc() {
    let dir = std::env::temp_dir().join(format!("threads_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let names = ["channel", "pingpong", "producers", "mutex", "flag", "nested", "single", "parallel", "panic", "tls", "edges", "builder", "locks"];
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
            let o = match run_with_deadline(&exe, 60) {
                Ok(o) => o,
                Err(partial) => return Some(format!("{name} ({be}): still running after 60 s (a lost wakeup or a deadlock?), killed\n--- got\n{partial}")),
            };
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
            // (a Builder's name is the panic message's)
            if name == "builder" && !String::from_utf8_lossy(&o.stderr).contains("thread 'boom' panicked at src/main.rs:") {
                return Some(format!("{name} ({be}): stderr\n{}", String::from_utf8_lossy(&o.stderr)));
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
