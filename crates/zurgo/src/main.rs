//! The `zurgo` command: `build`, `run`, `metadata`, `clean` (cargo's flags for them).

use std::path::PathBuf;
use zurgo::compile::{self, BuildOpts};
use zurgo::platform::Target;
use zurgo::resolve::{self, Mode, Opts};

const USAGE: &str = "usage: zurgo <command> [options]

commands:
  build     build the package (or every member of a virtual workspace) with zustc
  run       build, then run a binary (arguments after `--` go to it)
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
  --time                         zustc's timings
  --filter-platform <triple>     (metadata) only the dependencies of that platform
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
            "--bin" => o.bins.push(val(&a)),
            "-j" | "--jobs" => o.jobs = Some(val(&a).parse().unwrap_or_else(|_| die("`-j` needs a number"))),
            "-v" | "--verbose" => o.verbose = true,
            "--time" => o.time = true,
            "--no-cache" => o.no_cache = true,
            "--filter-platform" => filter = Some(val(&a)),
            "--format-version" => {
                if val(&a) != "1" {
                    die("only `--format-version 1` exists");
                }
            }
            "--locked" | "--offline" | "--frozen" | "-q" | "--quiet" => {}
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
            let st = std::process::Command::new(&exe).args(&rest).status().unwrap_or_else(|e| die(&format!("{}: {e}", exe.display())));
            std::process::exit(st.code().unwrap_or(101));
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
