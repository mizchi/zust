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
use std::sync::atomic::{AtomicU64, Ordering};
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

fn save_finding(out: &Path, f: &Failure, src: &str, min: Option<&str>, edition: u16) -> PathBuf {
    let dir = out.join("findings");
    let _ = std::fs::create_dir_all(&dir);
    let name = format!("{}-{:016x}", f.kind, hash(&f.place));
    let p = dir.join(format!("{name}.rs"));
    let _ = std::fs::write(&p, src);
    if let Some(m) = min {
        let _ = std::fs::write(dir.join(format!("{name}.min.rs")), m);
    }
    let _ = std::fs::write(dir.join(format!("{name}.txt")), format!("{} at {} (edition {edition}; replay with --edition {edition})\n{}\n", f.kind, f.place, f.detail));
    p
}

fn minimize_failure(t: Target, src: &str, f: &Failure, edition: u16) -> String {
    mutate::minimize(src, 3000, &mut |cand| run_one(t, cand, edition).err().is_some_and(|g| g.kind == f.kind && g.place == f.place))
}

/// The worker: mutates and runs inputs until the deadline. Exits 0 at the deadline, 3 after saving a hang.
fn worker(a: &Args, deadline: u64) -> ! {
    install_panic_hook();
    let mut corpus = load_corpus(&a.corpus);
    let n0 = corpus.len();
    let mut rng = rng::Rng::new(a.seed);
    let current = a.out.join("current.rs");
    let started = Arc::new(AtomicU64::new(0));
    let input = Arc::new(Mutex::new(String::new()));
    {
        // (the watchdog: an input running past the timeout is saved as a hang, and the worker ends; the supervisor
        // starts another)
        let (started, input, out, timeout) = (started.clone(), input.clone(), a.out.clone(), a.timeout_ms);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let s = started.load(Ordering::Relaxed);
                if s != 0 && now_ms().saturating_sub(s) > timeout {
                    let src = input.lock().map(|s| s.clone()).unwrap_or_default();
                    let f = Failure { kind: "hang", place: format!("{:016x}", hash(&src)), detail: format!("over {timeout} ms") };
                    let p = save_finding(&out, &f, &src, None, 0);
                    eprintln!("zust-fuzz: hang: {}", p.display());
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
        let _ = std::fs::write(&current, &src);
        if let Ok(mut s) = input.lock() {
            s.clone_from(&src);
        }
        for &t in &a.targets {
            started.store(now_ms(), Ordering::Relaxed);
            let r = run_one(t, &src, edition);
            started.store(0, Ordering::Relaxed);
            runs += 1;
            match r {
                Ok(feat) => {
                    // (an answer not seen before: the input reached somewhere new, and mutants of it may go further)
                    if features.insert(format!("{}:{feat}", t.name())) && corpus.len() < n0 + 4096 {
                        corpus.push(src.clone());
                    }
                }
                Err(f) if seen.insert((f.kind, f.place.clone())) => {
                    let min = minimize_failure(t, &src, &f, edition);
                    let p = save_finding(&a.out, &f, &src, Some(&min), edition);
                    eprintln!("zust-fuzz: {} at {} ({}): {}", f.kind, f.place, f.detail.lines().next().unwrap_or(""), p.display());
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
                let p = save_finding(&a.out, &f, &src, None, 0);
                eprintln!("zust-fuzz: crash ({code:?}): {}", p.display());
            }
            Err(e) => {
                eprintln!("zust-fuzz: cannot start the worker: {e}");
                return 2;
            }
        }
        seed = seed.wrapping_add(1);
    }
    let _ = std::fs::remove_file(a.out.join("current.rs"));
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
                                println!("{}", minimize_failure(t, &src, &f, a.edition));
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
