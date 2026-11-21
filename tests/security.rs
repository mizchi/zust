//! Security regression tests: programs taken from past Rust CVEs and soundness bugs
//! (tests/security/*.rs), each checking that zustc does not let the same hole through.
//!
//! Every case says what safe behavior is in its header:
//!
//! - `//@ expect: reject`: zustc must reject it, with an error on a line marked `//~ ERROR`
//!   (an error elsewhere is a rejection for some other reason, and does not count).
//! - `//@ expect: panic`: it builds, and every run exits with a panic (101) after printing what
//!   rustc's build prints.
//! - `//@ expect: run`: it builds, exits 0 and prints exactly what rustc's build prints.
//! - `//@ expect: safe`: it builds, and every run either exits 0 printing only lines that start
//!   with `ok`, or panics. (For cases where rustc itself may do either.)
//! - `//@ expect: reject-or-panic`: rejected at a marked line, or built and every run panics.
//!
//! `//@ args: a b c` runs the program once per argument (one case per run, as a panic ends the
//! process). rustc (edition 2024, debug) is the oracle: the header must agree with what rustc does,
//! and the output is compared with its build's. Runtime cases are built with both backends
//! (Cranelift and minclif), and run under valgrind when it is installed (memory errors and
//! definite leaks fail; `ZUST_SECURITY_NO_VALGRIND=1` skips it).
//!
//! `tests/security/known_gaps.txt` lists the cases zustc does not pass yet, with the reason. A
//! listed case that fails is reported but does not fail the test; a listed case that passes fails
//! it, so the list only ever shrinks. `ZUST_SECURITY_FILTER=substr` runs only matching cases.
#![cfg(feature = "acg")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Expect {
    Reject,
    Panic,
    Run,
    Safe,
    RejectOrPanic,
}

struct Case {
    name: String,
    path: PathBuf,
    expect: Expect,
    args: Vec<String>,
    /// lines (1-based) marked `//~ ERROR`
    error_lines: Vec<usize>,
}

fn parse_case(path: &Path) -> Case {
    let src = std::fs::read_to_string(path).unwrap();
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    let mut expect = None;
    let mut args = vec![];
    let mut error_lines = vec![];
    for (i, line) in src.lines().enumerate() {
        if let Some(e) = line.strip_prefix("//@ expect:") {
            expect = Some(match e.trim() {
                "reject" => Expect::Reject,
                "panic" => Expect::Panic,
                "run" => Expect::Run,
                "safe" => Expect::Safe,
                "reject-or-panic" => Expect::RejectOrPanic,
                o => panic!("{name}: unknown expect `{o}`"),
            });
        } else if let Some(a) = line.strip_prefix("//@ args:") {
            args = a.split_whitespace().map(str::to_string).collect();
        }
        if line.contains("//~ ERROR") {
            error_lines.push(i + 1);
        }
    }
    let expect = expect.unwrap_or_else(|| panic!("{name}: no `//@ expect:` line"));
    if matches!(expect, Expect::Reject | Expect::RejectOrPanic) {
        assert!(!error_lines.is_empty(), "{name}: a rejected case needs a `//~ ERROR` line");
    }
    Case { name, path: path.to_path_buf(), expect, args, error_lines }
}

/// A new directory only this user can enter (as tests/panic_loc.rs), removed when dropped
struct PrivateDir(PathBuf);

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn private_dir(prefix: &str) -> PrivateDir {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::temp_dir();
    for k in 0..100u32 {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = base.join(format!("{prefix}_{}_{t}_{k}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return PrivateDir(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("{}: {e}", dir.display()),
        }
    }
    panic!("no new directory under {}", base.display());
}

/// Runs `cmd` with a time limit; None when it had to be killed
fn run_limited(mut cmd: Command, limit: Duration) -> Option<Output> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    // (read the pipes on threads, so a chatty child does not block on a full pipe)
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let to = std::thread::spawn(move || {
        let mut b = vec![];
        std::io::Read::read_to_end(&mut so, &mut b).ok();
        b
    });
    let te = std::thread::spawn(move || {
        let mut b = vec![];
        std::io::Read::read_to_end(&mut se, &mut b).ok();
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = to.join().unwrap();
    let stderr = te.join().unwrap();
    status.map(|status| Output { status, stdout, stderr })
}

fn describe(o: &Option<Output>) -> String {
    use std::os::unix::process::ExitStatusExt;
    match o {
        None => "timed out".to_string(),
        Some(o) => match (o.status.code(), o.status.signal()) {
            (Some(c), _) => format!("exit {c}"),
            (None, Some(s)) => format!("killed by signal {s}"),
            _ => "?".to_string(),
        },
    }
}

fn code(o: &Option<Output>) -> Option<i32> {
    o.as_ref().and_then(|o| o.status.code())
}

/// The lines of zustc's errors (`error: <path>: L:C: message`)
fn error_lines(stderr: &str) -> Vec<usize> {
    stderr
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("error: ")?;
            let after = rest.split(".rs: ").nth(1)?;
            after.split(':').next()?.trim().parse().ok()
        })
        .collect()
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

const LIMIT: Duration = Duration::from_secs(20);

/// Checks one case; Err(what went wrong)
fn check_case(c: &Case, dir: &Path, valgrind: bool) -> Result<(), String> {
    let zustc = env!("CARGO_BIN_EXE_zustc");
    let runs: Vec<Option<&str>> = if c.args.is_empty() { vec![None] } else { c.args.iter().map(|a| Some(a.as_str())).collect() };
    // rustc, the oracle: the header must say what rustc does
    let rbin = dir.join(format!("{}.rustc", c.name));
    let r = Command::new("rustc").args(["-Awarnings", "--edition", "2024"]).arg(&c.path).arg("-o").arg(&rbin).output().unwrap();
    let rustc_rejects = !r.status.success();
    let mut rustc_out = vec![];
    match (c.expect, rustc_rejects) {
        (Expect::Reject | Expect::RejectOrPanic, true) => {}
        (Expect::Reject | Expect::RejectOrPanic, false) => return Err("header says reject, but rustc accepts it (fix the case)".into()),
        (_, true) => return Err(format!("rustc rejects the case (fix it):\n{}", String::from_utf8_lossy(&r.stderr))),
        (e, false) => {
            for a in &runs {
                let mut cmd = Command::new(&rbin);
                cmd.args(a.iter());
                let o = run_limited(cmd, LIMIT);
                let ok = match e {
                    Expect::Panic => code(&o) == Some(101),
                    Expect::Run => code(&o) == Some(0),
                    _ => matches!(code(&o), Some(0 | 101)),
                };
                if !ok {
                    return Err(format!("header says {e:?}, but rustc's build gives {} for {a:?} (fix the case)", describe(&o)));
                }
                rustc_out.push(o.unwrap().stdout);
            }
        }
    }
    let backends: &[&str] = match c.expect {
        Expect::Reject => &["cranelift"],
        _ => &["cranelift", "minclif"],
    };
    for be in backends {
        let zbin = dir.join(format!("{}.{be}", c.name));
        let mut cmd = Command::new(zustc);
        cmd.arg(format!("--backend={be}")).arg("--no-cache").arg(&c.path).arg("-o").arg(&zbin);
        let b = run_limited(cmd, Duration::from_secs(120)).ok_or("zustc timed out")?;
        let stderr = String::from_utf8_lossy(&b.stderr).into_owned();
        if !b.status.success() {
            let lines = error_lines(&stderr);
            let at_marked = lines.iter().any(|l| c.error_lines.contains(l));
            return match c.expect {
                Expect::Reject | Expect::RejectOrPanic if at_marked => Ok(()),
                Expect::Reject | Expect::RejectOrPanic => Err(format!("rejected, but not at a marked line {:?}: {}", c.error_lines, stderr.trim())),
                _ => Err(format!("zustc ({be}) does not build it: {}", stderr.trim())),
            };
        }
        if c.expect == Expect::Reject {
            // say what the accepted program does: that is the hole
            let mut cmd = Command::new(&zbin);
            cmd.args(runs[0].iter());
            return Err(format!("zustc accepts it (its build: {})", describe(&run_limited(cmd, LIMIT))));
        }
        for (k, a) in runs.iter().enumerate() {
            let mut cmd = Command::new(&zbin);
            cmd.args(a.iter());
            let o = run_limited(cmd, LIMIT);
            let what = format!("{be}{}", a.map(|a| format!(", arg {a}")).unwrap_or_default());
            let stdout = o.as_ref().map(|o| o.stdout.clone()).unwrap_or_default();
            let same_out = rustc_out.get(k).is_some_and(|r| *r == stdout);
            let ok = match c.expect {
                Expect::Run => code(&o) == Some(0) && same_out,
                Expect::Panic => code(&o) == Some(101) && same_out,
                Expect::RejectOrPanic => code(&o) == Some(101),
                Expect::Safe => match code(&o) {
                    Some(0) => String::from_utf8_lossy(&stdout).lines().all(|l| l.starts_with("ok")),
                    Some(101) => true,
                    _ => false,
                },
                Expect::Reject => unreachable!(),
            };
            if !ok {
                let mut msg = format!("{what}: {} (want {:?})", describe(&o), c.expect);
                if !same_out && matches!(c.expect, Expect::Run | Expect::Panic) {
                    let want = rustc_out.get(k).map(|r| String::from_utf8_lossy(r).into_owned()).unwrap_or_default();
                    msg += &format!("\n  stdout: {:?}\n  rustc:  {:?}", String::from_utf8_lossy(&stdout), want);
                }
                if let Some(o) = &o {
                    let e = String::from_utf8_lossy(&o.stderr);
                    if !e.trim().is_empty() {
                        msg += &format!("\n  stderr: {}", e.trim());
                    }
                }
                return Err(msg);
            }
            if valgrind {
                let log = dir.join(format!("{}.{be}.{k}.vg", c.name));
                let mut cmd = Command::new("valgrind");
                cmd.args(["-q", "--leak-check=full", "--errors-for-leak-kinds=definite"]).arg(format!("--log-file={}", log.display())).arg(&zbin).args(a.iter());
                run_limited(cmd, Duration::from_secs(120)).ok_or_else(|| format!("{what}: valgrind timed out"))?;
                let v = std::fs::read_to_string(&log).unwrap_or_default();
                if !v.trim().is_empty() {
                    return Err(format!("{what}: valgrind:\n{}", v.lines().take(20).collect::<Vec<_>>().join("\n")));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn security_regressions() {
    if !have("rustc") {
        eprintln!("security: rustc not found, skipped");
        return;
    }
    let valgrind = std::env::var_os("ZUST_SECURITY_NO_VALGRIND").is_none() && have("valgrind");
    let filter = std::env::var("ZUST_SECURITY_FILTER").unwrap_or_default();
    let mut files: Vec<PathBuf> = std::fs::read_dir("tests/security").unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "rs")).collect();
    files.sort();
    let cases: Vec<Case> = files.iter().map(|p| parse_case(p)).filter(|c| c.name.contains(&filter)).collect();
    let gaps_src = std::fs::read_to_string("tests/security/known_gaps.txt").unwrap();
    let gaps: Vec<&str> = gaps_src.lines().map(|l| l.split('#').next().unwrap().trim()).filter(|l| !l.is_empty()).collect();
    for g in &gaps {
        assert!(files.iter().any(|p| p.file_stem().unwrap() == *g), "known_gaps.txt names `{g}`, which is not a case");
    }
    let dir = private_dir("security");
    // (cases in parallel: each one runs rustc, zustc and the programs)
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<(usize, Result<(), String>)>> = std::sync::Mutex::new(vec![]);
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(c) = cases.get(i) else { break };
                    let r = check_case(c, &dir.0, valgrind);
                    results.lock().unwrap().push((i, r));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);
    let mut bad = vec![];
    let (mut pass, mut gap) = (0, 0);
    for (i, r) in &results {
        let c = &cases[*i];
        let known = gaps.contains(&c.name.as_str());
        match (r, known) {
            (Ok(()), false) => pass += 1,
            (Ok(()), true) => bad.push(format!("{}: passes now; remove it from tests/security/known_gaps.txt", c.name)),
            (Err(e), true) => {
                gap += 1;
                eprintln!("known gap {}: {}", c.name, e.lines().next().unwrap_or(""));
            }
            (Err(e), false) => bad.push(format!("{}: {e}", c.name)),
        }
    }
    eprintln!("security: {pass} pass, {gap} known gaps, {} failing (valgrind: {valgrind})", bad.len());
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
