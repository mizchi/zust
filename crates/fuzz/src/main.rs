//! zust-fuzz: mutation-based fuzzing of zustc's front end and checker, with std only (no libFuzzer, stable Rust).
//!
//! ```text
//! zust-fuzz fuzz [--target T] [--corpus DIR].. [--time SECS] [--seed N] [--out DIR] [--timeout-ms MS] [--max-len N]
//! zust-fuzz replay [--target T] [--edition E] FILE..   run files once; exit 1 when one panics or mismatches
//! zust-fuzz minimize [--target T] [--edition E] FILE   the smallest source that still fails the same way
//! zust-fuzz show-expand FILE..                         both macro expanders' outputs, for an expand-diff mismatch
//! ```
//!
//! Targets (`--target`, default `check`): lex, parse, expand, expand-diff, prep, check (see targets.rs), several
//! separated by commas, or `all` (each input through every target).
//!
//! `fuzz` runs a supervisor that starts a worker process (this binary, `worker`) and restarts it after a crash: a
//! panic is caught in the worker, but a stack overflow or an abort kills it. Before each input the worker writes it to
//! `<out>/current.rs`, so the supervisor saves what killed it. A watchdog thread in the worker saves an input that runs
//! past `--timeout-ms` as a hang, and ends the worker. Findings go to `<out>/findings/` with the minimized source next
//! to them (`.min.rs`); the corpus is kept in memory, and an input whose answer (the start of its first error) is new
//! joins it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zust_fuzz::targets::{self, Target};
use zust_fuzz::{Failure, hash, install_panic_hook, mutate, rng, run_one};

struct Args {
    targets: Vec<Target>,
    corpus: Vec<PathBuf>,
    time: u64,
    seed: u64,
    out: PathBuf,
    timeout_ms: u64,
    max_len: usize,
    edition: u16,
    strict_diff: bool,
    files: Vec<PathBuf>,
}

fn parse_args(it: &[String]) -> Result<Args, String> {
    let mut a = Args {
        targets: vec![Target::Check],
        corpus: vec![],
        time: 60,
        seed: SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_secs()),
        out: PathBuf::from("out/fuzz"),
        timeout_ms: 5000,
        max_len: 16 * 1024,
        edition: 2021,
        strict_diff: false,
        files: vec![],
    };
    let mut i = 0;
    while i < it.len() {
        let val = |i: usize| it.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", it[i]));
        match it[i].as_str() {
            "--target" => {
                let v = val(i)?;
                a.targets = if v == "all" {
                    targets::ALL.to_vec()
                } else {
                    v.split(',').map(|t| Target::parse(t).ok_or(format!("unknown target {t}"))).collect::<Result<_, _>>()?
                };
                i += 1;
            }
            "--corpus" => {
                a.corpus.push(PathBuf::from(val(i)?));
                i += 1;
            }
            "--time" => {
                a.time = val(i)?.parse().map_err(|e| format!("--time: {e}"))?;
                i += 1;
            }
            "--seed" => {
                a.seed = val(i)?.parse().map_err(|e| format!("--seed: {e}"))?;
                i += 1;
            }
            "--out" => {
                a.out = PathBuf::from(val(i)?);
                i += 1;
            }
            "--timeout-ms" => {
                a.timeout_ms = val(i)?.parse().map_err(|e| format!("--timeout-ms: {e}"))?;
                i += 1;
            }
            "--strict-diff" => a.strict_diff = true,
            "--edition" => {
                a.edition = val(i)?.parse().map_err(|e| format!("--edition: {e}"))?;
                i += 1;
            }
            "--max-len" => {
                a.max_len = val(i)?.parse().map_err(|e| format!("--max-len: {e}"))?;
                i += 1;
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => a.files.push(PathBuf::from(s)),
        }
        i += 1;
    }
    if a.corpus.is_empty() {
        a.corpus = vec![PathBuf::from("examples"), PathBuf::from("tests/probe"), PathBuf::from("tests/fuzz")];
    }
    Ok(a)
}

/// The .rs files under the corpus directories (or the files themselves), at most 64 KiB each.
fn load_corpus(dirs: &[PathBuf]) -> Vec<String> {
    fn walk(p: &Path, out: &mut Vec<String>) {
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            let mut es: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            es.sort();
            for e in es {
                walk(&e, out);
            }
        } else if p.extension().is_some_and(|e| e == "rs") && std::fs::metadata(p).is_ok_and(|m| m.len() <= 64 * 1024) {
            if let Ok(s) = std::fs::read_to_string(p) {
                out.push(s);
            }
        }
    }
    let mut out = vec![];
    for d in dirs {
        walk(d, &mut out);
    }
    if out.is_empty() {
        out.push("fn main() {}\n".to_string());
    }
    out
}

/// Saves a finding: its input, the minimized input when there is one, and what was found. The input's path, or the
/// error that kept a file from being written
fn save_finding(out: &Path, f: &Failure, src: &str, min: Option<&str>, edition: u16) -> std::io::Result<PathBuf> {
    let dir = out.join("findings");
    std::fs::create_dir_all(&dir)?;
    let name = format!("{}-{:016x}", f.kind, hash(&f.place));
    let p = dir.join(format!("{name}.rs"));
    std::fs::write(&p, src)?;
    if let Some(m) = min {
        std::fs::write(dir.join(format!("{name}.min.rs")), m)?;
    }
    std::fs::write(dir.join(format!("{name}.txt")), format!("{} at {} (edition {edition}; replay with --edition {edition})\n{}\n", f.kind, f.place, f.detail))?;
    Ok(p)
}

/// Prints where a finding was saved, or that it could not be
fn report(what: &str, saved: std::io::Result<PathBuf>) {
    match saved {
        Ok(p) => eprintln!("zust-fuzz: {what}: {}", p.display()),
        Err(e) => eprintln!("zust-fuzz: {what}: the finding could not be saved: {e}"),
    }
}

/// The input that runs, as the watchdog sees it: its text, its edition and when its run started (0: none runs), in one
/// record under one lock, so the watchdog never pairs one input's start with another's text or edition
#[derive(Default)]
struct Running {
    src: String,
    edition: u16,
    started: u64,
}

/// The running input, shared with the watchdog; also in `current.rs` and `current.edition`, for the supervisor after
/// a crash
struct Active {
    running: Arc<Mutex<Running>>,
    current: PathBuf,
}

impl Active {
    fn set(&self, src: &str, edition: u16) {
        let _ = std::fs::write(&self.current, src);
        let _ = std::fs::write(self.current.with_extension("edition"), edition.to_string());
        if let Ok(mut r) = self.running.lock() {
            r.src.clear();
            r.src.push_str(src);
            r.edition = edition;
            r.started = 0;
        }
    }

    /// Runs one target on the input set last, under the watchdog
    fn run(&self, t: Target, src: &str, edition: u16) -> Result<String, Failure> {
        if let Ok(mut r) = self.running.lock() {
            r.started = now_ms();
        }
        let res = run_one(t, src, edition);
        if let Ok(mut r) = self.running.lock() {
            r.started = 0;
        }
        res
    }
}

/// In a worker each candidate runs as an input does, under the watchdog: one that hangs or crashes ends the worker as
/// an input would, after the finding was saved; and none runs past the worker's deadline (the input as it is then is
/// the minimized one). `minimize` on the command line runs them directly
fn minimize_failure(active: Option<(&Active, u64)>, t: Target, src: &str, f: &Failure, edition: u16) -> String {
    mutate::minimize(src, 3000, &mut |cand| {
        if active.is_some_and(|(_, deadline)| now_secs() >= deadline) {
            return false;
        }
        let r = match active.map(|a| a.0) {
            Some(a) => {
                a.set(cand, edition);
                a.run(t, cand, edition)
            }
            None => run_one(t, cand, edition),
        };
        r.err().is_some_and(|g| g.kind == f.kind && g.place == f.place)
    })
}

/// The edition of the input a worker ran last (`current.edition`), 2021 when it is not known
fn current_edition(out: &Path) -> u16 {
    std::fs::read_to_string(out.join("current.edition")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(2021)
}

/// The worker: mutates and runs inputs until the deadline. Exits 0 at the deadline, 3 after saving a hang.
fn worker(a: &Args, deadline: u64) -> ! {
    install_panic_hook();
    let mut corpus = load_corpus(&a.corpus);
    let n0 = corpus.len();
    let mut rng = rng::Rng::new(a.seed);
    let active = Active { running: Arc::new(Mutex::new(Running { edition: 2021, ..Running::default() })), current: a.out.join("current.rs") };
    {
        // (the watchdog: an input running past the timeout is saved as a hang, and the worker ends; the supervisor
        // starts another. The lock is held from the check to the exit, so the run cannot end or change meanwhile)
        let (running, out, timeout) = (active.running.clone(), a.out.clone(), a.timeout_ms);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let Ok(r) = running.lock() else { continue };
                if r.started != 0 && now_ms().saturating_sub(r.started) > timeout {
                    let f = Failure { kind: "hang", place: format!("{:016x}", hash(&r.src)), detail: format!("over {timeout} ms") };
                    report("hang", save_finding(&out, &f, &r.src, None, r.edition));
                    std::process::exit(3);
                }
            }
        });
    }
    // (one report per kind and place: the details (a mismatch's lengths, a panic's index) vary with the input)
    let mut seen: HashSet<(&'static str, String)> = HashSet::new();
    let mut features: HashSet<String> = HashSet::new();
    let (mut runs, mut last_report) = (0u64, Instant::now());
    while now_secs() < deadline {
        let i = rng.below(corpus.len());
        let j = rng.below(corpus.len());
        let src = mutate::mutate(&mut rng, &corpus[i], &corpus[j], a.max_len);
        let edition = *rng.pick(&[2015u16, 2018, 2021, 2021, 2024]);
        active.set(&src, edition);
        for &t in &a.targets {
            let r = active.run(t, &src, edition);
            runs += 1;
            match r {
                Ok(feat) => {
                    // (an answer not seen before: the input reached somewhere new, and mutants of it may go further)
                    if features.insert(format!("{}:{feat}", t.name())) && corpus.len() < n0 + 4096 {
                        corpus.push(src.clone());
                    }
                }
                Err(f) if seen.insert((f.kind, f.place.clone())) => {
                    let what = format!("{} at {} ({})", f.kind, f.place, f.detail.lines().next().unwrap_or(""));
                    // (saved before it is minimized, which a candidate that hangs or crashes ends)
                    report(&what, save_finding(&a.out, &f, &src, None, edition));
                    let min = minimize_failure(Some((&active, deadline)), t, &src, &f, edition);
                    if let Err(e) = save_finding(&a.out, &f, &src, Some(&min), edition) {
                        eprintln!("zust-fuzz: {what}: the minimized input could not be saved: {e}");
                    }
                    // (the input the next targets run on)
                    active.set(&src, edition);
                }
                Err(_) => {}
            }
        }
        if last_report.elapsed() > Duration::from_secs(10) {
            eprintln!("zust-fuzz: {runs} runs, corpus {}, {} findings", corpus.len(), seen.len());
            last_report = Instant::now();
        }
    }
    eprintln!("zust-fuzz: worker done: {runs} runs, corpus {}, {} findings", corpus.len(), seen.len());
    // (a mismatch of the two expanders is saved and printed, but fails the run only with --strict-diff: random
    // inputs are mostly invalid Rust, where the two may tokenize differently without either being wrong)
    let failing = seen.iter().any(|(k, _)| *k != "mismatch" || a.strict_diff);
    std::process::exit(if failing { 1 } else { 0 })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

/// The supervisor: runs workers until the time is up, restarting one that crashed or hung. Exit 1 when anything was
/// found.
fn supervise(a: &Args, raw: &[String]) -> i32 {
    let _ = std::fs::create_dir_all(&a.out);
    let deadline = now_secs() + a.time;
    let exe = std::env::current_exe().expect("current exe");
    let mut found = false;
    let mut seed = a.seed;
    eprintln!("zust-fuzz: targets {:?}, {} s, seed {seed}, out {}", a.targets.iter().map(|t| t.name()).collect::<Vec<_>>(), a.time, a.out.display());
    while now_secs() < deadline {
        let st = std::process::Command::new(&exe).arg("worker").args(raw).args(["--seed", &seed.to_string(), "--deadline", &deadline.to_string()]).status();
        match st.map(|s| (s.code(), s.success())) {
            Ok((_, true)) => {}
            Ok((Some(1), _)) | Ok((Some(3), _)) => found = true,
            Ok((code, _)) => {
                // (killed by a signal or an abort: a stack overflow, an allocation failure)
                found = true;
                let src = std::fs::read_to_string(a.out.join("current.rs")).unwrap_or_default();
                let f = Failure { kind: "crash", place: format!("{:016x}", hash(&src)), detail: format!("worker exit {code:?}") };
                report(&format!("crash ({code:?})"), save_finding(&a.out, &f, &src, None, current_edition(&a.out)));
            }
            Err(e) => {
                eprintln!("zust-fuzz: cannot start the worker: {e}");
                return 2;
            }
        }
        seed = seed.wrapping_add(1);
    }
    let _ = std::fs::remove_file(a.out.join("current.rs"));
    let _ = std::fs::remove_file(a.out.join("current.edition"));
    if found {
        eprintln!("zust-fuzz: findings in {}", a.out.join("findings").display());
        1
    } else {
        0
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().cloned() else {
        eprintln!("usage: zust-fuzz fuzz|replay|minimize [options] (see crates/fuzz/README.md)");
        std::process::exit(2);
    };
    let mut rest = argv[1..].to_vec();
    // (the worker's own arguments, which the supervisor appends)
    let mut deadline = 0;
    if let Some(k) = rest.iter().position(|s| s == "--deadline") {
        deadline = rest.get(k + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
        rest.drain(k..(k + 2).min(rest.len()));
    }
    let a = match parse_args(&rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("zust-fuzz: {e}");
            std::process::exit(2);
        }
    };
    match cmd.as_str() {
        "fuzz" => std::process::exit(supervise(&a, &rest)),
        "worker" => worker(&a, deadline),
        // (both expanders' outputs of each file, for a look at an expand-diff mismatch)
        "show-expand" => {
            for p in &a.files {
                let src = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                println!("== fast\n{:?}\n== reference\n{:?}", zust_core::macros::expand(&src), zust_core::macros::expand_reference(&src));
            }
        }
        "replay" | "minimize" => {
            install_panic_hook();
            let mut bad = 0;
            for p in &a.files {
                let src = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                for &t in &a.targets {
                    match run_one(t, &src, a.edition) {
                        Ok(f) => println!("{}: {}: ok ({f})", p.display(), t.name()),
                        Err(f) => {
                            bad += 1;
                            println!("{}: {}: {} at {}: {}", p.display(), t.name(), f.kind, f.place, f.detail);
                            if cmd == "minimize" {
                                println!("{}", minimize_failure(None, t, &src, &f, a.edition));
                            }
                        }
                    }
                }
            }
            std::process::exit(if bad > 0 { 1 } else { 0 })
        }
        _ => {
            eprintln!("zust-fuzz: unknown command {cmd}");
            std::process::exit(2)
        }
    }
}
