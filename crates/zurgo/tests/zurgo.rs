//! zurgo against cargo:
//! - `path_workspace`: a workspace of path packages (a lib and a bin of one name, features through `default`,
//!   `features = [..]` and `dep:`, a dev-dependency that must not be built) runs as its cargo build does;
//! - `registry_crates_run_like_cargo`: the cratebench crates from crates.io (fnv, adler2, hex, itoa, rustc-hash,
//!   cfg-if), built by zurgo, print what their cargo builds print;
//! - `metadata_matches_cargo`: `zurgo metadata` and `cargo metadata` give the same graph (packages, targets, ids,
//!   enabled features, edges and their kinds) for those packages and for cratebench/ui's dioxus and leptos apps;
//! - `errors`: what M0 does not do yet is an error that says so.
//!
//! The registry tests need cargo and the network (`cargo fetch`), as the rest of zust's tests do.

use std::path::{Path, PathBuf};
use std::process::Command;

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

#[test]
fn registry_crates_run_like_cargo() {
    let mut bad = vec![];
    for n in ["fnv", "adler2", "hex", "itoa", "rustc-hash", "cfg-if"] {
        let m = fixture(&format!("{n}/Cargo.toml"));
        let (ok, _, err) = cargo(&["fetch", "--locked"], &m, &tdir(n));
        assert!(ok, "cargo fetch {n}: {err}");
        let (ok, want, err) = cargo(&["run", "--locked", "-q"], &m, &tdir(&format!("{n}-cargo")));
        assert!(ok, "cargo run {n}: {err}");
        let (ok, got, err) = zurgo(&["run"], &m, &tdir(n));
        if !ok {
            bad.push(format!("{n}: zurgo run failed: {err}"));
        } else if got != want {
            bad.push(format!("{n}: output differs\n--- zurgo\n{got}--- cargo\n{want}"));
        }
    }
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
            .filter(|t| t["kind"].as_array().unwrap().iter().all(|k| matches!(k.as_str(), Some("lib" | "rlib" | "proc-macro" | "bin" | "cdylib" | "staticlib" | "dylib" | "custom-build"))))
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
    let mut manifests: Vec<PathBuf> = ["path-ws", "fnv", "hex", "cfg-if"].iter().map(|n| fixture(n).join("Cargo.toml")).collect();
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
        ("pm/src/lib.rs", ""),
    ]);
    let m = d.join("p/Cargo.toml");
    // (no Cargo.lock: zurgo has no resolver yet)
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(!ok && err.contains("cargo generate-lockfile"), "{err}");
    let (ok, _, err) = cargo(&["generate-lockfile", "--offline"], &m, &d.join("t"));
    assert!(ok, "{err}");
    // (a proc-macro dependency names mizchi/zust#29)
    let (ok, _, err) = zurgo(&["build"], &m, &d.join("t"));
    assert!(!ok && err.contains("proc-macro crate `pm`") && err.contains("#29"), "{err}");
    // (but metadata works)
    let (ok, out, err) = zurgo(&["metadata"], &m, &d.join("t"));
    assert!(ok && out.contains("\"proc-macro\""), "{err}");
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
            "[package]\nname = \"app\"\nversion.workspace = true\nedition.workspace = true\ndefault-run = \"app\"\n[dependencies]\nutil = { path = \"../util\" }\nhelper = { path = \"../../helper\" }\n[features]\nextra = []\n[[bin]]\nname = \"app\"\npath = \"src/main.rs\"\n[[bin]]\nname = \"extra\"\npath = \"src/extra.rs\"\nrequired-features = [\"extra\"]\n",
        ),
        ("crates/app/src/lib.rs", "pub fn hi() -> String {\n    format!(\"{} {}\", util::word(), helper::n())\n}\n"),
        ("crates/app/src/main.rs", "fn main() {\n    println!(\"app: {}\", app::hi());\n}\n"),
        ("crates/app/src/extra.rs", "fn main() {\n    println!(\"extra: {}\", app::hi());\n}\n"),
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
    for m in [root.clone(), sc.0.join("crates/app/Cargo.toml")] {
        let a = ["metadata", "--format-version", "1", "--offline"];
        let (ok, want, err) = cargo(&a, &m, &t);
        assert!(ok, "cargo metadata: {err}");
        let (ok, got, err) = zurgo(&a, &m, &t);
        assert!(ok, "zurgo metadata: {err}");
        assert_eq!(graph(&got), graph(&want), "{}", m.display());
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
    // (tools: `autolib = false` builds no library from its src/lib.rs, and its util is loud)
    let (ok, out, err) = zurgo(&["run"], &sc.0.join("crates/tools/Cargo.toml"), &t);
    assert!(ok, "{err}");
    assert_eq!(out, "tools: LOUD\n");
}
