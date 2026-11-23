//! `zurgo build`: the packages the selected members need on the target side, as one zustc workspace
//! (`Session2::build_workspace`): a library per package in dependency order, then the members' binaries.
//!
//! Build scripts run first (build_script.rs); their cfgs and environment, and cargo's `CARGO_PKG_*`, go to each
//! crate. A proc-macro dependency is a stub library (proc_macro_stub.rs): using one of its macros is an error
//! (mizchi/zust#29). Not yet (mizchi/zust#28): two versions of one library and renamed dependencies (zustc's
//! workspace has one name per crate).

use crate::build_script::{self, Compiler, Runner};
use crate::proc_macro_stub;
use crate::metadata::extern_name;
use crate::platform::Target;
use crate::resolve::{self, Graph, Mode, Node, Opts};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use zustc::driver2::{CrateSpec, Options2, Session2};

type R<T> = Result<T, String>;

#[derive(Clone, Debug, Default)]
pub struct BuildOpts {
    pub release: bool,
    pub target: Option<String>,
    pub features: Vec<String>,
    pub all_features: bool,
    pub no_default: bool,
    /// only these binaries (all of the selected packages' when empty)
    pub bins: Vec<String>,
    pub jobs: Option<usize>,
    pub verbose: bool,
    pub time: bool,
    pub no_cache: bool,
    /// `--build-script-compiler`
    pub bs_compiler: Compiler,
}

pub struct Built {
    pub out_dir: PathBuf,
    /// (binary name, executable)
    pub bins: Vec<(String, PathBuf)>,
    pub crates: Vec<CrateSpec>,
}

/// The packages a command works on, as cargo chooses them: a member's own manifest selects that member; the root
/// manifest of a workspace selects its default members (`default-members`, else the root package, else every member)
pub fn selection(g: &Graph) -> Vec<usize> {
    match g.root {
        Some(r) if crate::manifest::normalize(&g.pkgs[r].m.dir) != g.ws_dir => vec![r],
        _ => g.default_members.clone(),
    }
}

/// The target directory (`CARGO_TARGET_DIR`, relative to where zurgo runs, else the workspace's `target/`), absolute:
/// build scripts and rustc run in other directories
pub fn target_dir(g: &Graph) -> PathBuf {
    let d = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| g.ws_dir.join("target"));
    if d.is_absolute() {
        d
    } else {
        std::env::current_dir().map(|c| c.join(&d)).unwrap_or(d)
    }
}

pub fn out_dir(g: &Graph, o: &BuildOpts) -> PathBuf {
    let mut d = target_dir(g).join("zust");
    if let Some(t) = &o.target {
        d = d.join(t);
    }
    d.join(if o.release { "release" } else { "debug" })
}

/// The workspace for zustc (also written by `-v`, in the format of `zustc build`)
pub fn plan(g: &Graph, o: &BuildOpts) -> R<Vec<CrateSpec>> {
    let host = Target::host();
    let target = match &o.target {
        Some(t) => Target::by_triple(t).ok_or_else(|| format!("unknown target `{t}` (x86_64-unknown-linux-gnu, wasm32-unknown-unknown)"))?,
        None => host.clone(),
    };
    // (the members to build. Their features alone are activated: another member's do not leak into this build)
    let selected = selection(g);
    let r = resolve::features(
        g,
        &Opts {
            mode: Mode::Split,
            dev: false,
            target: Some(&target),
            host: &host,
            features: o.features.clone(),
            all_features: o.all_features,
            no_default: o.no_default,
            only: Some(selected.clone()),
        },
    );
    r.check_sources(g)?;
    // (the target side, dependencies first)
    let mut order: Vec<Node> = vec![];
    let mut state: BTreeMap<Node, u8> = BTreeMap::new();
    // (proc-macro dependencies: stubs, before the crates that use them)
    let mut pms: Vec<Node> = vec![];
    fn visit(g: &Graph, r: &resolve::Resolved, n: Node, order: &mut Vec<Node>, state: &mut BTreeMap<Node, u8>, pms: &mut Vec<Node>) -> R<()> {
        match state.get(&n) {
            Some(2) => return Ok(()),
            Some(_) => return Err(format!("dependency cycle at `{}`", g.pkgs[n.0].m.name)),
            None => {}
        }
        state.insert(n, 1);
        for &(ei, t) in &r.nodes[&n].edges {
            let d = g.decl(n.0, &g.edges[n.0][ei]);
            if d.kind != crate::manifest::DepKind::Normal {
                continue;
            }
            if g.pkgs[t.0].m.is_proc_macro() {
                if !pms.iter().any(|p| p.0 == t.0) {
                    pms.push(t);
                }
                continue;
            }
            visit(g, r, t, order, state, pms)?;
        }
        state.insert(n, 2);
        order.push(n);
        Ok(())
    }
    for &m in &selected {
        visit(g, &r, (m, false), &mut order, &mut state, &mut pms)?;
    }
    let display = |p: &Path| -> String {
        // (cargo gives rustc the members' paths from the workspace root, and the others' as they are)
        p.strip_prefix(&g.ws_dir).map(|x| x.display().to_string()).unwrap_or_else(|_| p.display().to_string())
    };
    // (the bins asked for: they exist and their required features are on, before any build script runs)
    if !o.bins.is_empty() {
        let mut seen: Vec<&String> = vec![];
        for &m in &selected {
            for b in g.pkgs[m].m.bins.iter().filter(|b| o.bins.contains(&b.name)) {
                seen.push(&b.name);
                let missing = missing_features(g, &r, (m, false), b);
                if !missing.is_empty() {
                    return Err(format!(
                        "the bin `{}` needs the features `{}` (`required-features`): turn them on with `--features`",
                        b.name,
                        missing.iter().map(|f| f.as_str()).collect::<Vec<_>>().join("`, `")
                    ));
                }
            }
        }
        let missing: Vec<&String> = o.bins.iter().filter(|b| !seen.contains(b)).collect();
        if !missing.is_empty() {
            return Err(format!("no bin target named `{}`", missing.iter().map(|b| b.as_str()).collect::<Vec<_>>().join("`, `")));
        }
    }
    // (build scripts, dependencies first)
    let mut bs = Runner::new(g, &r, &target, &host, out_dir(g, o), o.release, o.bs_compiler, o.jobs, o.verbose);
    let mut bs_out: BTreeMap<Node, build_script::Output> = BTreeMap::new();
    for &n in &order {
        if let Some(x) = bs.run(n)? {
            bs_out.insert(n, x);
        }
    }
    let mut crates: Vec<CrateSpec> = vec![];
    let mut lib_of: BTreeMap<String, usize> = BTreeMap::new();
    // (a stub for each proc-macro dependency; a crate of the build that uses one of their derives or attributes is
    //  an error, as zustc would drop the attribute)
    // (the proc macros of each stub, checked against the crates once they are all planned)
    let mut pm_macros: Vec<(usize, proc_macro_stub::Macros)> = vec![];
    if !pms.is_empty() {
        let stub_dir = out_dir(g, o).join("proc-macro-stubs");
        std::fs::create_dir_all(&stub_dir).map_err(|e| format!("{}: {e}", stub_dir.display()))?;
        for &pn in &pms {
            let p = pn.0;
            let pk = &g.pkgs[p].m;
            let Some(lib) = &pk.lib else { continue };
            // (two proc-macro packages of one library name: one stub cannot stand for both)
            if let Some(&other) = lib_of.get(&lib.name) {
                return Err(format!(
                    "two packages give the library `{}`: `{} v{}` and `{} v{}`. Two versions of one crate are not supported yet (mizchi/zust#28)",
                    lib.name, g.pkgs[other].m.name, g.pkgs[other].m.version, pk.name, pk.version
                ));
            }
            let src = std::fs::read_to_string(&lib.path).map_err(|e| format!("{}: {e}", lib.path.display()))?;
            // (its root with its features: `#[cfg_attr(feature = "x", proc_macro_attribute)]`)
            let cfg = zustc::cfg::CfgSet { features: r.nodes[&pn].features.iter().cloned().collect(), debug_assertions: !o.release, ..Default::default() };
            let m = proc_macro_stub::macros_of(&zustc::cfg::strip(&src, &cfg).unwrap_or(src));
            let stub = stub_dir.join(format!("{}.rs", lib.name));
            let text = proc_macro_stub::stub_text(&pk.name, &m);
            if std::fs::read_to_string(&stub).ok().as_deref() != Some(text.as_str()) {
                std::fs::write(&stub, &text).map_err(|e| format!("{}: {e}", stub.display()))?;
            }
            if o.verbose {
                eprintln!("note: `{} v{}` is a proc-macro crate: built as a stub (mizchi/zust#29)", pk.name, pk.version);
            }
            lib_of.insert(lib.name.clone(), p);
            crates.push(CrateSpec {
                name: lib.name.clone(),
                root: stub.clone(),
                display: stub.display().to_string(),
                lib: true,
                deps: vec![],
                features: vec![],
                exe: None,
                cfgs: vec![],
                env: vec![],
            });
            pm_macros.push((p, m));
        }
    }
    let n_stubs = crates.len();
    // (the node of each crate after the stubs)
    let mut node_of: Vec<Node> = vec![];
    for &n in &order {
        let pk = &g.pkgs[n.0];
        let Some(lib) = &pk.m.lib else { continue };
        if let Some(&other) = lib_of.get(&lib.name) {
            return Err(format!(
                "two packages give the library `{}`: `{} v{}` and `{} v{}`. Two versions of one crate are not supported yet (mizchi/zust#28)",
                lib.name, g.pkgs[other].m.name, g.pkgs[other].m.version, pk.m.name, pk.m.version
            ));
        }
        lib_of.insert(lib.name.clone(), n.0);
        let (cfgs, env) = build_script::crate_env(&pk.m, &lib.name, bs_out.get(&n));
        node_of.push(n);
        crates.push(CrateSpec {
            name: lib.name.clone(),
            root: lib.path.clone(),
            display: display(&lib.path),
            lib: true,
            deps: deps_of(g, &r, n)?,
            features: r.nodes[&n].features.iter().cloned().collect(),
            exe: None,
            cfgs,
            env,
        });
    }
    // (the requested binaries, over all the selected packages)
    let mut found: Vec<&String> = vec![];
    for &m in &selected {
        let pk = &g.pkgs[m];
        let n = (m, false);
        for b in &pk.m.bins {
            let asked = o.bins.contains(&b.name);
            if !o.bins.is_empty() && !asked {
                continue;
            }
            found.extend(o.bins.iter().filter(|x| **x == b.name));
            // (`required-features`: a bin is built only when they are all on; asking for it by name says which miss:
            //  checked before any build script runs, with the names asked for)
            if !missing_features(g, &r, n, b).is_empty() {
                continue;
            }
            let mut deps = deps_of(g, &r, n)?;
            if let Some(l) = &pk.m.lib {
                deps.insert(0, l.name.clone());
            }
            // (a crate name of its own when another crate has the bin's name: the package's library, or another
            //  bin's crate name)
            let mut name = b.name.clone();
            let mut k = 0;
            while lib_of.contains_key(&name) || crates.iter().any(|c| c.name == name) {
                k += 1;
                name = if k == 1 { format!("{}-bin", b.name) } else { format!("{}-bin{k}", b.name) };
            }
            let (cfgs, mut env) = build_script::crate_env(&pk.m, &b.name, bs_out.get(&n));
            env.push(("CARGO_BIN_NAME".into(), b.name.clone()));
            env.push(("CARGO_PRIMARY_PACKAGE".into(), "1".into()));
            node_of.push(n);
            crates.push(CrateSpec {
                name,
                root: b.path.clone(),
                display: display(&b.path),
                lib: false,
                deps,
                features: r.nodes[&n].features.iter().cloned().collect(),
                exe: Some(b.name.clone()),
                cfgs,
                env,
            });
        }
    }
    let missing: Vec<&String> = o.bins.iter().filter(|b| !found.contains(b)).collect();
    if !missing.is_empty() {
        return Err(format!("no bin target named `{}`", missing.iter().map(|b| b.as_str()).collect::<Vec<_>>().join("`, `")));
    }
    // (a crate that uses one of a stub's derives or attributes: an error, as zustc would drop the attribute. Each crate
    //  is read as zustc reads it: its module tree, with its features and cfgs. Only the proc-macro crates it reaches
    //  count, directly or through the libraries that re-export them)
    if !pm_macros.is_empty() {
        let wasm = target.triple.starts_with("wasm32");
        let mut reach: BTreeMap<Node, Vec<usize>> = BTreeMap::new();
        for (c, &n) in crates[n_stubs..].iter().zip(&node_of) {
            let pm_deps = pm_reach(g, &r, n, &mut reach);
            if pm_deps.is_empty() {
                continue;
            }
            let cfg = zustc::cfg::CfgSet { features: c.features.clone(), debug_assertions: !o.release, wasm, cfgs: c.cfgs.clone(), env: c.env.clone(), ..Default::default() };
            let (text, _) = zustc::srcmap::load_crate(&c.root, &c.display, &cfg)?;
            let u = proc_macro_stub::uses_in(&text);
            for (p, m) in pm_macros.iter().filter(|(p, _)| pm_deps.contains(p)) {
                if let Some(h) = u.hit(m) {
                    let pk = &g.pkgs[*p].m;
                    return Err(format!("`{}` uses {h} of the proc-macro crate `{} v{}`: proc macros cannot run yet (mizchi/zust#29)", c.name, pk.name, pk.version));
                }
            }
        }
    }
    Ok(crates)
}

/// The `required-features` of a bin that are off (`dep/feat` is a feature of the dependency `dep`, on the node this
/// package's edge reaches)
fn missing_features<'a>(g: &Graph, r: &resolve::Resolved, n: Node, b: &'a crate::manifest::Target) -> Vec<&'a String> {
    let on = |f: &String| -> bool {
        match f.split_once('/') {
            Some((dep, feat)) => r.nodes[&n].edges.iter().any(|&(ei, t)| {
                g.decl(n.0, &g.edges[n.0][ei]).name == dep.trim_end_matches('?') && r.nodes.get(&t).is_some_and(|s| s.features.contains(feat))
            }),
            None => r.nodes[&n].features.contains(f),
        }
    };
    b.required_features.iter().filter(|f| !on(f)).collect()
}

/// The proc-macro packages a node reaches through its normal dependencies (a library may re-export them)
fn pm_reach(g: &Graph, r: &resolve::Resolved, n: Node, memo: &mut BTreeMap<Node, Vec<usize>>) -> Vec<usize> {
    if let Some(v) = memo.get(&n) {
        return v.clone();
    }
    let mut v: Vec<usize> = vec![];
    for &(ei, t) in &r.nodes[&n].edges {
        if g.decl(n.0, &g.edges[n.0][ei]).kind != crate::manifest::DepKind::Normal {
            continue;
        }
        let more = if g.pkgs[t.0].m.is_proc_macro() { vec![t.0] } else { pm_reach(g, r, t, memo) };
        for p in more {
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    memo.insert(n, v.clone());
    v
}

/// The extern names of a node's normal dependencies (as zustc's workspace names them: the library names)
fn deps_of(g: &Graph, r: &resolve::Resolved, n: Node) -> R<Vec<String>> {
    let mut out: Vec<String> = vec![];
    for &(ei, t) in &r.nodes[&n].edges {
        let d = g.decl(n.0, &g.edges[n.0][ei]);
        if d.kind != crate::manifest::DepKind::Normal {
            continue;
        }
        let to = &g.pkgs[t.0].m;
        let Some(lib) = &to.lib else { continue };
        let name = extern_name(d, to);
        if name != lib.name {
            return Err(format!(
                "`{}` renames its dependency `{}` to `{}`: renamed dependencies are not supported yet (mizchi/zust#28)",
                g.pkgs[n.0].m.name, to.name, d.name
            ));
        }
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

/// The workspace as `zustc build` reads it (`-v`)
pub fn ws_text(crates: &[CrateSpec]) -> String {
    ws_text_in(crates, false)
}

/// ws_text, with the environment's values left out when `redact` (for a log: a build script's `rustc-env` may pass a
/// secret on)
fn ws_text_in(crates: &[CrateSpec], redact: bool) -> String {
    let mut s = String::new();
    for c in crates {
        let f = if c.features.is_empty() { String::new() } else { format!(" features={}", c.features.join(",")) };
        let mut e = match &c.exe {
            Some(e) if *e != c.name => format!(" exe={e}"),
            _ => String::new(),
        };
        let esc = zustc::driver2::ws_escape;
        for (k, v) in &c.cfgs {
            match v {
                Some(v) => e.push_str(&format!(" cfg={}={}", esc(k), esc(v))),
                None => e.push_str(&format!(" cfg={}", esc(k))),
            }
        }
        for (k, v) in &c.env {
            e.push_str(&format!(" env={}={}", esc(k), if redact { "…".to_string() } else { esc(v) }));
        }
        s.push_str(&format!("{} {} {} {}{f}{e}\n", if c.lib { "lib" } else { "bin" }, c.name, c.root.display(), c.deps.join(" ")));
    }
    s
}

pub fn build(g: &Graph, o: &BuildOpts) -> R<Built> {
    let out = out_dir(g, o);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    // (one zurgo build at a time in a target directory, as cargo locks its build directory: the OUT_DIR a build script
    //  filled stays the one this build's crates read)
    let _lock = build_script::lock_file(&out.join(".zurgo-lock"))?;
    let crates = plan(g, o)?;
    if o.verbose {
        let ws = ws_text(&crates);
        let p = out.join("zurgo-ws.txt");
        let _ = std::fs::write(&p, &ws);
        eprint!("{}", ws_text_in(&crates, true));
        eprintln!("(written to {}, with the environment's values)", p.display());
    }
    let mut opts = Options2 { opt: o.release, time: o.time, no_cache: o.no_cache, ..Options2::default() };
    if let Some(j) = o.jobs {
        opts.jobs = j;
    }
    match o.target.as_deref() {
        None | Some("x86_64-unknown-linux-gnu") => opts.backend = Some(zustc::clif_backend::compile),
        Some("wasm32-unknown-unknown") => {
            opts.backend = Some(zustc::wasm_backend::compile);
            opts.module_writer = Some(zustc::wasm_backend::write_module);
        }
        Some(t) => return Err(format!("unknown target `{t}`")),
    }
    let wasm = opts.module_writer.is_some();
    Session2::new(opts).build_workspace(&crates, &out)?;
    // (wasm32: a bin is the module `<name>.wasm`)
    let bins = crates.iter().filter_map(|c| c.exe.as_ref().map(|e| (e.clone(), if wasm { out.join(format!("{e}.wasm")) } else { out.join(e) }))).collect();
    Ok(Built { out_dir: out, bins, crates })
}
