//! `zurgo build`: the packages the selected members need on the target side, as one zustc workspace
//! (`Session2::build_workspace`): a library per package in dependency order, then the members' binaries.
//!
//! Not yet (mizchi/zust#28): build scripts are not run (a warning names them), proc-macro dependencies are an error
//! (mizchi/zust#29), and so are two versions of one library and renamed dependencies (zustc's workspace has one
//! name per crate).

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

pub fn target_dir(g: &Graph) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| g.ws_dir.join("target"))
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
    fn visit(g: &Graph, r: &resolve::Resolved, n: Node, order: &mut Vec<Node>, state: &mut BTreeMap<Node, u8>) -> R<()> {
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
                return Err(format!(
                    "`{}` depends on the proc-macro crate `{}`: proc macros cannot run yet (mizchi/zust#29)",
                    g.pkgs[n.0].m.name, g.pkgs[t.0].m.name
                ));
            }
            visit(g, r, t, order, state)?;
        }
        state.insert(n, 2);
        order.push(n);
        Ok(())
    }
    for &m in &selected {
        visit(g, &r, (m, false), &mut order, &mut state)?;
    }
    let display = |p: &Path| -> String {
        // (cargo gives rustc the members' paths from the workspace root, and the others' as they are)
        p.strip_prefix(&g.ws_dir).map(|x| x.display().to_string()).unwrap_or_else(|_| p.display().to_string())
    };
    let mut crates: Vec<CrateSpec> = vec![];
    let mut lib_of: BTreeMap<String, usize> = BTreeMap::new();
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
        if pk.m.build.is_some() {
            eprintln!("warning: the build script of `{} v{}` is not run yet (mizchi/zust#28); its cfgs and generated files are missing", pk.m.name, pk.m.version);
        }
        crates.push(CrateSpec {
            name: lib.name.clone(),
            root: lib.path.clone(),
            display: display(&lib.path),
            lib: true,
            deps: deps_of(g, &r, n)?,
            features: r.nodes[&n].features.iter().cloned().collect(),
            exe: None,
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
            // (`required-features`: a bin is built only when they are all on; asking for it by name says which miss.
            //  `dep/feat` is a feature of the dependency `dep`, on the node this package's edge reaches)
            let on = |f: &String| -> bool {
                match f.split_once('/') {
                    Some((dep, feat)) => r.nodes[&n].edges.iter().any(|&(ei, t)| {
                        g.decl(n.0, &g.edges[n.0][ei]).name == dep.trim_end_matches('?') && r.nodes.get(&t).is_some_and(|s| s.features.contains(feat))
                    }),
                    None => r.nodes[&n].features.contains(f),
                }
            };
            let missing: Vec<&String> = b.required_features.iter().filter(|f| !on(f)).collect();
            if !missing.is_empty() {
                if asked {
                    return Err(format!(
                        "the bin `{}` needs the features `{}` (`required-features`): turn them on with `--features`",
                        b.name,
                        missing.iter().map(|f| f.as_str()).collect::<Vec<_>>().join("`, `")
                    ));
                }
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
            crates.push(CrateSpec {
                name,
                root: b.path.clone(),
                display: display(&b.path),
                lib: false,
                deps,
                features: r.nodes[&n].features.iter().cloned().collect(),
                exe: Some(b.name.clone()),
            });
        }
    }
    let missing: Vec<&String> = o.bins.iter().filter(|b| !found.contains(b)).collect();
    if !missing.is_empty() {
        return Err(format!("no bin target named `{}`", missing.iter().map(|b| b.as_str()).collect::<Vec<_>>().join("`, `")));
    }
    Ok(crates)
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
    let mut s = String::new();
    for c in crates {
        let f = if c.features.is_empty() { String::new() } else { format!(" features={}", c.features.join(",")) };
        let e = match &c.exe {
            Some(e) if *e != c.name => format!(" exe={e}"),
            _ => String::new(),
        };
        s.push_str(&format!("{} {} {} {}{f}{e}\n", if c.lib { "lib" } else { "bin" }, c.name, c.root.display(), c.deps.join(" ")));
    }
    s
}

pub fn build(g: &Graph, o: &BuildOpts) -> R<Built> {
    let crates = plan(g, o)?;
    let out = out_dir(g, o);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    if o.verbose {
        let ws = ws_text(&crates);
        let p = out.join("zurgo-ws.txt");
        let _ = std::fs::write(&p, &ws);
        eprint!("{ws}");
        eprintln!("(written to {})", p.display());
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
    Session2::new(opts).build_workspace(&crates, &out)?;
    let bins = crates.iter().filter_map(|c| c.exe.as_ref().map(|e| (e.clone(), out.join(e)))).collect();
    Ok(Built { out_dir: out, bins, crates })
}
