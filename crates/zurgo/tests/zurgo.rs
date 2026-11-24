//! zurgo against cargo:
//! - `path_workspace`: a workspace of path packages (a lib and a bin of one name, features through `default`,
//!   `features = [..]` and `dep:`, a dev-dependency that must not be built) runs as its cargo build does;
//! - `registry_crates_run_like_cargo`: the cratebench crates from crates.io (fnv, adler2, hex, itoa, rustc-hash,
//!   cfg-if; thiserror, with its build script and a stub for its proc-macro crate), built by zurgo, print what their
//!   cargo builds print;
//! - `metadata_matches_cargo`: `zurgo metadata` and `cargo metadata` give the same graph (packages, targets, ids,
//!   enabled features, edges and their kinds) for those packages and for cratebench/ui's dioxus and leptos apps;
//! - `build_scripts_match_cargo`: build scripts (a build-dependency, `links` metadata as `DEP_*`, code written to
//!   `OUT_DIR`, `rustc-cfg`, `rustc-env`, cargo's variables) give what they give under cargo, whichever compiler builds
//!   them, and run again only when what they depend on changes;
//! - `proc_macros_run_like_cargo`: a proc-macro crate (attribute, derive, function-like) is built by rustc and run by
//!   the toolchain's proc-macro server, and its users print what their cargo builds print;
//! - `errors`: what M0 does not do yet is an error that says so (proc macros without the server among them).
//!
//! The registry tests need cargo and the network (`cargo fetch`), as the rest of zust's tests do.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../../tests/common/par.rs"]
mod par;

const ZURGO: &str = env!("CARGO_BIN_EXE_zurgo");

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// A target directory of the test's own (zurgo and cargo honor `CARGO_TARGET_DIR`)
fn tdir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("zurgo").join(name)
}

/// A directory of this invocation alone (two runs of the tests at once do not share it), removed when dropped
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("zurgo").join(format!("{name}-{}-{t}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
    fn write(&self, files: &[(&str, &str)]) {
        for (p, text) in files {
            let f = self.0.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(cmd: &mut Command) -> (bool, String, String) {
    let o = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn zurgo(args: &[&str], manifest: &Path, target_dir: &Path) -> (bool, String, String) {
    run(Command::new(ZURGO).args(args).arg("--manifest-path").arg(manifest).env("CARGO_TARGET_DIR", target_dir))
}

/// zurgo without the toolchain's proc-macro server (`ZUSTC_PROC_MACRO_SRV` empty): proc-macro crates are stubs
fn zurgo_no_pm(args: &[&str], manifest: &Path, target_dir: &Path) -> (bool, String, String) {
    run(Command::new(ZURGO).args(args).arg("--manifest-path").arg(manifest).env("CARGO_TARGET_DIR", target_dir).env("ZUSTC_PROC_MACRO_SRV", ""))
}

fn cargo(args: &[&str], manifest: &Path, target_dir: &Path) -> (bool, String, String) {
    run(Command::new("cargo").args(args).arg("--manifest-path").arg(manifest).env("CARGO_TARGET_DIR", target_dir))
}

#[test]
fn path_workspace() {
    let m = fixture("path-ws/Cargo.toml");
    let t = tdir("path-ws");
    for (args, want) in [(&[][..], "app HELLO, ZURGO\nno extra\n"), (&["--features", "with-extra"][..], "app HELLO, ZURGO\nextra: 42\n")] {
        let mut a = vec!["run"];
        a.extend_from_slice(args);
        let (ok, out, err) = zurgo(&a, &m, &t);
        assert!(ok, "zurgo {a:?}: {err}");
        assert_eq!(out, want, "zurgo {a:?}");
    }
    // (the same build again compiles and links nothing)
    let (ok, _, err) = zurgo(&["build"], &m, &t);
    assert!(ok, "{err}");
    let (ok, _, err) = zurgo(&["build", "--time"], &m, &t);
    assert!(ok, "{err}");
    assert!(err.contains("fresh") && !err.contains("fns compiled") && !err.contains("link "), "a no-op build did work:\n{err}");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let q = to.join(p.file_name().unwrap());
        if p.is_dir() {
            if p.file_name().unwrap() != "target" {
                copy_dir(&p, &q);
            }
        } else {
            std::fs::copy(&p, &q).unwrap();
        }
    }
}

#[test]
fn build_scripts_match_cargo() {
    let sc = Scratch::new("build-script");
    copy_dir(&fixture("build-script"), &sc.0);
    let m = sc.0.join("Cargo.toml");
    let (ok, want, err) = cargo(&["run", "-q", "--offline"], &m, &sc.0.join("cargo-target"));
    assert!(ok, "cargo run: {err}");
    // (rustc for every script; auto: zustc for gen's (it can), rustc for the app's (`env::vars()`))
    for c in ["rustc", "auto"] {
        let t = sc.0.join(format!("t-{c}"));
        let (ok, got, err) = zurgo(&["run", "--build-script-compiler", c], &m, &t);
        assert!(ok, "zurgo run ({c}): {err}");
        assert_eq!(got, want, "{c}");
        let compilers: Vec<String> = std::fs::read_dir(t.join("zust/debug/build"))
            .unwrap()
            .filter_map(|e| std::fs::read_to_string(e.unwrap().path().join("compiler")).ok())
            .map(|s| s.split(' ').next().unwrap().to_string())
            .collect();
        let mut compilers = compilers;
        compilers.sort();
        assert_eq!(compilers, if c == "rustc" { ["rustc", "rustc"] } else { ["rustc", "zustc"] }, "{c}");
    }
    // (again: no script runs; after an edit of build.rs (its rerun-if-changed) only the app's runs)
    let t = sc.0.join("t-auto");
    let (ok, _, err) = zurgo(&["build", "-v"], &m, &t);
    assert!(ok && !err.contains("Running build script"), "{err}");
    std::fs::write(sc.0.join("build.rs"), std::fs::read_to_string(sc.0.join("build.rs")).unwrap() + "\n// edited\n").unwrap();
    let (ok, _, err) = zurgo(&["build", "-v"], &m, &t);
    assert!(ok && err.contains("Running build script of `bsapp") && !err.contains("Running build script of `gen"), "{err}");
    // (a script that fails stops the build with its output)
    std::fs::write(sc.0.join("gen/build.rs"), "fn main() {\n    eprintln!(\"gen says no\");\n    std::process::exit(3);\n}\n").unwrap();
    let (ok, _, err) = zurgo(&["build"], &m, &t);
    assert!(!ok && err.contains("failed to run the build script of `gen v1.0.0`") && err.contains("gen says no"), "{err}");
}

/// The same programs built for wasm32 and run under the repository's sandbox host print what their native builds print:
/// the build scripts run on the host, the crates are one module
#[test]
fn proc_macros_run_like_cargo() {
    // (a proc-macro crate of the workspace: an attribute with arguments, used through a library that re-exports it; a
    //  derive with a helper attribute, whose output calls that library; function-like macros as an item and in an
    //  expression; doc comments as `#[doc]`; the expansion's environment; paths from the crate and from the library
    //  beside `macro_rules!` of their names, and in `stringify!`. rustc builds it, the toolchain's proc-macro server
    //  runs it)
    let sc = Scratch::new("proc-macro");
    copy_dir(&fixture("proc-macro"), &sc.0);
    let m = sc.0.join("Cargo.toml");
    let (ok, want, err) = cargo(&["run", "-q", "--offline"], &m, &sc.0.join("cargo-target"));
    assert!(ok, "cargo run: {err}");
    let t = sc.0.join("zurgo-target");
    let (ok, got, err) = zurgo(&["run"], &m, &t);
    assert!(ok, "zurgo run: {err}");
    assert_eq!(got, want);
    // (again: nothing rebuilt, the dylib and the expansions are kept)
    let (ok, _, err) = zurgo(&["build", "--time"], &m, &t);
    assert!(ok, "{err}");
    assert!(err.contains("fresh") && !err.contains("fns compiled"), "a no-op build did work:\n{err}");
    // (a change to the proc macro: rebuilt, and the crates that use it see its new output)
    let lib = sc.0.join("pm/src/lib.rs");
    let src = std::fs::read_to_string(&lib).unwrap();
    std::fs::write(&lib, src.replace("{n}() * {k}", "{n}() * {k} + 1")).unwrap();
    let (ok, got, err) = zurgo(&["run"], &m, &t);
    assert!(ok, "zurgo run: {err}");
    assert_eq!(got.lines().next(), Some("7 22"), "{got}");
    // (a file the proc macro includes from outside its package: rustc's dep-info has it)
    std::fs::write(sc.0.join("shared.txt"), "shared v2\n").unwrap();
    let (ok, got, err) = zurgo(&["run"], &m, &t);
    assert!(ok, "zurgo run: {err}");
    assert_eq!(got.lines().last(), Some("shared v2"), "{got}");
}

#[test]
fn wasm32_runs_like_native() {
    if !Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("note: no node: wasm32_runs_like_native skipped");
        return;
    }
    // (the runner is split on white space, as cargo splits it: a path relative to the repository's root, where zurgo
    // runs, so a checkout under a directory with a space works)
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let host = "node tests/wasm/run.mjs";
    let bs = Scratch::new("build-script-wasm");
    copy_dir(&fixture("build-script"), &bs.0);
    let mut cases: Vec<(String, std::path::PathBuf)> = vec![("path-ws".into(), fixture("path-ws/Cargo.toml")), ("build-script".into(), bs.0.join("Cargo.toml"))];
    for n in ["fnv", "adler2", "hex", "itoa", "rustc-hash", "cfg-if", "thiserror", "wasm-bindgen-shared"] {
        let m = fixture(&format!("{n}/Cargo.toml"));
        let (ok, _, err) = cargo(&["fetch", "--locked"], &m, &tdir(n));
        assert!(ok, "cargo fetch {n}: {err}");
        cases.push((n.to_string(), m));
    }
    // (the crates on every core: each builds twice and runs under node)
    let bad: Vec<String> = par::par_map(&cases, |(n, m)| -> Option<String> {
        let (ok, want, err) = zurgo(&["run"], m, &tdir(&format!("{n}-native")));
        assert!(ok, "zurgo run {n}: {err}");
        // (what depends on the target: the build script reads `CARGO_CFG_*` for wasm32; `cfg(unix)` is off there)
        let want = match n.as_str() {
            "build-script" => want.replace("cfg x86_64 linux 64", "cfg wasm32 unknown 32"),
            "cfg-if" => want.replace("unix64", "other"),
            _ => want,
        };
        let r = run(Command::new(ZURGO)
            .args(["run", "--target", "wasm32-unknown-unknown", "--manifest-path"])
            .arg(m)
            .env("CARGO_TARGET_DIR", tdir(&format!("{n}-wasm")))
            .env("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER", host)
            .current_dir(&root));
        (!r.0 || r.1 != want).then(|| format!("{n}: {}\n--- want\n{want}--- got\n{}", r.2, r.1))
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn registry_crates_run_like_cargo() {
    // (the crates on every core: each is built by cargo and by zurgo)
    let crates = ["fnv", "adler2", "hex", "itoa", "rustc-hash", "cfg-if", "thiserror", "wasm-bindgen-shared"];
    let bad: Vec<String> = par::par_map(&crates, |&n| -> Option<String> {
        let m = fixture(&format!("{n}/Cargo.toml"));
        let (ok, _, err) = cargo(&["fetch", "--locked"], &m, &tdir(n));
        assert!(ok, "cargo fetch {n}: {err}");
        let (ok, want, err) = cargo(&["run", "--locked", "-q"], &m, &tdir(&format!("{n}-cargo")));
        assert!(ok, "cargo run {n}: {err}");
        let (ok, got, err) = zurgo(&["run"], &m, &tdir(n));
        if !ok {
            Some(format!("{n}: zurgo run failed: {err}"))
        } else if got != want {
            Some(format!("{n}: output differs\n--- zurgo\n{got}--- cargo\n{want}"))
        } else {
            None
        }
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

/// (packages and resolve nodes, keyed by `name@version`, in a form to compare)
fn graph(json: &str) -> Vec<String> {
    let m: serde_json::Value = serde_json::from_str(json).unwrap();
    let nv = |id: &str| -> String {
        let p = m["packages"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap();
        format!("{}@{}", p["name"].as_str().unwrap(), p["version"].as_str().unwrap())
    };
    let mut out = vec![];
    for p in m["packages"].as_array().unwrap() {
        let mut ts: Vec<String> = p["targets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["kind"].as_array().unwrap().iter().all(|k| matches!(k.as_str(), Some("lib" | "rlib" | "proc-macro" | "bin" | "cdylib" | "staticlib" | "dylib" | "custom-build" | "test"))))
            .map(|t| format!("{} {} {} {}", t["kind"], t["name"], t["src_path"], t["edition"]))
            .collect();
        ts.sort();
        out.push(format!("package {} id={} targets={ts:?}", nv(p["id"].as_str().unwrap()), p["id"]));
    }
    for n in m["resolve"]["nodes"].as_array().unwrap() {
        let mut deps: Vec<String> = n["deps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                let mut k: Vec<String> = d["dep_kinds"].as_array().unwrap().iter().map(|k| format!("{}/{}", k["kind"], k["target"])).collect();
                k.sort();
                format!("{} as {} {k:?}", nv(d["pkg"].as_str().unwrap()), d["name"])
            })
            .collect();
        deps.sort();
        let mut fs: Vec<String> = n["features"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
        fs.sort();
        out.push(format!("node {} features={fs:?} deps={deps:?}", nv(n["id"].as_str().unwrap())));
    }
    out.sort();
    out
}

#[test]
fn metadata_matches_cargo() {
    let ui = repo().join("cratebench/ui/apps");
    let mut manifests: Vec<PathBuf> = ["path-ws", "fnv", "hex", "cfg-if", "test-pkg", "test-flags"].iter().map(|n| fixture(n).join("Cargo.toml")).collect();
    manifests.push(ui.join("dioxus/Cargo.toml"));
    manifests.push(ui.join("leptos/Cargo.toml"));
    let mut bad = vec![];
    for m in &manifests {
        for plat in [None, Some("wasm32-unknown-unknown"), Some("x86_64-unknown-linux-gnu")] {
            let mut a = vec!["metadata", "--format-version", "1", "--locked"];
            if let Some(p) = plat {
                a.extend(["--filter-platform", p]);
            }
            // (cargo first: it fetches what zurgo then reads)
            let (ok, want, err) = cargo(&a, m, &tdir("metadata"));
            assert!(ok, "cargo metadata {}: {err}", m.display());
            let (ok, got, err) = zurgo(&a, m, &tdir("metadata"));
            assert!(ok, "zurgo metadata {}: {err}", m.display());
            let (w, g) = (graph(&want), graph(&got));
            if w != g {
                let only_c: Vec<&String> = w.iter().filter(|x| !g.contains(x)).take(4).collect();
                let only_z: Vec<&String> = g.iter().filter(|x| !w.contains(x)).take(4).collect();
                bad.push(format!("{} {plat:?}:\n  cargo only: {only_c:#?}\n  zurgo only: {only_z:#?}", m.display()));
            }
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

#[test]
fn errors() {
    let sc = Scratch::new("errors");
    let d = sc.0.clone();
    sc.write(&[
        ("p/Cargo.toml", "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n[dependencies]\npm = { path = \"../pm\" }\n"),
        ("p/src/main.rs", "fn main() {}\n"),
        ("pm/Cargo.toml", "[package]\nname = \"pm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\nproc-macro = true\n"),
        (
            "pm/src/lib.rs",
            "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_attribute]\npub fn traced(_a: TokenStream, i: TokenStream) -> TokenStream {\n    i\n}\n#[proc_macro]\npub fn html(i: TokenStream) -> TokenStream {\n    i\n}\n",
        ),
    ]);
    let m = d.join("p/Cargo.toml");
    // (no Cargo.lock: zurgo has no resolver yet)
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(!ok && err.contains("cargo generate-lockfile"), "{err}");
    let (ok, _, err) = cargo(&["generate-lockfile", "--offline"], &m, &d.join("t"));
    assert!(ok, "{err}");
    // (without the toolchain's proc-macro server, a proc-macro dependency is a stub: unused, the build goes on; so does
    //  a use in a file the crate does not reach, under a cfg that is off (before or after the attribute), or in a
    //  macro_rules! body nothing calls)
    let zurgo = zurgo_no_pm;
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(ok, "{err}");
    sc.write(&[
        ("p/src/unused.rs", "#[pm::traced]\nfn f() {}\n"),
        (
            "p/src/main.rs",
            "#[cfg(any())]\n#[pm::traced]\nfn hidden() {}\n#[pm::traced]\n#[cfg(any())]\nfn hidden2() {}\nmacro_rules! unused {\n    () => {\n        #[pm::traced]\n        fn t() {}\n    };\n}\nfn main() {}\n",
        ),
    ]);
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(ok, "{err}");
    // (under another name: `use pm::traced as trace;`)
    sc.write(&[("p/src/main.rs", "use pm::traced as trace;\n#[trace]\nfn main() {}\n")]);
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(!ok && err.contains("the attribute `#[trace]` of the proc-macro crate `pm v0.1.0`"), "{err}");
    // (its attribute used: an error that names mizchi/zust#29, not an attribute dropped)
    sc.write(&[("p/src/main.rs", "#[pm::traced]\nfn main() {}\n")]);
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(!ok && err.contains("the attribute `#[traced]` of the proc-macro crate `pm v0.1.0`") && err.contains("#29"), "{err}");
    // (a function-like macro: the stub's compile_error!)
    sc.write(&[("p/src/main.rs", "fn main() {\n    pm::html!(<p>);\n}\n")]);
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(!ok && err.contains("`html!` is a proc macro of `pm`, which cannot run yet (mizchi/zust#29)"), "{err}");
    // (but metadata works)
    let (ok, out, err) = zurgo(&["metadata"], &m, &d.join("t"));
    assert!(ok && out.contains("\"proc-macro\""), "{err}");
    // (with the server, the proc macros run: `traced` gives the item back, `html!` its input, which is not Rust)
    let zurgo = self::zurgo;
    sc.write(&[("p/src/main.rs", "#[pm::traced]\nfn main() {}\n")]);
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(ok, "{err}");
    sc.write(&[("p/src/main.rs", "fn main() {\n    pm::html!(<p>);\n}\n")]);
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(!ok && !err.contains("#29"), "{err}");
    // (a target name that is a path: it would put the executable outside the target directory)
    sc.write(&[
        ("q/Cargo.toml", "[package]\nname = \"q\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n[[bin]]\nname = \"../escape\"\npath = \"src/main.rs\"\n"),
        ("q/Cargo.lock", "version = 3\n\n[[package]]\nname = \"q\"\nversion = \"0.1.0\"\n"),
        ("q/src/main.rs", "fn main() {}\n"),
    ]);
    let (ok, _, err) = zurgo(&["build"], &d.join("q/Cargo.toml"), &d.join("t"));
    assert!(!ok && err.contains("binary name `../escape` is not allowed"), "{err}");
    assert!(!d.join("t/zust/escape").exists() && !d.join("t/escape").exists());
    // (a git dependency is not matched by crates.io's package of the same name and version)
    let crates_io = "registry+https://github.com/rust-lang/crates.io-index";
    sc.write(&[
        ("g/Cargo.toml", "[package]\nname = \"g\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n[dependencies]\nfnv = { git = \"https://example.com/fnv\" }\n"),
        (
            "g/Cargo.lock",
            format!("version = 3\n\n[[package]]\nname = \"fnv\"\nversion = \"1.0.7\"\nsource = \"{crates_io}\"\n\n[[package]]\nname = \"g\"\nversion = \"0.1.0\"\ndependencies = [\"fnv\"]\n").as_str(),
        ),
        ("g/src/main.rs", "fn main() {}\n"),
    ]);
    let (ok, _, err) = zurgo(&["metadata"], &d.join("g/Cargo.toml"), &d.join("t"));
    assert!(!ok && err.contains("Cargo.lock does not resolve `fnv`"), "{err}");
}

/// Workspace rules, against cargo: members from a glob and an explicit entry (one each), a path dependency inside the
/// workspace that becomes a member without being listed, `default-members`, fields inherited by a member read
/// through `--manifest-path`, `autolib = false`, `required-features`, `default-run`, a lib and a bin of one name, and a
/// feature another member turns on that must not reach a build of `app` alone
#[test]
fn workspace_rules_match_cargo() {
    let sc = Scratch::new("ws-rules");
    sc.write(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"crates/util\"]\ndefault-members = [\"crates/app\"]\nresolver = \"2\"\n[workspace.package]\nversion = \"0.3.0\"\nedition = \"2021\"\n",
        ),
        (
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\nversion.workspace = true\nedition.workspace = true\ndefault-run = \"app\"\n[dependencies]\nutil = { path = \"../util\" }\nhelper = { path = \"../../helper\" }\n[features]\nextra = []\n[[bin]]\nname = \"app\"\npath = \"src/main.rs\"\n[[bin]]\nname = \"extra\"\npath = \"src/extra.rs\"\nrequired-features = [\"extra\"]\n[[bin]]\nname = \"loudbin\"\npath = \"src/loud.rs\"\nrequired-features = [\"util/loud\"]\n",
        ),
        ("crates/app/src/lib.rs", "pub fn hi() -> String {\n    format!(\"{} {}\", util::word(), helper::n())\n}\n"),
        ("crates/app/src/main.rs", "fn main() {\n    println!(\"app: {}\", app::hi());\n}\n"),
        ("crates/app/src/extra.rs", "fn main() {\n    println!(\"extra: {}\", app::hi());\n}\n"),
        ("crates/app/src/loud.rs", "fn main() {\n    println!(\"loud: {}\", app::hi());\n}\n"),
        ("crates/util/Cargo.toml", "[package]\nname = \"util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[features]\nloud = []\n"),
        ("crates/util/src/lib.rs", "pub fn word() -> &'static str {\n    if cfg!(feature = \"loud\") { \"LOUD\" } else { \"quiet\" }\n}\n"),
        ("crates/tools/Cargo.toml", "[package]\nname = \"tools\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautolib = false\n[dependencies]\nutil = { path = \"../util\", features = [\"loud\"] }\n"),
        ("crates/tools/src/lib.rs", "compile_error!(\"autolib = false: not a library\");\n"),
        ("crates/tools/src/main.rs", "fn main() {\n    println!(\"tools: {}\", util::word());\n}\n"),
        ("helper/Cargo.toml", "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("helper/src/lib.rs", "pub fn n() -> i32 {\n    7\n}\n"),
    ]);
    let root = sc.0.join("Cargo.toml");
    let t = sc.0.join("target");
    let (ok, _, err) = cargo(&["generate-lockfile", "--offline"], &root, &t);
    assert!(ok, "{err}");
    // (metadata from the root and from a member: the graph and the member lists are cargo's)
    // (also with features asked for: a member's feature, and a dependency's, at the virtual root)
    let runs: [(&PathBuf, &[&str]); 4] = [(&root, &[]), (&sc.0.join("crates/app/Cargo.toml"), &[]), (&root, &["--features", "extra"]), (&root, &["--features", "util/loud"])];
    for (m, more) in runs {
        let m = m.clone();
        let mut a = vec!["metadata", "--format-version", "1", "--offline"];
        a.extend(more);
        let (ok, want, err) = cargo(&a, &m, &t);
        assert!(ok, "cargo metadata: {err}");
        let (ok, got, err) = zurgo(&a, &m, &t);
        assert!(ok, "zurgo metadata: {err}");
        assert_eq!(graph(&got), graph(&want), "{} {more:?}", m.display());
        let members = |j: &str, k: &str| -> Vec<String> {
            let v: serde_json::Value = serde_json::from_str(j).unwrap();
            let mut x: Vec<String> = v[k].as_array().unwrap().iter().map(|i| i.as_str().unwrap().to_string()).collect();
            x.sort();
            x
        };
        for k in ["workspace_members", "workspace_default_members"] {
            assert_eq!(members(&got, k), members(&want, k), "{k} of {}", m.display());
        }
    }
    // (app alone: `default-run`, and util without the feature that tools turns on)
    let app = sc.0.join("crates/app/Cargo.toml");
    let (ok, out, err) = zurgo(&["run"], &app, &t);
    assert!(ok, "{err}");
    assert_eq!(out, "app: quiet 7\n");
    // (`required-features`: left out of a default build, an error when asked for by name, built when on)
    let (ok, _, err) = zurgo(&["build", "--bin", "extra"], &app, &t);
    assert!(!ok && err.contains("required-features"), "{err}");
    let (ok, out, err) = zurgo(&["run", "--bin", "extra", "--features", "extra"], &app, &t);
    assert!(ok, "{err}");
    assert_eq!(out, "extra: quiet 7\n");
    // (from the virtual root: the default member is selected, and a bare `--features` is its feature)
    let (ok, out, err) = zurgo(&["run", "--bin", "extra", "--features", "extra"], &root, &t);
    assert!(ok, "{err}");
    assert_eq!(out, "extra: quiet 7\n");
    // (`required-features` of a dependency's feature: `util/loud` is checked on util)
    let (ok, _, err) = zurgo(&["build", "--bin", "loudbin"], &app, &t);
    assert!(!ok && err.contains("util/loud"), "{err}");
    let (ok, out, err) = zurgo(&["run", "--bin", "loudbin", "--features", "util/loud"], &app, &t);
    assert!(ok, "{err}");
    assert_eq!(out, "loud: LOUD 7\n");
    // (tools: `autolib = false` builds no library from its src/lib.rs, and its util is loud)
    let (ok, out, err) = zurgo(&["run"], &sc.0.join("crates/tools/Cargo.toml"), &t);
    assert!(ok, "{err}");
    assert_eq!(out, "tools: LOUD\n");
}

/// The root manifest of a workspace with a package of its own and `default-members` as a glob: commands at the root
/// select the default members (not the root package), as cargo does
#[test]
fn root_package_and_default_members_glob() {
    let sc = Scratch::new("ws-root");
    sc.write(&[
        ("Cargo.toml", "[package]\nname = \"rootpkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\nmembers = [\"crates/*\"]\ndefault-members = [\"crates/*\"]\n"),
        ("src/main.rs", "fn main() {\n    println!(\"root\");\n}\n"),
        ("crates/a/Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nb = { path = \"../b\" }\n"),
        ("crates/a/src/main.rs", "fn main() {\n    println!(\"a {}\", b::n());\n}\n"),
        ("crates/b/Cargo.toml", "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ("crates/b/src/lib.rs", "pub fn n() -> i32 {\n    2\n}\n"),
    ]);
    let root = sc.0.join("Cargo.toml");
    let t = sc.0.join("target");
    let (ok, _, err) = cargo(&["generate-lockfile", "--offline"], &root, &t);
    assert!(ok, "{err}");
    let a = ["metadata", "--format-version", "1", "--offline"];
    let (ok, want, err) = cargo(&a, &root, &t);
    assert!(ok, "{err}");
    let (ok, got, err) = zurgo(&a, &root, &t);
    assert!(ok, "{err}");
    let dm = |j: &str| -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(j).unwrap();
        let mut x: Vec<String> = v["workspace_default_members"].as_array().unwrap().iter().map(|i| i.as_str().unwrap().to_string()).collect();
        x.sort();
        x
    };
    assert_eq!(dm(&got), dm(&want));
    let (ok, out, err) = zurgo(&["run"], &root, &t);
    assert!(ok, "{err}");
    assert_eq!(out, "a 2\n");
    assert!(!t.join("zust/debug/rootpkg").exists(), "the root package was built");
}

/// `cargo test`'s outcome of each test: name → "ok", "FAILED" or "ignored" (the fixture's names are unique across
/// its test binaries)
fn cargo_outcomes(out: &str) -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    for l in out.lines() {
        let Some(r) = l.strip_prefix("test ") else { continue };
        let Some((name, res)) = r.split_once(" ... ") else { continue };
        let name = name.trim_end_matches(" - should panic").to_string();
        m.insert(name, res.trim().to_string());
    }
    m
}

/// `zurgo test`'s outcome of each test that ran: name → "ok" / "FAILED" (its lines `PASS [..] (k/n) label name`)
fn zurgo_outcomes(err: &str) -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    for l in err.lines() {
        let t = l.trim_start();
        let pass = t.starts_with("PASS [");
        if !(pass || t.starts_with("FAIL (") || t.starts_with("TIMEOUT [") || t.starts_with("SIGNAL [")) || !t.contains("] (") {
            continue;
        }
        let name = t.rsplit(' ').next().unwrap().to_string();
        m.insert(name, if pass { "ok" } else { "FAILED" }.to_string());
    }
    m
}

/// `zurgo test` runs the same tests as `cargo test`, with the same outcomes: the lib's unit tests (with a
/// dev-dependency, `#[should_panic]` right and wrong, a `Result` test, two ignored), a bin's, and an integration test
/// against the lib and the bin (`CARGO_BIN_EXE_`), from the package's directory. A failing test fails the run (101)
#[test]
fn test_runs_like_cargo_test() {
    let m = fixture("test-pkg/Cargo.toml");
    let (ok, out, _) = cargo(&["test", "--no-fail-fast", "--offline"], &m, &tdir("test-pkg-cargo"));
    assert!(!ok, "the fixture has failing tests");
    let want = cargo_outcomes(&out);
    assert_eq!(want.len(), 13, "{want:?}");
    let o = Command::new(ZURGO).args(["test", "--manifest-path"]).arg(&m).env("CARGO_TARGET_DIR", tdir("test-pkg")).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(101), "{err}");
    let got = zurgo_outcomes(&err);
    let ran: std::collections::BTreeMap<String, String> = want.iter().filter(|(_, v)| *v != "ignored").map(|(k, v)| (k.clone(), v.clone())).collect();
    assert_eq!(got, ran, "\n{err}");
    // (the ignored ones are listed as such)
    let (ok, listed, _) = run(Command::new(ZURGO).args(["test", "--manifest-path"]).arg(&m).args(["--", "--list"]).env("CARGO_TARGET_DIR", tdir("test-pkg")));
    assert!(ok, "{listed}");
    for (n, _) in want.iter().filter(|(_, v)| *v == "ignored") {
        assert!(listed.contains(&format!("tp (lib): {n} (ignored)")), "{n}:\n{listed}");
    }
    // (the times, for the next run's order)
    let times = std::fs::read_to_string(tdir("test-pkg").join("zust/debug/test-times")).unwrap();
    assert!(times.contains("tp (lib)\ttests::adds\t"), "{times}");
    // (a filter, and one target)
    let (ok, _, err) = zurgo(&["test", "--lib", "adds"], &m, &tdir("test-pkg"));
    assert!(ok, "{err}");
    assert_eq!(zurgo_outcomes(&err).into_keys().collect::<Vec<_>>(), ["tests::adds"], "{err}");
    // (cargo's `--tests`: the unit tests too)
    let (_, _, err) = zurgo(&["test", "--tests", "adds"], &m, &tdir("test-pkg"));
    assert_eq!(zurgo_outcomes(&err).into_keys().collect::<Vec<_>>(), ["tests::adds"], "{err}");
    // (`--bin x`: that bin's unit tests alone)
    let (ok, _, err) = zurgo(&["test", "--bin", "tp"], &m, &tdir("test-pkg"));
    assert!(ok, "{err}");
    assert_eq!(zurgo_outcomes(&err).into_keys().collect::<Vec<_>>(), ["greets"], "{err}");
    let (ok, _, err) = zurgo(&["test", "--bin", "nope"], &m, &tdir("test-pkg"));
    assert!(!ok && err.contains("no bin target named `nope`"), "{err}");
    // (`--list --ignored`: the ignored ones alone)
    let (ok, listed, _) = run(Command::new(ZURGO).args(["test", "--lib", "--manifest-path"]).arg(&m).args(["--", "--list", "--ignored"]).env("CARGO_TARGET_DIR", tdir("test-pkg")));
    assert!(ok);
    assert!(listed.lines().all(|l| l.ends_with("(ignored)")) && listed.lines().count() == 3, "{listed}");
    // (a test target that does not exist is an error, not a run of nothing)
    let (ok, _, err) = zurgo(&["test", "--test", "nope"], &m, &tdir("test-pkg"));
    assert!(!ok && err.contains("no test target named `nope`"), "{err}");
}

/// A test that never ends is stopped at `ZURGO_TEST_TIMEOUT`, and fails
#[test]
fn test_timeout() {
    let m = fixture("test-pkg/Cargo.toml");
    let o = Command::new(ZURGO)
        .args(["test", "--lib", "--manifest-path"])
        .arg(&m)
        .args(["--", "--ignored", "--exact", "tests::hangs"])
        .env("CARGO_TARGET_DIR", tdir("test-pkg-timeout"))
        .env("ZURGO_TEST_TIMEOUT", "1")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(101), "{err}");
    assert!(err.contains("TIMEOUT [") && err.contains("tests::hangs"), "{err}");
    // (a test that ends but leaves a process holding its output: it passes, the process is stopped, and the run
    //  does not wait for it)
    let t0 = std::time::Instant::now();
    let o = Command::new(ZURGO)
        .args(["test", "--lib", "--manifest-path"])
        .arg(&m)
        .args(["--", "--ignored", "--exact", "tests::leaves_a_child"])
        .env("CARGO_TARGET_DIR", tdir("test-pkg-timeout"))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(t0.elapsed() < std::time::Duration::from_secs(20), "{err}");
}

/// The runner speaks libtest's command line: it runs the executables `cargo test` builds as well, with the outcomes
/// cargo's own run gives (libtest checks `#[should_panic]` itself)
#[test]
fn runner_drives_libtest_executables() {
    let m = fixture("test-pkg/Cargo.toml");
    let td = tdir("test-pkg-libtest");
    let (ok, out, err) = cargo(&["test", "--no-run", "--offline", "--message-format=json"], &m, &td);
    assert!(ok, "{err}");
    let mut exes: Vec<zurgo::test_runner::TestExe> = vec![];
    for l in out.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        if v["reason"] == "compiler-artifact" && v["profile"]["test"] == true {
            if let Some(e) = v["executable"].as_str() {
                exes.push(zurgo::test_runner::TestExe::new(v["target"]["name"].as_str().unwrap(), e, fixture("test-pkg")));
            }
        }
    }
    assert_eq!(exes.len(), 3, "{out}");
    let s = zurgo::test_runner::run(&exes, &zurgo::test_runner::RunOpts::default(), &td.join("test-times")).unwrap();
    let got: std::collections::BTreeMap<String, String> = s.results.iter().map(|r| (r.1.clone(), if r.2 { "ok" } else { "FAILED" }.to_string())).collect();
    let (_, out, _) = cargo(&["test", "--no-fail-fast", "--offline"], &m, &td);
    let want: std::collections::BTreeMap<String, String> = cargo_outcomes(&out).into_iter().filter(|(_, v)| v != "ignored").collect();
    assert_eq!(got, want);
    assert_eq!(s.skipped, 3);
}

/// `test = false` leaves a target's tests out; a `harness = false` target is one test, its own `main`, that passes when
/// it exits with 0 (as `cargo test` runs them). `--test x` builds that test crate alone
#[test]
fn test_flags_like_cargo() {
    let m = fixture("test-flags/Cargo.toml");
    let (ok, out, err) = cargo(&["test", "--offline"], &m, &tdir("test-flags-cargo"));
    assert!(ok, "{out}{err}");
    assert!(out.contains("custom ran") || err.contains("custom ran"), "{out}{err}");
    assert!(!out.contains("never") && !err.contains("this test target is off"), "{out}{err}");
    let t = tdir("test-flags");
    let (ok, _, err) = zurgo(&["test"], &m, &t);
    assert!(ok, "{err}");
    assert_eq!(zurgo_outcomes(&err).into_iter().collect::<Vec<_>>(), [("custom".to_string(), "ok".to_string()), ("uses_lib".to_string(), "ok".to_string())], "{err}");
    // (`--test normal`: that test crate alone is built)
    let t2 = tdir("test-flags-one");
    let _ = std::fs::remove_dir_all(&t2);
    let (ok, _, err) = zurgo(&["test", "--test", "normal"], &m, &t2);
    assert!(ok, "{err}");
    assert_eq!(zurgo_outcomes(&err).into_keys().collect::<Vec<_>>(), ["uses_lib"], "{err}");
    assert!(!t2.join("zust/debug/custom_test_test.test").exists() && t2.join("zust/debug/normal_test_test.test").exists());
    // (`--test off` names the `test = false` target: it runs, and fails, as under cargo)
    let (ok, _, err) = zurgo(&["test", "--test", "off"], &m, &t);
    assert!(!ok, "{err}");
    assert_eq!(zurgo_outcomes(&err).into_iter().collect::<Vec<_>>(), [("never".to_string(), "FAILED".to_string())], "{err}");
    let (ok, out, _) = cargo(&["test", "--offline", "--test", "off"], &m, &tdir("test-flags-cargo"));
    assert!(!ok && out.contains("test never ... FAILED"), "{out}");
}

/// `zurgo test --target wasm32-unknown-unknown`: each test in a module of its own, under cargo's runner variable;
/// `#[should_panic]` is the runner's to check there (a module starts no child process). The outcomes are `cargo test`'s
/// on the host
#[test]
fn test_wasm32_like_cargo_test() {
    if !Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success()) {
        eprintln!("note: no node: test_wasm32_like_cargo_test skipped");
        return;
    }
    let m = fixture("test-wasm/Cargo.toml");
    let (ok, out, _) = cargo(&["test", "--no-fail-fast", "--offline"], &m, &tdir("test-wasm-cargo"));
    assert!(!ok);
    let want: std::collections::BTreeMap<String, String> = cargo_outcomes(&out).into_iter().filter(|(_, v)| v != "ignored").collect();
    assert_eq!(want.len(), 6, "{want:?}");
    let host = format!("node {}", repo().join("tests/wasm/run.mjs").display());
    let o = Command::new(ZURGO)
        .args(["test", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(&m)
        .env("CARGO_TARGET_DIR", tdir("test-wasm"))
        .env("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER", host)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(101), "{err}");
    assert_eq!(zurgo_outcomes(&err), want, "\n{err}");
}
