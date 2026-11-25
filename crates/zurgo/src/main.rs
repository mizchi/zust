//! The `zurgo` command: `build`, `run`, `test`, `metadata`, `clean` (cargo's flags for them).

use std::path::PathBuf;
use zurgo::compile::{self, BuildOpts};
use zurgo::platform::Target;
use zurgo::resolve::{self, Mode, Opts};

const USAGE: &str = "usage: zurgo <command> [options]

commands:
  build     build the package (or every member of a virtual workspace) with zustc
  run       build, then run a binary (arguments after `--` go to it)
  check     check every crate a build compiles, and report each one's errors (all the fns' bodies' errors of a
            crate, not only the first); writes no objects
  test      build the test crates (unit tests of the lib and bins, tests/*.rs) and run every test in a process
            of its own, all at once: `zurgo test [filter] [-- libtest args]`
  metadata  the package graph as JSON, as `cargo metadata --format-version 1` writes it
  clean     remove target/zust

options:
  --manifest-path <Cargo.toml>   the package (default: the nearest Cargo.toml)
  -r, --release                  optimize (zustc -O); output in target/zust/release
  -F, --features <a,b>           features of the package (`dep/feat` for a dependency's)
  --all-features, --no-default-features
  --target <triple>              x86_64-unknown-linux-gnu (default) or wasm32-unknown-unknown
  --bin <name>                   only this binary (build) / the binary to run (run)
  -j <n>                         parallel jobs
  -v                             print the zustc workspace (also written to target/zust/<profile>/zurgo-ws.txt)
  --build-script-compiler <c>    auto (zustc, else rustc; default), zustc or rustc
  --time                         zustc's timings
  --filter-platform <triple>     (metadata) only the dependencies of that platform
  --lib / --test <name>          (test) only the lib's unit tests / that integration test (also --bin <name>,
                                 --bins, --tests)
  --no-run                       (test) build the tests, do not run them
  --no-fail-fast / --fail-fast   (test) run every test (the default) / stop starting tests after a failure
  after `--` (test): <filter>, --exact, --skip <s>, --ignored, --include-ignored, --list, --test-threads <n>
    (--nocapture, --show-output are accepted: a failure's output is always shown)
  ZURGO_TEST_TIMEOUT=<secs> (300) stops a test, ZURGO_TEST_SLOW=<secs> (60) marks it slow
  --locked / --offline / --frozen are accepted: zurgo always uses Cargo.lock as it is and never downloads";

fn die(m: &str) -> ! {
    eprintln!("error: {m}");
    std::process::exit(1)
}

fn main() {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>().into_iter().peekable();
    let cmd = match args.next() {
        Some(c) if !c.starts_with('-') => c,
        Some(c) if c == "--version" || c == "-V" => {
            println!("zurgo {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2)
        }
    };
    let mut manifest: Option<PathBuf> = None;
    let mut o = BuildOpts::default();
    let mut filter: Option<String> = None;
    let mut rest: Vec<String> = vec![];
    // (`zurgo test`: the test name before `--`, which targets, --no-run, fail-fast)
    let mut test_filter: Vec<String> = vec![];
    let mut only: Vec<String> = vec![];
    let mut no_run = false;
    let mut fail_fast = false;
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().unwrap_or_else(|| die(&format!("`{name}` needs a value")));
        match a.as_str() {
            "--" => {
                rest.extend(args.by_ref());
                break;
            }
            "--manifest-path" => manifest = Some(val(&a).into()),
            "-r" | "--release" => o.release = true,
            "-F" | "--features" => o.features.extend(val(&a).split([',', ' ']).filter(|f| !f.is_empty()).map(str::to_string)),
            "--all-features" => o.all_features = true,
            "--no-default-features" => o.no_default = true,
            "--target" => o.target = Some(val(&a)),
            // (`zurgo test --bin x`: that bin's unit tests; the bins are all still built, for `CARGO_BIN_EXE_`)
            "--bin" if cmd == "test" || cmd == "t" => only.push(format!("(bin {})", val(&a))),
            "--bin" => o.bins.push(val(&a)),
            "-j" | "--jobs" => o.jobs = Some(val(&a).parse().unwrap_or_else(|_| die("`-j` needs a number"))),
            "-v" | "--verbose" => o.verbose = true,
            "--time" => o.time = true,
            "--build-script-compiler" => {
                o.bs_compiler = zurgo::build_script::Compiler::parse(&val(&a)).unwrap_or_else(|| die("`--build-script-compiler` is auto, zustc or rustc"))
            }
            "--no-cache" => o.no_cache = true,
            "--filter-platform" => filter = Some(val(&a)),
            "--format-version" => {
                if val(&a) != "1" {
                    die("only `--format-version 1` exists");
                }
            }
            "--locked" | "--offline" | "--frozen" | "-q" | "--quiet" => {}
            "--lib" if cmd == "test" || cmd == "t" => only.push("(lib)".into()),
            "--bins" if cmd == "test" || cmd == "t" => only.push("(bin ".into()),
            // (cargo's `--tests`: every target that has tests, unit tests too)
            "--tests" if cmd == "test" || cmd == "t" => only.push(String::new()),
            "--test" if cmd == "test" || cmd == "t" => only.push(format!("(test {})", val(&a))),
            "--no-run" if cmd == "test" || cmd == "t" => no_run = true,
            "--no-fail-fast" if cmd == "test" || cmd == "t" => fail_fast = false,
            "--fail-fast" if cmd == "test" || cmd == "t" => fail_fast = true,
            s if (cmd == "test" || cmd == "t") && !s.starts_with('-') => test_filter.push(s.to_string()),
            s if s.starts_with("--manifest-path=") => manifest = Some(s["--manifest-path=".len()..].into()),
            s if s.starts_with("--features=") => o.features.extend(s["--features=".len()..].split(',').filter(|f| !f.is_empty()).map(str::to_string)),
            s if s.starts_with("--target=") => o.target = Some(s["--target=".len()..].to_string()),
            s if s.starts_with("--filter-platform=") => filter = Some(s["--filter-platform=".len()..].to_string()),
            s if s.starts_with("-j") && s.len() > 2 => o.jobs = Some(s[2..].parse().unwrap_or_else(|_| die("`-j` needs a number"))),
            s => die(&format!("unknown option `{s}`\n\n{USAGE}")),
        }
    }
    let path = resolve::find_manifest(manifest.as_deref()).unwrap_or_else(|e| die(&e));
    let g = resolve::load(&path).unwrap_or_else(|e| die(&e));
    match cmd.as_str() {
        "build" | "b" => {
            let t0 = std::time::Instant::now();
            compile::build(&g, &o).unwrap_or_else(|e| die(&e));
            eprintln!("    Finished `{}` profile in {:.2}s (zustc)", if o.release { "release" } else { "dev" }, t0.elapsed().as_secs_f64());
        }
        "check" | "c" => {
            let t0 = std::time::Instant::now();
            let diags = compile::check(&g, &o).unwrap_or_else(|e| die(&e));
            let n: usize = diags.iter().map(|(_, d)| d.len()).sum();
            for (krate, ds) in &diags {
                for d in ds {
                    // (a program's type renamed against the prelude's: its own name, as zustc shows it)
                    eprintln!("error: crate `{krate}`: {}", d.replace("__u_", ""));
                }
            }
            if n > 0 {
                let bad = diags.iter().filter(|(_, d)| !d.is_empty()).count();
                eprintln!("error: {n} error{} in {bad} crate{}", if n == 1 { "" } else { "s" }, if bad == 1 { "" } else { "s" });
                std::process::exit(1);
            }
            eprintln!("    Finished checking {} crate{} in {:.2}s (zustc)", diags.len(), if diags.len() == 1 { "" } else { "s" }, t0.elapsed().as_secs_f64());
        }
        "run" | "r" => {
            // (without `--bin`: the `default-run` of the selected package, as cargo chooses)
            if o.bins.is_empty() {
                if let [p] = compile::selection(&g).as_slice() {
                    if let Some(d) = g.pkgs[*p].m.default_run.clone() {
                        o.bins.push(d);
                    }
                }
            }
            let built = compile::build(&g, &o).unwrap_or_else(|e| die(&e));
            let exe = match built.bins.as_slice() {
                [] => die("no binary to run"),
                [(_, e)] => e.clone(),
                many => die(&format!("several binaries ({}): choose one with `--bin`", many.iter().map(|b| b.0.as_str()).collect::<Vec<_>>().join(", "))),
            };
            // (a wasm32 module runs under the runner cargo's variable names, as `cargo run` runs it)
            let mut cmd = if exe.extension().is_some_and(|x| x == "wasm") {
                let var = "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER";
                let runner = std::env::var(var).unwrap_or_default();
                let mut words = runner.split_whitespace();
                let Some(prog) = words.next() else {
                    die(&format!("{} is a wasm32 module: set {var} to a program that runs it (`node tests/wasm/run.mjs` in this repository)", exe.display()))
                };
                let mut c = std::process::Command::new(prog);
                c.args(words).arg(&exe);
                c
            } else {
                std::process::Command::new(&exe)
            };
            let st = cmd.args(&rest).status().unwrap_or_else(|e| die(&format!("{}: {e}", exe.display())));
            std::process::exit(st.code().unwrap_or(101));
        }
        "test" | "t" => {
            o.test = true;
            o.test_targets = only.clone();
            let t0 = std::time::Instant::now();
            let built = compile::build(&g, &o).unwrap_or_else(|e| die(&e));
            eprintln!("    Finished `{}` profile in {:.2}s (zustc)", if o.release { "release" } else { "dev" }, t0.elapsed().as_secs_f64());
            // (the targets asked for: `--lib`, `--bins`, `--tests`, `--test x`)
            // (`--test x` and `--bin x` name targets that must exist)
            for w in only.iter().filter(|w| w.ends_with(')')) {
                if !built.tests.iter().any(|t| t.label.ends_with(w.as_str())) {
                    let (what, name) = w[1..w.len() - 1].split_once(' ').unwrap_or(("test", ""));
                    die(&format!("no {what} target named `{name}`"));
                }
            }
            let exes: Vec<_> = built.tests.iter().filter(|t| only.is_empty() || only.iter().any(|w| t.label.contains(w.as_str()))).cloned().collect();
            if no_run {
                for t in &exes {
                    eprintln!("  Executable {} ({})", t.label, t.exe.display());
                }
                return;
            }
            let mut ro = zurgo::test_runner::RunOpts { filters: test_filter, fail_fast, ..Default::default() };
            let secs = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok()).map(std::time::Duration::from_secs_f64);
            if let Some(t) = secs("ZURGO_TEST_TIMEOUT") {
                ro.timeout = t;
            }
            if let Some(t) = secs("ZURGO_TEST_SLOW") {
                ro.slow = t;
            }
            // (libtest's arguments, after `--`)
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--exact" => ro.exact = true,
                    "--ignored" => ro.ignored = true,
                    "--include-ignored" => ro.include_ignored = true,
                    "--list" => ro.list = true,
                    "--skip" => ro.skip.extend(it.next().cloned()),
                    "--test-threads" => ro.threads = it.next().and_then(|n| n.parse().ok()).unwrap_or_else(|| die("`--test-threads` needs a number")),
                    s if s.starts_with("--test-threads=") => ro.threads = s[15..].parse().unwrap_or_else(|_| die("`--test-threads` needs a number")),
                    "--nocapture" | "--show-output" | "-q" | "--quiet" => {}
                    "--format" | "--color" => {
                        it.next();
                    }
                    s if s.starts_with('-') => die(&format!("unknown test option `{s}`")),
                    s => ro.filters.push(s.to_string()),
                }
            }
            let sum = zurgo::test_runner::run(&exes, &ro, &built.out_dir.join("test-times")).unwrap_or_else(|e| die(&e));
            std::process::exit(if sum.ok() { 0 } else { 101 });
        }
        "metadata" => {
            let host = Target::host();
            let t = filter.as_deref().map(|t| Target::by_triple(t).unwrap_or_else(|| die(&format!("unknown platform `{t}`"))));
            let r = resolve::features(
                &g,
                &Opts {
                    mode: Mode::Unified,
                    dev: true,
                    target: t.as_ref(),
                    host: &host,
                    features: o.features.clone(),
                    all_features: o.all_features,
                    no_default: o.no_default,
                    only: None,
                },
            );
            println!("{}", zurgo::metadata::json(&g, &r, &compile::target_dir(&g), t.as_ref()));
        }
        "clean" => {
            let d = compile::target_dir(&g).join("zust");
            if d.exists() {
                std::fs::remove_dir_all(&d).unwrap_or_else(|e| die(&format!("{}: {e}", d.display())));
            }
        }
        c => die(&format!("unknown command `{c}`\n\n{USAGE}")),
    }
}
