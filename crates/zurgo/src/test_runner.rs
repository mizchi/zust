//! `zurgo test`'s runner, on nextest's model: list the tests of every test executable, then run each test in a
//! process of its own (`<exe> <name> --exact`), all of them over the cores at once.
//!
//! It speaks libtest's command line (`--list --format terse`, `--exact`, `--ignored`, `--include-ignored`), so it
//! runs zustc's harness (zust_syntax::test_harness) and a libtest executable alike. A test passes when its process
//! exits with 0: `#[should_panic]` is the harness's to check.
//!
//! The longest tests start first: each test's last time is kept in `<out>/test-times` (`label`, `name`, ms).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

/// A test executable: what it is (`pkg (lib)`, `pkg (bin x)`, `pkg (test x)`), where it is, the directory it runs in
/// (the package's), and whether it has a test harness. One with none (`harness = false`) is one test, named after
/// its target, that passes when the executable exits with 0
#[derive(Clone, Debug)]
pub struct TestExe {
    pub label: String,
    /// the target's name (the name of the one test of an executable with no harness)
    pub target: String,
    pub exe: PathBuf,
    pub dir: PathBuf,
    pub harness: bool,
}

impl TestExe {
    /// one with a harness (cargo's own executables too)
    pub fn new(label: &str, exe: impl Into<PathBuf>, dir: impl Into<PathBuf>) -> TestExe {
        TestExe { label: label.to_string(), target: label.to_string(), exe: exe.into(), dir: dir.into(), harness: true }
    }
}

#[derive(Clone, Debug)]
pub struct RunOpts {
    /// workers (`--test-threads`; the cores by default)
    pub threads: usize,
    /// the tests to run: names containing one of these (whole names with `exact`)
    pub filters: Vec<String>,
    pub exact: bool,
    pub skip: Vec<String>,
    /// run the ignored tests only
    pub ignored: bool,
    /// run the ignored tests as well
    pub include_ignored: bool,
    /// stop starting tests after the first failure
    pub fail_fast: bool,
    /// a test still running then is marked SLOW
    pub slow: Duration,
    /// a test still running then is stopped, and fails
    pub timeout: Duration,
    /// only list the tests
    pub list: bool,
}

impl Default for RunOpts {
    fn default() -> Self {
        RunOpts {
            threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2),
            filters: vec![],
            exact: false,
            skip: vec![],
            ignored: false,
            include_ignored: false,
            fail_fast: false,
            slow: Duration::from_secs(60),
            timeout: Duration::from_secs(300),
            list: false,
        }
    }
}

/// One test to run
struct Job {
    /// index of its executable
    exe: usize,
    name: String,
    ignored: bool,
    /// `#[should_panic]`'s expected text, for a wasm32 module (where the harness cannot run the test in a child: the
    /// runner checks the panic)
    should_panic: Option<String>,
}

enum Outcome {
    Pass,
    Fail(Option<i32>),
    Timeout,
}

struct Done {
    job: usize,
    outcome: Outcome,
    time: Duration,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// The command that runs an executable: a wasm32 module (`.wasm`) under cargo's runner variable,
/// `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER` (split on white space; a word that names a file from zurgo's directory
/// is made absolute, as the test runs in its package's directory)
fn command(exe: &Path) -> Result<Command, String> {
    if exe.extension().is_none_or(|x| x != "wasm") {
        return Ok(Command::new(exe));
    }
    let var = "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER";
    let runner = std::env::var(var).unwrap_or_default();
    let mut words = runner.split_whitespace().map(|w| {
        let p = Path::new(w);
        if p.is_relative() && p.exists() { std::fs::canonicalize(p).map(|a| a.display().to_string()).unwrap_or_else(|_| w.to_string()) } else { w.to_string() }
    });
    let Some(prog) = words.next() else { return Err(format!("{} is a wasm32 module: set {var} to a program that runs it (`node tests/wasm/run.mjs` in this repository)", exe.display())) };
    let mut c = Command::new(prog);
    c.args(words).arg(exe);
    Ok(c)
}

/// A wasm32 module's tests, from zustc's harness (`--list --format zust`): (name, ignored, should_panic's expected)
fn list_zust(exe: &Path, dir: &Path) -> Result<Vec<(String, bool, Option<String>)>, String> {
    let mut c = command(exe)?;
    c.args(["--list", "--format", "zust"]).current_dir(dir).stdin(Stdio::null());
    let o = c.output().map_err(|e| format!("{}: {e}", exe.display()))?;
    if !o.status.success() {
        return Err(format!("{} --list failed ({}):\n{}", exe.display(), o.status, String::from_utf8_lossy(&o.stderr)));
    }
    let mut v = vec![];
    for l in String::from_utf8_lossy(&o.stdout).lines() {
        let Some((name, rest)) = l.split_once(": test") else { continue };
        let ignored = rest.starts_with(" ignore");
        let sp = rest.find(" should_panic ").map(|i| debug_str(rest[i + 14..].trim()));
        v.push((name.to_string(), ignored, sp));
    }
    Ok(v)
}

/// The value of a string as `{:?}` writes it (`"a\"b"`)
fn debug_str(s: &str) -> String {
    let s = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s);
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('u') => {
                let h: String = it.by_ref().skip(1).take_while(|c| *c != '}').collect();
                out.extend(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32));
            }
            Some(x) => out.push(x),
            None => {}
        }
    }
    out
}

/// A panic's message in a test's stderr: after the last panic header (a line `thread '..' panicked at ..:`), to the
/// `note:` line. None: no panic
fn panic_msg(err: &str) -> Option<String> {
    let mut at = None;
    let mut pos = 0;
    for l in err.split('\n') {
        if l.starts_with("thread '") && l.contains("' panicked at ") && l.ends_with(':') {
            at = Some(pos + l.len() + 1);
        }
        pos += l.len() + 1;
    }
    let m = err.get(at?..).unwrap_or("");
    let m = m.find("\nnote: run with `RUST_BACKTRACE").map_or(m, |e| &m[..e]);
    Some(m.to_string())
}

/// `<exe> --list --format terse [--ignored]`: the names it prints (`name: test`)
fn list(exe: &Path, dir: &Path, ignored: bool) -> Result<Vec<String>, String> {
    let mut c = command(exe)?;
    c.args(["--list", "--format", "terse"]).current_dir(dir).stdin(Stdio::null());
    if ignored {
        c.arg("--ignored");
    }
    let o = c.output().map_err(|e| format!("{}: {e}", exe.display()))?;
    if !o.status.success() {
        return Err(format!("{} --list failed ({}):\n{}", exe.display(), o.status, String::from_utf8_lossy(&o.stderr)));
    }
    Ok(String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.strip_suffix(": test")).map(str::to_string).collect())
}

fn matches(name: &str, pats: &[String], exact: bool) -> bool {
    pats.iter().any(|p| if exact { name == p } else { name.contains(p.as_str()) })
}

/// The last time of each test: (label, name) → ms
fn read_times(p: &Path) -> BTreeMap<(String, String), u64> {
    let mut m = BTreeMap::new();
    for l in std::fs::read_to_string(p).unwrap_or_default().lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if let [a, b, t] = f[..] {
            if let Ok(t) = t.parse() {
                m.insert((a.to_string(), b.to_string()), t);
            }
        }
    }
    m
}

fn write_times(p: &Path, m: &BTreeMap<(String, String), u64>) {
    let mut s = String::new();
    for ((a, b), t) in m {
        // (a label or name with a tab or a line break would not read back: such a test is left out)
        if !a.contains(['\t', '\n']) && !b.contains(['\t', '\n']) {
            s.push_str(&format!("{a}\t{b}\t{t}\n"));
        }
    }
    let _ = std::fs::write(p, s);
}

/// Runs one test: its process, its output, its time; stopped at `timeout`. (`harness`: false runs the executable as
/// it is, the whole of it one test)
fn run_one(exe: &Path, dir: &Path, name: &str, ignored: bool, harness: bool, should_panic: Option<&str>, o: &RunOpts, slow: &dyn Fn()) -> (Outcome, Vec<u8>, Vec<u8>) {
    let mut c = match command(exe) {
        Ok(c) => c,
        Err(e) => return (Outcome::Fail(None), vec![], format!("{e}\n").into_bytes()),
    };
    if harness {
        c.arg(name).arg("--exact").arg("--nocapture");
    }
    c.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if ignored {
        c.arg("--include-ignored");
    }
    // (a process group of its own: at a timeout, the processes the test started go with it)
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return (Outcome::Fail(None), vec![], format!("{}: {e}\n", exe.display()).into_bytes()),
    };
    // (the pipes on threads of their own: a test that writes much does not block on a full pipe)
    let mut po = child.stdout.take().unwrap();
    let mut pe = child.stderr.take().unwrap();
    let (tx_o, rx_o) = mpsc::channel();
    let (tx_e, rx_e) = mpsc::channel();
    std::thread::spawn(move || {
        let mut v = vec![];
        let _ = po.read_to_end(&mut v);
        let _ = tx_o.send(v);
    });
    std::thread::spawn(move || {
        let mut v = vec![];
        let _ = pe.read_to_end(&mut v);
        let _ = tx_e.send(v);
    });
    let pgid = child.id();
    let kill_group = || {
        let _ = Command::new("kill").args(["-KILL", "--", &format!("-{pgid}")]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    let t0 = Instant::now();
    let mut said_slow = false;
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(st)) => break if st.success() { Outcome::Pass } else { Outcome::Fail(st.code()) },
            Ok(None) => {}
            Err(_) => break Outcome::Fail(None),
        }
        let e = t0.elapsed();
        if e >= o.timeout {
            kill_group();
            let _ = child.kill();
            let _ = child.wait();
            break Outcome::Timeout;
        }
        if !said_slow && e >= o.slow {
            said_slow = true;
            slow();
        }
        std::thread::sleep(Duration::from_millis(if e < Duration::from_millis(50) { 1 } else { 10 }));
    };
    // (the output: a process the test started and left running may hold the pipes open. Wait a second for them; then
    //  stop the test's process group and wait one more; then go on without the rest of the output)
    let mut left = false;
    let mut take = |rx: &mpsc::Receiver<Vec<u8>>| -> Vec<u8> {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(v) => v,
            Err(_) => {
                left = true;
                kill_group();
                rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default()
            }
        }
    };
    let out = take(&rx_o);
    let mut err = take(&rx_e);
    if left {
        err.extend_from_slice(b"\n(zurgo: a process the test started kept its output open after it ended: stopped)\n");
    }
    // (a wasm32 `#[should_panic]` test ran in the module: it passes when it panicked, with the expected text)
    let outcome = match (should_panic, outcome) {
        // (no expected text: any panic, a trap too (wasm32's checks trap without a message))
        (Some(exp), Outcome::Fail(Some(101))) if exp.is_empty() || panic_msg(&String::from_utf8_lossy(&err)).is_some_and(|m| m.contains(exp)) => Outcome::Pass,
        (Some(exp), Outcome::Fail(Some(101))) => {
            err.extend_from_slice(format!("note: panic did not contain expected string\n      expected substring: {exp:?}\n").as_bytes());
            Outcome::Fail(Some(101))
        }
        (Some(_), Outcome::Pass) => {
            err.extend_from_slice(b"note: test did not panic as expected\n");
            Outcome::Fail(Some(0))
        }
        (_, x) => x,
    };
    (outcome, out, err)
}

/// What a run did: each test that ran, (label, name, passed), in the order they finished; and how many were skipped
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub results: Vec<(String, String, bool)>,
    pub skipped: usize,
}

impl Summary {
    pub fn ok(&self) -> bool {
        self.results.iter().all(|r| r.2)
    }
}

/// Lists and runs the tests of `exes`
pub fn run(exes: &[TestExe], o: &RunOpts, times_path: &Path) -> Result<Summary, String> {
    let t_all = Instant::now();
    let mut jobs: Vec<Job> = vec![];
    let mut skipped = 0usize;
    for (i, x) in exes.iter().enumerate() {
        let (label, exe, dir) = (&x.label, &x.exe, &x.dir);
        // (no harness: one test, named after its target)
        // (a wasm32 module: zustc's harness says each test's flags at once; its `should_panic` the runner checks)
        let mut panics: BTreeMap<String, String> = BTreeMap::new();
        let (names, ign) = if !x.harness {
            (vec![x.target.clone()], vec![])
        } else if exe.extension().is_some_and(|e| e == "wasm") {
            let l = list_zust(exe, dir)?;
            for (n, _, sp) in &l {
                if let Some(sp) = sp {
                    panics.insert(n.clone(), sp.clone());
                }
            }
            (l.iter().map(|t| t.0.clone()).collect(), l.iter().filter(|t| t.1).map(|t| t.0.clone()).collect())
        } else {
            (list(exe, dir, false)?, list(exe, dir, true)?)
        };
        for n in names {
            if (!o.filters.is_empty() && !matches(&n, &o.filters, o.exact)) || matches(&n, &o.skip, o.exact) {
                continue;
            }
            let ignored = ign.contains(&n);
            if o.list {
                // (`--ignored`: the ignored ones alone, as libtest lists them)
                if !o.ignored || ignored {
                    println!("{label}: {n}{}", if ignored { " (ignored)" } else { "" });
                }
                continue;
            }
            let runs = if o.ignored { ignored } else { !ignored || o.include_ignored };
            if !runs {
                skipped += 1;
                continue;
            }
            let should_panic = panics.get(&n).cloned();
            jobs.push(Job { exe: i, name: n, ignored, should_panic });
        }
    }
    if o.list {
        return Ok(Summary::default());
    }
    // (the longest first, by their last times; a test not seen before after them, in the order listed)
    let mut times = read_times(times_path);
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    order.sort_by_key(|&k| std::cmp::Reverse(times.get(&(exes[jobs[k].exe].label.clone(), jobs[k].name.clone())).copied().unwrap_or(0)));
    let threads = o.threads.max(1).min(jobs.len().max(1));
    eprintln!("    Starting {} tests across {} binaries ({skipped} skipped, {threads} threads)", jobs.len(), exes.len());
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel::<Done>();
    let slow_said: Mutex<Vec<usize>> = Mutex::new(vec![]);
    let (mut passed, mut failed): (usize, Vec<usize>) = (0, vec![]);
    let mut results: Vec<(String, String, bool)> = vec![];
    std::thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let (jobs, order, next, stop, slow_said) = (&jobs, &order, &next, &stop, &slow_said);
            s.spawn(move || {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&j) = order.get(k) else { break };
                    let job = &jobs[j];
                    let x = &exes[job.exe];
                    let (label, exe, dir) = (&x.label, &x.exe, &x.dir);
                    let t0 = Instant::now();
                    let slow = || {
                        eprintln!("        SLOW [>{:>8.3}s] {label} {}", o.slow.as_secs_f64(), job.name);
                        slow_said.lock().unwrap().push(j);
                    };
                    let (outcome, stdout, stderr) = run_one(exe, dir, &job.name, job.ignored, x.harness, job.should_panic.as_deref(), o, &slow);
                    // (fail-fast: no other test starts once one failed, before the result reaches the printer)
                    if o.fail_fast && !matches!(outcome, Outcome::Pass) {
                        stop.store(true, Ordering::Relaxed);
                    }
                    if tx.send(Done { job: j, outcome, time: t0.elapsed(), stdout, stderr }).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // (the results as they come: one line each, a failure's output after its line)
        let total = jobs.len();
        for (n, d) in rx.into_iter().enumerate() {
            let job = &jobs[d.job];
            let label = &exes[job.exe].label;
            let secs = d.time.as_secs_f64();
            times.insert((label.clone(), job.name.clone()), d.time.as_millis() as u64);
            results.push((label.clone(), job.name.clone(), matches!(d.outcome, Outcome::Pass)));
            let status = match d.outcome {
                Outcome::Pass => {
                    passed += 1;
                    "PASS".to_string()
                }
                Outcome::Fail(c) => {
                    failed.push(d.job);
                    match c {
                        Some(c) => format!("FAIL ({c})"),
                        None => "SIGNAL".to_string(),
                    }
                }
                Outcome::Timeout => {
                    failed.push(d.job);
                    "TIMEOUT".to_string()
                }
            };
            eprintln!("{status:>12} [{secs:>8.3}s] ({:>w$}/{total}) {label} {}", n + 1, job.name, w = total.to_string().len());
            if !matches!(d.outcome, Outcome::Pass) {
                for (what, b) in [("STDOUT", &d.stdout), ("STDERR", &d.stderr)] {
                    if !b.is_empty() {
                        eprintln!("--- {what}: {label} {} ---\n{}", job.name, String::from_utf8_lossy(b).trim_end());
                    }
                }
                if o.fail_fast {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    write_times(times_path, &times);
    let not_run = jobs.len() - passed - failed.len();
    eprintln!(
        "     Summary [{:>8.3}s] {} tests run: {passed} passed, {} failed, {skipped} skipped{}",
        t_all.elapsed().as_secs_f64(),
        passed + failed.len(),
        failed.len(),
        if not_run > 0 { format!(", {not_run} not run (fail-fast)") } else { String::new() }
    );
    for &j in &failed {
        eprintln!("        FAIL {} {}", exes[jobs[j].exe].label, jobs[j].name);
    }
    Ok(Summary { results, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_message() {
        let err = "thread 'main' panicked at src/lib.rs:3:5:\nworker' panicked at failure\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";
        assert_eq!(panic_msg(err).as_deref(), Some("worker' panicked at failure"));
        assert_eq!(panic_msg("wasm trap: unreachable\n"), None);
        assert_eq!(debug_str("\"a\\\"b\\n\\u{41}\""), "a\"b\nA");
    }
}
