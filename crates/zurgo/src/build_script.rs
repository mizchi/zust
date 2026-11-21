//! Build scripts, as cargo runs them (mizchi/zust#28, M1):
//!
//! - **Compile.** The script is a host binary with the package's build-dependencies. zustc builds it when it can (the
//!   host libraries and the script as one zustc workspace). Otherwise rustc does: the host libraries as rlibs (proc
//!   macros as dylibs), then the script. `auto` tries zustc first.
//! - **Run** in the package's directory, with the variables cargo sets (`OUT_DIR`, `TARGET`, `HOST`, `PROFILE`,
//!   `CARGO_PKG_*`, `CARGO_FEATURE_*`, `CARGO_CFG_*`, `RUSTC`, `DEP_<links>_<key>`, ...). `RUSTC` is the real rustc,
//!   so a script's probes and version checks see what they see under cargo.
//! - **Read** its output, in both `cargo:` and `cargo::` forms: `rustc-cfg` and `rustc-env` go to the package's
//!   crates, `warning` is printed, `error` fails the build, `metadata` (and an unknown `cargo:key`) goes to the
//!   dependents' scripts as `DEP_<links>_<key>`, `rerun-if-changed` / `rerun-if-env-changed` decide when it runs
//!   again. Link directives are kept but zustc does not apply them yet.
//! - **Cache.** The output is kept in `target/zust/<profile>/build/<package>-<hash>/`, with the inputs it depends on;
//!   the script runs again only when one of them (or the script) changes. Without `rerun-if-*` lines, as cargo does,
//!   any file of the package counts.

use crate::manifest::{DepKind, Package};
use crate::metadata::extern_name;
use crate::platform::Target;
use crate::resolve::{Graph, Node, Resolved};
use crate::toml;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use zustc::driver2::{CrateSpec, Options2, Session2};

type R<T> = Result<T, String>;

/// Which compiler builds build scripts (`--build-script-compiler`)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Compiler {
    /// zustc, and rustc when zustc cannot
    #[default]
    Auto,
    Zustc,
    Rustc,
}

impl Compiler {
    pub fn parse(s: &str) -> Option<Compiler> {
        match s {
            "auto" => Some(Compiler::Auto),
            "zustc" => Some(Compiler::Zustc),
            "rustc" => Some(Compiler::Rustc),
            _ => None,
        }
    }
}

/// What a build script gave
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub out_dir: PathBuf,
    pub cfgs: Vec<(String, Option<String>)>,
    pub env: Vec<(String, String)>,
    /// `metadata` (`DEP_<links>_<key>` for the dependents' scripts)
    pub metadata: Vec<(String, String)>,
    /// `rustc-link-lib`, `rustc-link-search`, `rustc-link-arg`, `rustc-flags`: not applied yet
    pub link: Vec<String>,
}

pub struct Runner<'a> {
    pub g: &'a Graph,
    pub r: &'a Resolved,
    pub target: &'a Target,
    pub host: &'a Target,
    /// target/zust/[triple/]profile
    pub out: PathBuf,
    pub release: bool,
    pub compiler: Compiler,
    pub jobs: Option<usize>,
    pub verbose: bool,
    /// the target directory: not part of a package's files, also when it is inside the package
    skip: PathBuf,
    done: BTreeMap<Node, Option<Output>>,
}

impl<'a> Runner<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(g: &'a Graph, r: &'a Resolved, target: &'a Target, host: &'a Target, out: PathBuf, release: bool, compiler: Compiler, jobs: Option<usize>, verbose: bool) -> Self {
        let skip = crate::compile::target_dir(g);
        Runner { g, r, target, host, out, release, compiler, jobs, verbose, skip, done: BTreeMap::new() }
    }

    /// The output of the build script of node n (None: the package has none). Its dependencies' scripts run first
    pub fn run(&mut self, n: Node) -> R<Option<Output>> {
        if let Some(o) = self.done.get(&n) {
            return Ok(o.clone());
        }
        let g = self.g;
        let pk = &g.pkgs[n.0].m;
        let Some(script) = pk.build.clone() else {
            self.done.insert(n, None);
            return Ok(None);
        };
        // (the scripts of the normal dependencies first: their `links` metadata is this script's `DEP_*`)
        let mut dep_env: Vec<(String, String)> = vec![];
        for (_, t) in self.edges(n, DepKind::Normal) {
            if let Some(o) = self.run(t)? {
                if let Some(l) = &g.pkgs[t.0].m.links {
                    let l = l.to_uppercase().replace('-', "_");
                    for (k, v) in &o.metadata {
                        dep_env.push((format!("DEP_{l}_{}", k.to_uppercase().replace('-', "_")), v.clone()));
                    }
                }
            }
        }
        let side = if n.1 { self.host } else { self.target };
        let feats: Vec<String> = self.r.nodes[&n].features.iter().cloned().collect();
        let h = hash(&[&g.pkgs[n.0].id, &feats.join(","), &side.triple, if self.release { "release" } else { "debug" }]);
        let dir = self.out.join("build").join(format!("{}-{h}", pk.name));
        let out_dir = dir.join("out");
        mkdir(&out_dir)?;
        let exe = self.compile(n, &script, &dir)?;
        // (the environment: cargo's variables)
        let mut env = pkg_env(pk);
        env.push(("CARGO_MANIFEST_DIR".into(), pk.dir.display().to_string()));
        env.push(("CARGO_MANIFEST_PATH".into(), pk.manifest_path.display().to_string()));
        if let Some(l) = &pk.links {
            env.push(("CARGO_MANIFEST_LINKS".into(), l.clone()));
        }
        for f in &feats {
            env.push((format!("CARGO_FEATURE_{}", f.to_uppercase().replace('-', "_")), "1".into()));
        }
        let mut keys: Vec<&str> = side.cfg.iter().map(|(k, _)| k.as_str()).collect();
        keys.dedup();
        for k in keys {
            if k == "debug_assertions" && self.release {
                continue;
            }
            env.push((format!("CARGO_CFG_{}", k.to_uppercase()), side.values(k).join(",")));
        }
        env.push(("OUT_DIR".into(), out_dir.display().to_string()));
        env.push(("TARGET".into(), side.triple.clone()));
        env.push(("HOST".into(), self.host.triple.clone()));
        env.push(("NUM_JOBS".into(), self.jobs.unwrap_or_else(|| std::thread::available_parallelism().map(|p| p.get()).unwrap_or(1)).to_string()));
        env.push(("OPT_LEVEL".into(), if self.release { "3" } else { "0" }.into()));
        env.push(("DEBUG".into(), (!self.release).to_string()));
        env.push(("PROFILE".into(), if self.release { "release" } else { "debug" }.into()));
        env.push(("RUSTC".into(), rustc()));
        env.push(("RUSTDOC".into(), std::env::var("RUSTDOC").unwrap_or_else(|_| "rustdoc".into())));
        env.push(("CARGO".into(), std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())));
        env.push(("CARGO_ENCODED_RUSTFLAGS".into(), String::new()));
        env.extend(dep_env);
        // (the last run, if nothing it depends on changed)
        let stamp_path = dir.join("stamp");
        let output_path = dir.join("output");
        let exe_fp = file_fp(&exe);
        let env_fp = hash(&env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>());
        if let (Ok(stamp), Ok(text)) = (std::fs::read_to_string(&stamp_path), std::fs::read_to_string(&output_path)) {
            if stamp_fresh(&stamp, &exe_fp, &env_fp, &pk.dir, &self.skip) {
                let o = parse(&text, pk, &out_dir, false)?;
                self.done.insert(n, Some(o.clone()));
                return Ok(Some(o));
            }
        }
        if self.verbose {
            eprintln!("     Running build script of `{} v{}`", pk.name, pk.version);
        }
        let res = Command::new(&exe).current_dir(&pk.dir).envs(env.iter().map(|(k, v)| (k, v))).output().map_err(|e| format!("{}: {e}", exe.display()))?;
        let stdout = String::from_utf8_lossy(&res.stdout).into_owned();
        let _ = std::fs::write(dir.join("stderr"), &res.stderr);
        if !res.status.success() {
            return Err(format!(
                "failed to run the build script of `{} v{}` ({})\n--- stdout\n{}\n--- stderr\n{}",
                pk.name,
                pk.version,
                res.status,
                tail(&stdout),
                tail(&String::from_utf8_lossy(&res.stderr))
            ));
        }
        let o = parse(&stdout, pk, &out_dir, true)?;
        std::fs::write(&output_path, &stdout).map_err(|e| format!("{}: {e}", output_path.display()))?;
        std::fs::write(&stamp_path, stamp_text(&stdout, &exe_fp, &env_fp, &pk.dir, &self.skip)).map_err(|e| format!("{}: {e}", stamp_path.display()))?;
        self.done.insert(n, Some(o.clone()));
        Ok(Some(o))
    }

    /// The active edges of node n of one kind: (the declaration's index, the node)
    fn edges(&self, n: Node, kind: DepKind) -> Vec<(usize, Node)> {
        self.r.nodes[&n].edges.iter().filter(|&&(ei, _)| self.g.decl(n.0, &self.g.edges[n.0][ei]).kind == kind).copied().collect()
    }

    /// The host libraries the build script of n needs, dependencies first
    fn host_closure(&self, n: Node) -> Vec<Node> {
        fn visit(s: &Runner, m: Node, out: &mut Vec<Node>) {
            if out.contains(&m) {
                return;
            }
            for (_, t) in s.edges(m, DepKind::Normal) {
                visit(s, t, out);
            }
            out.push(m);
        }
        let mut out = vec![];
        for (_, t) in self.edges(n, DepKind::Build) {
            visit(self, t, &mut out);
        }
        out
    }

    /// The build script's executable: compiled by zustc or rustc (as `self.compiler` says)
    fn compile(&mut self, n: Node, script: &Path, dir: &Path) -> R<PathBuf> {
        let closure = self.host_closure(n);
        // (their own build scripts first)
        let mut outs: BTreeMap<Node, Option<Output>> = BTreeMap::new();
        for &m in &closure {
            let o = self.run(m)?;
            outs.insert(m, o);
        }
        let g = self.g;
        let pk = &g.pkgs[n.0].m;
        // (auto: the compiler that built it last time, while the sources it is built from are the same)
        let srcs = hash(&std::iter::once(n).chain(closure.iter().copied()).map(|m| src_fp(&g.pkgs[m.0].m.dir, &self.skip)).collect::<Vec<_>>().iter().map(String::as_str).collect::<Vec<_>>());
        let memo = dir.join("compiler");
        let last_rustc = self.compiler == Compiler::Auto && std::fs::read_to_string(&memo).is_ok_and(|t| t == format!("rustc {srcs}"));
        let zustc_err = if self.compiler == Compiler::Rustc || last_rustc {
            None
        } else {
            match self.compile_zustc(n, script, dir, &closure, &outs) {
                Ok(exe) => {
                    let _ = std::fs::write(&memo, format!("zustc {srcs}"));
                    return Ok(exe);
                }
                Err(e) if self.compiler == Compiler::Zustc => return Err(format!("the build script of `{} v{}`: {e}", pk.name, pk.version)),
                Err(e) => Some(e),
            }
        };
        if let Some(e) = &zustc_err {
            if self.verbose {
                eprintln!("note: the build script of `{} v{}` is compiled with rustc (zustc: {})", pk.name, pk.version, e.lines().next().unwrap_or(""));
            }
        }
        let exe = self.compile_rustc(n, script, dir, &closure, &outs).map_err(|e| match zustc_err {
            Some(z) => format!("{e}\n(zustc could not compile it either: {})", z.lines().next().unwrap_or("")),
            None => e,
        })?;
        let _ = std::fs::write(&memo, format!("rustc {srcs}"));
        Ok(exe)
    }

    /// The extern names of the build-dependencies of n (or the normal ones of a host library m), with their nodes
    fn externs(&self, n: Node, kind: DepKind) -> Vec<(String, Node)> {
        let mut v: Vec<(String, Node)> = vec![];
        for (ei, t) in self.edges(n, kind) {
            let d = self.g.decl(n.0, &self.g.edges[n.0][ei]);
            let to = &self.g.pkgs[t.0].m;
            if to.lib.is_none() {
                continue;
            }
            let e = extern_name(d, to);
            if !v.iter().any(|(x, _)| *x == e) {
                v.push((e, t));
            }
        }
        v
    }

    fn compile_zustc(&self, n: Node, script: &Path, dir: &Path, closure: &[Node], outs: &BTreeMap<Node, Option<Output>>) -> R<PathBuf> {
        let mut crates: Vec<CrateSpec> = vec![];
        for &m in closure {
            let p = &self.g.pkgs[m.0].m;
            let lib = p.lib.as_ref().ok_or_else(|| format!("`{}` has no library", p.name))?;
            if lib.is_proc_macro() {
                return Err(format!("its build-dependency `{}` is a proc-macro crate", p.name));
            }
            if crates.iter().any(|c| c.name == lib.name) {
                return Err(format!("two versions of `{}` among its build-dependencies", p.name));
            }
            let mut deps = vec![];
            for (e, t) in self.externs(m, DepKind::Normal) {
                if Some(&e) != self.g.pkgs[t.0].m.lib.as_ref().map(|l| &l.name) {
                    return Err(format!("`{}` renames a dependency", p.name));
                }
                deps.push(e);
            }
            let (cfgs, env) = crate_env(p, &lib.name, outs.get(&m).cloned().flatten().as_ref());
            crates.push(CrateSpec {
                name: lib.name.clone(),
                root: lib.path.clone(),
                display: lib.path.display().to_string(),
                lib: true,
                deps,
                features: self.r.nodes[&m].features.iter().cloned().collect(),
                exe: None,
                cfgs,
                env,
            });
        }
        let pk = &self.g.pkgs[n.0].m;
        let mut deps = vec![];
        for (e, t) in self.externs(n, DepKind::Build) {
            if Some(&e) != self.g.pkgs[t.0].m.lib.as_ref().map(|l| &l.name) {
                return Err(format!("it renames its build-dependency `{}`", self.g.pkgs[t.0].m.name));
            }
            deps.push(e);
        }
        let (cfgs, mut env) = crate_env(pk, "build_script_build", None);
        env.retain(|(k, _)| k != "OUT_DIR");
        crates.push(CrateSpec {
            name: "build_script_build".into(),
            root: script.to_path_buf(),
            display: script.display().to_string(),
            lib: false,
            deps,
            features: vec![],
            exe: Some("build-script-build".into()),
            cfgs,
            env,
        });
        let zd = dir.join("zust");
        let mut opts = Options2 { backend: Some(zustc::clif_backend::compile), ..Options2::default() };
        if let Some(j) = self.jobs {
            opts.jobs = j;
        }
        Session2::new(opts).build_workspace(&crates, &zd)?;
        Ok(zd.join("build-script-build"))
    }

    fn compile_rustc(&self, n: Node, script: &Path, dir: &Path, closure: &[Node], outs: &BTreeMap<Node, Option<Output>>) -> R<PathBuf> {
        let deps_dir = self.out.join("build").join("deps");
        mkdir(&deps_dir)?;
        // (a host library as an rlib (a proc macro as a dylib), named by a hash of what it is built from)
        let mut art: BTreeMap<Node, PathBuf> = BTreeMap::new();
        for &m in closure {
            let p = &self.g.pkgs[m.0].m;
            let lib = p.lib.as_ref().ok_or_else(|| format!("`{}` has no library", p.name))?;
            let feats: Vec<String> = self.r.nodes[&m].features.iter().cloned().collect();
            let o = outs.get(&m).cloned().flatten();
            let externs: Vec<(String, PathBuf)> = self.externs(m, DepKind::Normal).into_iter().filter_map(|(e, t)| art.get(&t).map(|a| (e, a.clone()))).collect();
            let h = hash(&[&self.g.pkgs[m.0].id, &feats.join(","), &format!("{externs:?}"), &format!("{:?}", o.as_ref().map(|o| (&o.cfgs, &o.env)))]);
            let pm = lib.is_proc_macro();
            let file = deps_dir.join(format!("lib{}-{h}.{}", lib.name, if pm { "so" } else { "rlib" }));
            let fresh = file.is_file() && (self.g.pkgs[m.0].source.is_some() || src_fp(&p.dir, &self.skip) == std::fs::read_to_string(file.with_extension("fp")).unwrap_or_default());
            if !fresh {
                let mut c = Command::new(rustc());
                c.args(["--crate-name", &lib.name, "--edition", &lib.edition, "--crate-type", if pm { "proc-macro" } else { "lib" }, "--emit=link", "--cap-lints", "allow"]);
                c.args(["-C", "opt-level=0", "-C", &format!("metadata={h}"), "-C", &format!("extra-filename=-{h}"), "--out-dir"]).arg(&deps_dir);
                c.arg("-L").arg(format!("dependency={}", deps_dir.display()));
                for f in &feats {
                    c.args(["--cfg", &format!("feature=\"{f}\"")]);
                }
                if pm {
                    c.args(["--extern", "proc_macro"]);
                }
                for (e, a) in &externs {
                    c.arg("--extern").arg(format!("{e}={}", a.display()));
                }
                let (cfgs, env) = crate_env(p, &lib.name, o.as_ref());
                for (k, v) in &cfgs {
                    c.args(["--cfg", &match v {
                        Some(v) => format!("{k}=\"{v}\""),
                        None => k.clone(),
                    }]);
                }
                c.envs(env.iter().map(|(k, v)| (k, v))).current_dir(&p.dir).arg(&lib.path);
                run_rustc(c, &format!("`{} v{}`", p.name, p.version))?;
                let _ = std::fs::write(file.with_extension("fp"), src_fp(&p.dir, &self.skip));
            }
            art.insert(m, file);
        }
        let pk = &self.g.pkgs[n.0].m;
        let rd = dir.join("rustc");
        mkdir(&rd)?;
        let exe = rd.join("build-script-build");
        let mut c = Command::new(rustc());
        c.args(["--crate-name", "build_script_build", "--edition", &pk.edition, "--crate-type", "bin", "--emit=link", "--cap-lints", "allow", "-C", "opt-level=0", "-o"]).arg(&exe);
        c.arg("-L").arg(format!("dependency={}", deps_dir.display()));
        for (e, t) in self.externs(n, DepKind::Build) {
            if let Some(a) = art.get(&t) {
                c.arg("--extern").arg(format!("{e}={}", a.display()));
            }
        }
        let (_, mut env) = crate_env(pk, "build_script_build", None);
        env.retain(|(k, _)| k != "OUT_DIR");
        c.envs(env.iter().map(|(k, v)| (k, v))).current_dir(&pk.dir).arg(script);
        // (not again while the command and the package's files are the same)
        let key = hash(&[&format!("{c:?}"), &src_fp(&pk.dir, &self.skip)]);
        let kp = rd.join("key");
        if exe.is_file() && std::fs::read_to_string(&kp).is_ok_and(|k| k == key) {
            return Ok(exe);
        }
        run_rustc(c, &format!("the build script of `{} v{}`", pk.name, pk.version))?;
        let _ = std::fs::write(&kp, key);
        Ok(exe)
    }
}

/// `CARGO_PKG_*` of a package (what cargo gives rustc and build scripts)
pub fn pkg_env(pk: &Package) -> Vec<(String, String)> {
    let s = |k: &str| toml::get_str(&pk.meta, &[k]).unwrap_or("").to_string();
    let (core, pre) = pk.version.split_once('-').unwrap_or((&pk.version, ""));
    let core = core.split_once('+').map(|x| x.0).unwrap_or(core);
    let mut parts = core.split('.');
    let mut env: Vec<(String, String)> = vec![
        ("CARGO_PKG_NAME".into(), pk.name.clone()),
        ("CARGO_PKG_VERSION".into(), pk.version.clone()),
        ("CARGO_PKG_VERSION_MAJOR".into(), parts.next().unwrap_or("0").into()),
        ("CARGO_PKG_VERSION_MINOR".into(), parts.next().unwrap_or("0").into()),
        ("CARGO_PKG_VERSION_PATCH".into(), parts.next().unwrap_or("0").into()),
        ("CARGO_PKG_VERSION_PRE".into(), pre.split_once('+').map(|x| x.0).unwrap_or(pre).into()),
        ("CARGO_PKG_AUTHORS".into(), pk.meta.get("authors").map(|v| v.str_list().join(":")).unwrap_or_default()),
        ("CARGO_PKG_DESCRIPTION".into(), s("description")),
        ("CARGO_PKG_HOMEPAGE".into(), s("homepage")),
        ("CARGO_PKG_REPOSITORY".into(), s("repository")),
        ("CARGO_PKG_LICENSE".into(), s("license")),
        ("CARGO_PKG_LICENSE_FILE".into(), s("license-file")),
        ("CARGO_PKG_RUST_VERSION".into(), s("rust-version")),
    ];
    let readme = match pk.meta.get("readme") {
        Some(toml::Value::String(r)) => r.clone(),
        _ if pk.dir.join("README.md").is_file() => "README.md".into(),
        _ => String::new(),
    };
    env.push(("CARGO_PKG_README".into(), readme));
    env
}

/// The cfgs and environment of one of a package's crates (lib or bin): cargo's variables, and what its build script
/// gave (`rustc-cfg`, `rustc-env`, `OUT_DIR`)
pub fn crate_env(pk: &Package, crate_name: &str, bs: Option<&Output>) -> (Vec<(String, Option<String>)>, Vec<(String, String)>) {
    let mut env = pkg_env(pk);
    env.push(("CARGO_MANIFEST_DIR".into(), pk.dir.display().to_string()));
    env.push(("CARGO_MANIFEST_PATH".into(), pk.manifest_path.display().to_string()));
    env.push(("CARGO_CRATE_NAME".into(), crate_name.replace('-', "_")));
    let mut cfgs = vec![];
    if let Some(o) = bs {
        env.push(("OUT_DIR".into(), o.out_dir.display().to_string()));
        env.extend(o.env.iter().cloned());
        cfgs = o.cfgs.clone();
    }
    (cfgs, env)
}

/// A build script's stdout. `fresh`: it just ran (warnings are printed then)
fn parse(text: &str, pk: &Package, out_dir: &Path, fresh: bool) -> R<Output> {
    let mut o = Output { out_dir: out_dir.to_path_buf(), ..Default::default() };
    let mut errors = vec![];
    for line in text.lines() {
        let (key, val, new) = if let Some(r) = line.strip_prefix("cargo::") {
            match r.split_once('=') {
                Some((k, v)) => (k, v, true),
                None => continue,
            }
        } else if let Some(r) = line.strip_prefix("cargo:") {
            match r.split_once('=') {
                Some((k, v)) => (k, v, false),
                None => continue,
            }
        } else {
            continue;
        };
        match key {
            "rustc-cfg" => o.cfgs.push(match val.split_once('=') {
                Some((k, v)) => (k.trim().to_string(), Some(v.trim().trim_matches('"').to_string())),
                None => (val.trim().to_string(), None),
            }),
            "rustc-env" => {
                if let Some((k, v)) = val.split_once('=') {
                    o.env.push((k.to_string(), v.to_string()));
                }
            }
            "warning" => {
                if fresh {
                    eprintln!("warning: {}@{}: {val}", pk.name, pk.version);
                }
            }
            "error" => errors.push(val.to_string()),
            "rerun-if-changed" | "rerun-if-env-changed" | "rustc-check-cfg" => {}
            "rustc-link-lib" | "rustc-link-search" | "rustc-flags" | "rustc-cdylib-link-arg" => o.link.push(format!("{key}={val}")),
            k if k.starts_with("rustc-link-arg") => o.link.push(format!("{key}={val}")),
            "metadata" if new => {
                if let Some((k, v)) = val.split_once('=') {
                    o.metadata.push((k.to_string(), v.to_string()));
                }
            }
            // (the old form: any other key is metadata)
            k if !new => o.metadata.push((k.to_string(), val.to_string())),
            k => return Err(format!("the build script of `{} v{}` printed an unknown instruction `cargo::{k}`", pk.name, pk.version)),
        }
    }
    if !errors.is_empty() {
        return Err(format!("the build script of `{} v{}` failed:\n{}", pk.name, pk.version, errors.join("\n")));
    }
    if fresh && !o.link.is_empty() {
        eprintln!("warning: {}@{}: link instructions of its build script are not applied yet: {}", pk.name, pk.version, o.link.join(", "));
    }
    Ok(o)
}

/// What a run depends on: the script, its environment, and the files (and variables) its `rerun-if-*` lines name;
/// without them, every file of the package. One line each: `exe <fp>`, `env <fp>`, `file <path> <fp>`, `var <name>
/// <value>`
fn stamp_text(stdout: &str, exe_fp: &str, env_fp: &str, pkg_dir: &Path, skip: &Path) -> String {
    let mut s = format!("exe {exe_fp}\nenv {env_fp}\n");
    let mut files: Vec<PathBuf> = vec![];
    let mut vars: Vec<String> = vec![];
    for line in stdout.lines() {
        let r = line.strip_prefix("cargo::").or_else(|| line.strip_prefix("cargo:"));
        match r.and_then(|r| r.split_once('=')) {
            Some(("rerun-if-changed", p)) => files.push(pkg_dir.join(p)),
            Some(("rerun-if-env-changed", v)) => vars.push(v.to_string()),
            _ => {}
        }
    }
    if files.is_empty() && vars.is_empty() {
        s.push_str(&format!("all {}\n", src_fp(pkg_dir, skip)));
    }
    for f in files {
        s.push_str(&format!("file {}\t{}\n", f.display(), path_fp(&f)));
    }
    for v in vars {
        s.push_str(&format!("var {v}\t{}\n", std::env::var(&v).unwrap_or_else(|_| "\u{0}unset".into())));
    }
    s
}

fn stamp_fresh(stamp: &str, exe_fp: &str, env_fp: &str, pkg_dir: &Path, skip: &Path) -> bool {
    let mut lines = stamp.lines();
    if lines.next() != Some(&format!("exe {exe_fp}")) || lines.next() != Some(&format!("env {env_fp}")) {
        return false;
    }
    lines.all(|l| {
        if let Some(fp) = l.strip_prefix("all ") {
            return fp == src_fp(pkg_dir, skip);
        }
        if let Some((p, fp)) = l.strip_prefix("file ").and_then(|r| r.split_once('\t')) {
            return path_fp(Path::new(p)) == fp;
        }
        if let Some((v, val)) = l.strip_prefix("var ").and_then(|r| r.split_once('\t')) {
            return std::env::var(v).unwrap_or_else(|_| "\u{0}unset".into()) == val;
        }
        false
    })
}

/// A file's size and modification time ("-" when it does not exist)
fn file_fp(p: &Path) -> String {
    match std::fs::metadata(p) {
        Ok(m) => {
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            format!("{}:{t}", m.len())
        }
        Err(_) => "-".into(),
    }
}

/// A file, or a directory with everything under it
fn path_fp(p: &Path) -> String {
    if p.is_dir() {
        let mut v = vec![];
        walk(p, Path::new(""), &mut v);
        hash(&v.iter().map(String::as_str).collect::<Vec<_>>())
    } else {
        file_fp(p)
    }
}

/// Every file of a package (not `target/`, the target directory or hidden directories), for a script without
/// `rerun-if-*` lines
fn src_fp(dir: &Path, skip: &Path) -> String {
    let mut v = vec![];
    walk(dir, skip, &mut v);
    hash(&v.iter().map(String::as_str).collect::<Vec<_>>())
}

fn walk(dir: &Path, skip: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut es: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    es.sort();
    for e in es {
        let name = e.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if e.is_dir() {
            if name != "target" && !name.starts_with('.') && crate::manifest::normalize(&e) != crate::manifest::normalize(skip) {
                walk(&e, skip, out);
            }
        } else {
            out.push(format!("{} {}", e.display(), file_fp(&e)));
        }
    }
}

/// FNV-1a over the parts, as 16 hex digits
pub fn hash(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes().chain([0xff]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}

fn rustc() -> String {
    std::env::var("RUSTC").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "rustc".into())
}

fn run_rustc(mut c: Command, what: &str) -> R<()> {
    let o = c.output().map_err(|e| format!("rustc ({what}): {e}"))?;
    if !o.status.success() {
        return Err(format!("rustc could not compile {what}:\n{}", tail(&String::from_utf8_lossy(&o.stderr))));
    }
    Ok(())
}

fn mkdir(d: &Path) -> R<()> {
    std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))
}

/// The last lines of a process's output
fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}
