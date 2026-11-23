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
//! - `//@ expect: unsupported`: rustc builds it, and zustc rejects it because it does not have the API (the CVE cannot
//!   happen). When zustc gets the API, the case fails: turn it into a test of the CVE's behavior.
//!
//! Every case names where it comes from: `//@ cve: CVE-..` (a std CVE; tests/security/cves.txt lists them all and
//! the case of each), `//@ issue: rust-lang/rust#..` (a rustc soundness bug), `//@ pattern: ..` (a neighbor of a
//! CVE or of a hole found here: the same implementation pattern elsewhere), or `//@ rule: ..` (a language rule
//! every safe program relies on, with no CVE of its own). An error code after `//~ ERROR` must be the code zustc
//! reports, when zustc's message has one.
//!
//! `//@ args: a b c` runs the program once per argument (one case per run, as a panic ends the
//! process). rustc (edition 2024, debug) is the oracle: the header must agree with what rustc does,
//! and the output is compared with its build's. Runtime cases are built with both backends
//! (Cranelift and minclif), and run under valgrind when it is installed (memory errors and
//! definite leaks fail; `ZUST_SECURITY_NO_VALGRIND=1` skips it).
//!
//! A case zustc does not pass yet says so with `//@ todo: <why>` (or `//@ todo(valgrind): <why>` when only valgrind
//! sees the failure, a leak). It is run and reported as TODO, and does not fail the test; once it passes, it fails the
//! test until its `todo` line is removed, so the TODOs only go away. TODO.md (at the top of the repository) lists
//! every one with what is left elsewhere. `ZUST_SECURITY_FILTER=substr` runs only matching cases.
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
    Unsupported,
}

struct Case {
    name: String,
    path: PathBuf,
    expect: Expect,
    args: Vec<String>,
    /// lines (1-based) marked `//~ ERROR`, with the error code the mark names (if any)
    error_lines: Vec<usize>,
    error_codes: Vec<(usize, String)>,
    /// (line, text the message must contain): `//~ ERROR E0133 "union field"`
    error_texts: Vec<(usize, String)>,
    /// `//@ cve:` ids
    cves: Vec<String>,
    /// `//@ todo: why`: not passed yet (and whether only valgrind sees the failure)
    todo: Option<(String, bool)>,
}

fn parse_case(path: &Path) -> Case {
    let src = std::fs::read_to_string(path).unwrap();
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    let mut expect = None;
    let mut args = vec![];
    let mut error_lines = vec![];
    let mut error_codes = vec![];
    let mut error_texts = vec![];
    let mut cves = vec![];
    let mut source = false;
    let mut todo = None;
    for (i, line) in src.lines().enumerate() {
        if let Some(c) = line.strip_prefix("//@ cve:") {
            cves.extend(c.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()));
            source = true;
        }
        if ["//@ issue:", "//@ pattern:", "//@ rule:"].iter().any(|p| line.starts_with(p)) {
            source = true;
        }
        if let Some(t) = line.strip_prefix("//@ todo:") {
            todo = Some((t.trim().to_string(), false));
        } else if let Some(t) = line.strip_prefix("//@ todo(valgrind):") {
            todo = Some((t.trim().to_string(), true));
        }
        if let Some(e) = line.strip_prefix("//@ expect:") {
            expect = Some(match e.trim() {
                "reject" => Expect::Reject,
                "panic" => Expect::Panic,
                "run" => Expect::Run,
                "safe" => Expect::Safe,
                "reject-or-panic" => Expect::RejectOrPanic,
                "unsupported" => Expect::Unsupported,
                o => panic!("{name}: unknown expect `{o}`"),
            });
        } else if let Some(a) = line.strip_prefix("//@ args:") {
            args = a.split_whitespace().map(str::to_string).collect();
        }
        if let Some(k) = line.find("//~ ERROR") {
            error_lines.push(i + 1);
            if let Some(code) = line[k + 9..].split_whitespace().next().filter(|w| w.len() == 5 && w.starts_with('E') && w[1..].chars().all(|c| c.is_ascii_digit())) {
                error_codes.push((i + 1, code.to_string()));
            }
            let rest = &line[k + 9..];
            if let (Some(a), Some(b)) = (rest.find('"'), rest.rfind('"')) {
                if b > a {
                    error_texts.push((i + 1, rest[a + 1..b].to_string()));
                }
            }
        }
    }
    assert!(source, "{name}: no `//@ cve:` / `//@ issue:` / `//@ pattern:` / `//@ rule:` line (where does the case come from?)");
    let expect = expect.unwrap_or_else(|| panic!("{name}: no `//@ expect:` line"));
    if matches!(expect, Expect::Reject | Expect::RejectOrPanic) {
        assert!(!error_lines.is_empty(), "{name}: a rejected case needs a `//~ ERROR` line");
    }
    Case { name, path: path.to_path_buf(), expect, args, error_lines, error_codes, error_texts, cves, todo }
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
    use std::os::unix::process::CommandExt;
    // (its own process group: zustc forks workers that keep the pipes open, so a timeout kills the whole group)
    cmd.process_group(0);
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
            let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
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

/// zustc's errors (`error: <path>: L:C: message`): (line, the `(Exxxx)` code in the message if any, the message)
fn errors(stderr: &str) -> Vec<(usize, Option<String>, String)> {
    stderr
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("error: ")?;
            let after = rest.split(".rs: ").nth(1)?;
            let line = after.split(':').next()?.trim().parse().ok()?;
            let code = after.match_indices("(E").find_map(|(k, _)| {
                let c = after.get(k + 1..k + 6)?;
                (c[1..].chars().all(|x| x.is_ascii_digit()) && after.get(k + 6..k + 7) == Some(")")).then(|| c.to_string())
            });
            Some((line, code, after.to_string()))
        })
        .collect()
}

/// A rejection that counts: at a marked line, and with the mark's code when zustc's message names one
fn rejected_as_marked(c: &Case, errs: &[(usize, Option<String>, String)]) -> bool {
    errs.iter().any(|(l, code, msg)| {
        c.error_lines.contains(l)
            && match (code, c.error_codes.iter().find(|x| x.0 == *l)) {
                (Some(got), Some((_, want))) => got == want,
                _ => true,
            }
            && c.error_texts.iter().filter(|x| x.0 == *l).all(|(_, t)| msg.contains(t.as_str()))
    })
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
        (Expect::Unsupported, false) => {}
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
        Expect::Reject | Expect::Unsupported => &["cranelift"],
        _ => &["cranelift", "minclif"],
    };
    for be in backends {
        let zbin = dir.join(format!("{}.{be}", c.name));
        let mut cmd = Command::new(zustc);
        cmd.arg(format!("--backend={be}")).arg("--no-cache").arg(&c.path).arg("-o").arg(&zbin);
        let b = run_limited(cmd, Duration::from_secs(120)).ok_or("zustc timed out")?;
        let stderr = String::from_utf8_lossy(&b.stderr).into_owned();
        if !b.status.success() {
            let at_marked = rejected_as_marked(c, &errors(&stderr));
            return match c.expect {
                Expect::Unsupported => Ok(()),
                Expect::Reject | Expect::RejectOrPanic if at_marked => Ok(()),
                Expect::Reject | Expect::RejectOrPanic => Err(format!("rejected, but not at a marked line {:?}: {}", c.error_lines, stderr.trim())),
                _ => Err(format!("zustc ({be}) does not build it: {}", stderr.trim())),
            };
        }
        if c.expect == Expect::Unsupported {
            return Err("zustc builds it now: it has the API, so make this case a test of the CVE's behavior (expect run / panic)".into());
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
                Expect::Reject | Expect::Unsupported => unreachable!(),
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
    let dir = private_dir("security");
    // (cases in parallel: each one runs rustc, zustc and the programs)
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(8);
    // (the cases with the most runs first: one of 20 runs, each under valgrind, takes as long as a quarter of the rest,
    // and started last it would run alone at the end)
    let mut order: Vec<usize> = (0..cases.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(cases[i].args.len()));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: std::sync::Mutex<Vec<(usize, Result<(), String>)>> = std::sync::Mutex::new(vec![]);
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&i) = order.get(k) else { break };
                    let c = &cases[i];
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
        match (r, &c.todo) {
            (Ok(()), None) => pass += 1,
            // (a failure only valgrind sees, and valgrind did not run: not known to pass)
            (Ok(()), Some((_, true))) if !valgrind => {
                gap += 1;
                eprintln!("TODO {}: not checked (valgrind did not run)", c.name);
            }
            (Ok(()), Some(_)) => bad.push(format!("{}: passes now; remove its `//@ todo` line (and its entry in TODO.md)", c.name)),
            (Err(e), Some((why, _))) => {
                gap += 1;
                eprintln!("TODO {}: {why}\n    {}", c.name, e.lines().next().unwrap_or(""));
            }
            (Err(e), None) => bad.push(format!("{}: {e}", c.name)),
        }
    }
    eprintln!("security: {pass} pass, {gap} TODO, {} failing (valgrind: {valgrind})", bad.len());
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// tests/security/cves.txt lists every std CVE (RustSec's advisories for `std`, and the Rust blog's) with the case
/// that covers it, or `n/a: <why>`. Each named case exists and says `//@ cve:` of that id; each `//@ cve:` id is listed.
#[test]
fn every_std_cve_has_a_case() {
    let text = std::fs::read_to_string("tests/security/cves.txt").unwrap();
    let mut listed: Vec<String> = vec![];
    let mut bad = vec![];
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let cols: Vec<&str> = line.split('\t').map(str::trim).collect();
        let (id, cover) = (cols[0], *cols.last().unwrap());
        listed.push(id.to_string());
        if cover.starts_with("n/a:") {
            continue;
        }
        for case in cover.split(',').map(str::trim) {
            let p = PathBuf::from(format!("tests/security/{case}.rs"));
            if !p.exists() {
                bad.push(format!("{id}: no case {case}"));
                continue;
            }
            if !parse_case(&p).cves.iter().any(|c| c == id) {
                bad.push(format!("{id}: {case} does not say `//@ cve: {id}`"));
            }
        }
    }
    for e in std::fs::read_dir("tests/security").unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            for id in parse_case(&p).cves {
                if !listed.contains(&id) {
                    bad.push(format!("{}: `//@ cve: {id}` is not in cves.txt", p.display()));
                }
            }
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// tests/security/unsafe_std_apis.txt: every unsafe std API needs `unsafe` in safe code. rustc must reject each with
/// E0133; zustc must reject it too (with E0133, or as an API it does not have). A row zustc does not pass yet has a
/// fourth column, `todo: why`
#[test]
fn unsafe_std_apis() {
    if !have("rustc") {
        eprintln!("security: rustc not found, skipped");
        return;
    }
    let text = std::fs::read_to_string("tests/security/unsafe_std_apis.txt").unwrap();
    let mut gaps: Vec<String> = vec![];
    let rows: Vec<(String, String, String)> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            assert!(c.len() == 3 || (c.len() == 4 && c[3].starts_with("todo:")), "unsafe_std_apis.txt: `{l}` is not name<TAB>setup<TAB>expression[<TAB>todo: why]");
            if c.len() == 4 {
                gaps.push(c[0].to_string());
            }
            (c[0].to_string(), c[1].to_string(), c[2].to_string())
        })
        .collect();
    let dir = private_dir("security_unsafe_api");
    let zustc = env!("CARGO_BIN_EXE_zustc");
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: std::sync::Mutex<Vec<(usize, Result<&'static str, String>)>> = std::sync::Mutex::new(vec![]);
    std::thread::scope(|s| {
        for _ in 0..std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(8) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((name, setup, expr)) = rows.get(i) else { break };
                    let src = format!("#![allow(unused)]\nfn main() {{\n    {setup}\n    let _r = {expr};\n}}\n");
                    let path = dir.0.join(format!("{name}.rs"));
                    std::fs::write(&path, &src).unwrap();
                    let r = Command::new("rustc").args(["--edition", "2024", "--error-format=short"]).arg(&path).arg("-o").arg(dir.0.join(format!("{name}.rustc"))).output().unwrap();
                    let rs = String::from_utf8_lossy(&r.stderr);
                    let res = if r.status.success() || !rs.lines().any(|l| l.contains(":4:") && l.contains("E0133")) {
                        Err(format!("rustc does not reject it with E0133 at the expression (fix the table): {}", rs.lines().next().unwrap_or("")))
                    } else {
                        let z = Command::new(zustc).arg("--no-cache").arg(&path).arg("-o").arg(dir.0.join(name.as_str())).output().unwrap();
                        let zs = String::from_utf8_lossy(&z.stderr);
                        if z.status.success() {
                            Err("zustc accepts it in safe code".to_string())
                        } else if zs.contains("(E0133)") {
                            Ok("E0133")
                        } else {
                            Ok("unsupported")
                        }
                    };
                    out.lock().unwrap().push((i, res));
                }
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    out.sort_by_key(|x| x.0);
    let (mut e0133, mut unsupported, mut bad) = (vec![], vec![], vec![]);
    for (i, r) in out {
        let name = &rows[i].0;
        let gap = gaps.contains(name);
        match (r, gap) {
            (Ok("E0133"), false) => e0133.push(name.clone()),
            (Ok(_), false) => unsupported.push(name.clone()),
            (Ok(_), true) => bad.push(format!("{name}: rejected now; remove its `todo:` column (and its entry in TODO.md)")),
            (Err(e), true) => eprintln!("TODO unsafe API {name}: {e}"),
            (Err(e), false) => bad.push(format!("{name}: {e}")),
        }
    }
    eprintln!("unsafe std APIs: {} need unsafe (E0133), {} not supported by zustc: {}", e0133.len(), unsupported.len(), unsupported.join(" "));
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// TODO.md names every case that has a `//@ todo` line (in a table row, `| \`name\` | why |`), and every case its
/// table rows name still has one, so the list of what is left is in one place
#[test]
fn todo_md_lists_every_todo() {
    let md = std::fs::read_to_string("TODO.md").unwrap();
    let rows: Vec<&str> = md.lines().filter_map(|l| l.strip_prefix("| `")?.split('`').next()).collect();
    let mut bad = vec![];
    let mut cases = vec![];
    for e in std::fs::read_dir("tests/security").unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            let c = parse_case(&p);
            let named = rows.contains(&c.name.as_str());
            match (&c.todo, named) {
                (Some(_), false) => bad.push(format!("{}: has a `//@ todo` line but no row of TODO.md names it", c.name)),
                (None, true) => bad.push(format!("{}: a row of TODO.md names it, but it has no `//@ todo` line (done? remove the row)", c.name)),
                _ => {}
            }
            cases.push(c.name);
        }
    }
    for r in &rows {
        if !cases.iter().any(|c| c == r) {
            bad.push(format!("TODO.md names `{r}`, which is not a case of tests/security"));
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
