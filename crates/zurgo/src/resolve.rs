//! The package graph and its features.
//!
//! The graph comes from Cargo.lock: each locked package with its manifest, and an edge for each dependency
//! declaration that the lock resolved. Features are then activated from the workspace members, as cargo does:
//! - `Mode::Unified` (what `cargo metadata` reports, from cargo's dependency resolver): one feature set per package,
//!   over every dependency kind and every platform; a weak `x?/f` also pulls `x` in (the dependency resolver
//!   over-approximates, the feature resolver decides later). `--filter-platform` only chooses which edges are shown
//!   (`metadata::json`);
//! - `Mode::Split` (what `cargo build` uses, resolver 2 and 3): a package built for the host (a build dependency, a
//!   proc macro, and what they use) has its own feature set; dev-dependencies are left out; a platform is checked
//!   against the host or the target, by the side the dependent is on.

use crate::lock::{self, Lock, LockDep};
use crate::manifest::{self, Dep, DepKind, Package, Workspace};
use crate::platform::Target;
use crate::semver::{Version, VersionReq};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

type R<T> = Result<T, String>;

#[derive(Clone, Debug)]
pub struct Pkg {
    /// as `cargo metadata` writes it (`registry+https://..#name@1.0.0`, `path+file:///dir#0.1.0`)
    pub id: String,
    pub source: Option<String>,
    pub checksum: Option<String>,
    pub m: Package,
    pub member: bool,
    /// the sources are not on disk (a package for another platform that `cargo fetch` did not download): only its
    /// name and version are known, and activating it is an error
    pub missing: bool,
}

#[derive(Clone, Debug)]
pub struct Edge {
    /// the declaration in the dependent's manifest (`Package::deps`)
    pub decl: usize,
    pub to: usize,
}

#[derive(Debug)]
pub struct Graph {
    pub pkgs: Vec<Pkg>,
    /// per package: its locked dependencies, of every kind and platform
    pub edges: Vec<Vec<Edge>>,
    pub members: Vec<usize>,
    /// `default-members`, or the root package, or every member of a virtual workspace (as cargo chooses them)
    pub default_members: Vec<usize>,
    pub root: Option<usize>,
    pub ws_dir: PathBuf,
    /// "1", "2" or "3"
    pub resolver: String,
}

impl Graph {
    pub fn decl(&self, p: usize, e: &Edge) -> &Dep {
        &self.pkgs[p].m.deps[e.decl]
    }
}

/// The manifest to start from: the given one, or the nearest Cargo.toml from the current directory up
pub fn find_manifest(given: Option<&Path>) -> R<PathBuf> {
    if let Some(p) = given {
        return std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    }
    let mut d = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        if d.join("Cargo.toml").is_file() {
            return Ok(d.join("Cargo.toml"));
        }
        if !d.pop() {
            return Err("could not find `Cargo.toml` in the current directory or any parent".into());
        }
    }
}

/// Expand the members of a workspace (`crates/*` globs in the last component)
fn members_of(ws: &Workspace) -> Vec<PathBuf> {
    expand(ws, &ws.members)
}

/// Directories of workspace patterns (`crates/*`: a glob in the last component), minus `exclude`
fn expand(ws: &Workspace, patterns: &[String]) -> Vec<PathBuf> {
    let mut out = vec![];
    for m in patterns {
        let (dir, last) = match m.rsplit_once('/') {
            Some((d, l)) => (ws.dir.join(d), l.to_string()),
            None => (ws.dir.clone(), m.clone()),
        };
        if last.contains('*') {
            let (pre, post) = last.split_once('*').unwrap();
            let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    n.starts_with(pre) && n.ends_with(post) && p.join("Cargo.toml").is_file()
                })
                .collect();
            found.sort();
            out.extend(found);
        } else {
            out.push(dir.join(last));
        }
    }
    out.retain(|p| !ws.exclude.iter().any(|x| ws.dir.join(x) == *p));
    out
}

/// The members of a workspace, each directory once: the root package (when the root manifest has one), the listed
/// members, and, transitively, the path dependencies inside the workspace's directory (cargo makes those members
/// too); none under `exclude`
fn ws_members(w: &Workspace) -> R<Vec<PathBuf>> {
    let excluded = |d: &Path| w.exclude.iter().any(|x| d.starts_with(manifest::normalize(&w.dir.join(x))));
    let mut todo: Vec<PathBuf> = vec![];
    if w.dir.join("Cargo.toml").is_file() && manifest::read(&w.dir.join("Cargo.toml"))?.contains_key("package") {
        todo.push(w.dir.clone());
    }
    todo.extend(members_of(w));
    let mut out: Vec<PathBuf> = vec![];
    let mut i = 0;
    while i < todo.len() {
        let d = manifest::normalize(&todo[i]);
        i += 1;
        if out.contains(&d) || (d != w.dir && excluded(&d)) {
            continue;
        }
        if let Ok(m) = manifest::load(&d.join("Cargo.toml"), Some(w)) {
            for dep in m.package.iter().flat_map(|p| p.deps.iter()) {
                if let Some(pd) = &dep.path {
                    if pd.starts_with(&w.dir) && pd.join("Cargo.toml").is_file() {
                        todo.push(pd.clone());
                    }
                }
            }
        }
        out.push(d);
    }
    Ok(out)
}

/// A directory as a file URL, percent-encoded as cargo's (the url crate's) path segments are
fn file_url(dir: &Path) -> String {
    let mut s = String::from("file://");
    for b in dir.to_string_lossy().bytes() {
        if b < 0x20 || b >= 0x7f || b" \"#<>?`{}%".contains(&b) {
            s.push_str(&format!("%{b:02X}"));
        } else {
            s.push(b as char);
        }
    }
    s
}

fn path_id(dir: &Path, name: &str, version: &str) -> String {
    let last = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let url = file_url(dir);
    if last == name { format!("path+{url}#{version}") } else { format!("path+{url}#{name}@{version}") }
}

/// Load the workspace of a manifest and the packages its Cargo.lock names
pub fn load(manifest_path: &Path) -> R<Graph> {
    let start_dir = manifest::normalize(manifest_path.parent().unwrap());
    // (the workspace first: this manifest's, or a parent's that has this package as a member; then the package,
    //  whose fields may be inherited from it)
    let mut ws: Option<Workspace> = manifest::workspace_of(&manifest::read(manifest_path)?, &start_dir);
    if ws.is_none() {
        let mut d = start_dir.clone();
        while d.pop() {
            let c = d.join("Cargo.toml");
            if c.is_file() {
                if let Some(w) = manifest::workspace_of(&manifest::read(&c)?, &d) {
                    if ws_members(&w)?.contains(&start_dir) {
                        ws = Some(w);
                    }
                    break;
                }
            }
        }
    }
    let start = manifest::load(manifest_path, ws.as_ref())?;
    let ws_dir = ws.as_ref().map(|w| w.dir.clone()).unwrap_or_else(|| start_dir.clone());
    let member_dirs: Vec<PathBuf> = match &ws {
        Some(w) => ws_members(w)?,
        None => vec![start_dir.clone()],
    };
    let default_dirs: Vec<PathBuf> = match &ws {
        Some(w) if !w.default_members.is_empty() => expand(w, &w.default_members).iter().map(|d| manifest::normalize(d)).collect(),
        Some(w) if member_dirs.contains(&w.dir) => vec![w.dir.clone()],
        _ => member_dirs.clone(),
    };
    let lock_path = ws_dir.join("Cargo.lock");
    if !lock_path.is_file() {
        return Err(format!(
            "{}: no Cargo.lock. zurgo has no resolver yet (mizchi/zust#28): run `cargo generate-lockfile` first",
            ws_dir.display()
        ));
    }
    let lock = lock::read(&lock_path)?;
    let resolver = start
        .package
        .as_ref()
        .and_then(|p| p.resolver.clone())
        .or_else(|| ws.as_ref().and_then(|w| w.resolver.clone()))
        .unwrap_or_else(|| match start.package.as_ref().map(|p| p.edition.as_str()) {
            Some("2024") => "3".into(),
            Some("2021") => "2".into(),
            _ => "1".into(),
        });
    let mut g = Graph { pkgs: vec![], edges: vec![], members: vec![], default_members: vec![], root: None, ws_dir: ws_dir.clone(), resolver };
    // (locked package → loaded package)
    let mut of_lock: BTreeMap<usize, usize> = BTreeMap::new();
    // (path packages: the directory of each name and version, found through the members' path dependencies)
    let mut path_dirs: BTreeMap<(String, String), PathBuf> = BTreeMap::new();
    let mut queue: Vec<usize> = vec![];
    for d in &member_dirs {
        let m = manifest::load(&d.join("Cargo.toml"), ws.as_ref())?;
        let Some(p) = m.package else { continue };
        let li = lock
            .packages
            .iter()
            .position(|l| l.name == p.name && l.version == p.version && l.source.is_none())
            .ok_or_else(|| format!("Cargo.lock does not list the member `{} v{}`: run `cargo generate-lockfile`", p.name, p.version))?;
        let k = g.pkgs.len();
        g.pkgs.push(Pkg { id: path_id(d, &p.name, &p.version), source: None, checksum: None, m: p, member: true, missing: false });
        g.members.push(k);
        if default_dirs.contains(d) {
            g.default_members.push(k);
        }
        if d == &start_dir {
            g.root = Some(k);
        }
        of_lock.insert(li, k);
        queue.push(k);
    }
    while let Some(k) = queue.pop() {
        // (the path dependencies of this package, so locked path packages can be found)
        for d in &g.pkgs[k].m.deps {
            if let Some(p) = &d.path {
                if let Ok(m) = manifest::load(&p.join("Cargo.toml"), ws.as_ref()) {
                    if let Some(pp) = m.package {
                        path_dirs.entry((pp.name.clone(), pp.version.clone())).or_insert_with(|| p.clone());
                    }
                }
            }
        }
        let li = *of_lock.iter().find(|(_, v)| **v == k).unwrap().0;
        let mut edges = vec![];
        let ldeps = lock.packages[li].deps.clone();
        for (di, d) in g.pkgs[k].m.deps.clone().iter().enumerate() {
            let Some(target_li) = pick(&lock, &ldeps, d) else {
                // (not locked: a dev-dependency of a package outside the workspace, or an optional one no feature
                //  turns on)
                if d.kind == DepKind::Dev && !g.pkgs[k].member || d.optional {
                    continue;
                }
                return Err(format!(
                    "Cargo.lock does not resolve `{}` for `{} v{}`: run `cargo update` or `cargo generate-lockfile`",
                    d.name, g.pkgs[k].m.name, g.pkgs[k].m.version
                ));
            };
            let to = match of_lock.get(&target_li) {
                Some(&t) => t,
                None => {
                    let lp = &lock.packages[target_li];
                    let (dir, id) = match &lp.source {
                        None => {
                            let dir = path_dirs
                                .get(&(lp.name.clone(), lp.version.clone()))
                                .cloned()
                                .or_else(|| d.path.clone())
                                .ok_or_else(|| format!("no directory for the path package `{} v{}`", lp.name, lp.version))?;
                            let id = path_id(&dir, &lp.name, &lp.version);
                            (dir, id)
                        }
                        Some(s) if lock::is_registry(s) => {
                            let id = format!("{}#{}@{}", lock::CRATES_IO, lp.name, lp.version);
                            match lock::registry_dir(&lp.name, &lp.version) {
                                Some(dir) => (dir, id),
                                None => {
                                    // (not downloaded: its dependencies and features from the index cache, when there)
                                    let m = match lock::index_entry(&lp.name, &lp.version) {
                                        Some(e) => Package::from_index(&e),
                                        None => Package::placeholder(&lp.name, &lp.version),
                                    };
                                    let t = g.pkgs.len();
                                    g.pkgs.push(Pkg { id, source: lp.source.clone(), checksum: lp.checksum.clone(), m, member: false, missing: true });
                                    of_lock.insert(target_li, t);
                                    edges.push(Edge { decl: di, to: t });
                                    queue.push(t);
                                    continue;
                                }
                            }
                        }
                        Some(s) => return Err(format!("`{} v{}`: source `{s}` is not supported yet (only crates.io and paths)", lp.name, lp.version)),
                    };
                    let m = manifest::load(&dir.join("Cargo.toml"), if lp.source.is_none() { ws.as_ref() } else { None })?;
                    let p = m.package.ok_or_else(|| format!("{}: not a package", dir.display()))?;
                    let t = g.pkgs.len();
                    g.pkgs.push(Pkg { id, source: lp.source.clone(), checksum: lp.checksum.clone(), m: p, member: false, missing: false });
                    of_lock.insert(target_li, t);
                    queue.push(t);
                    t
                }
            };
            edges.push(Edge { decl: di, to });
        }
        if g.edges.len() <= k {
            g.edges.resize_with(g.pkgs.len().max(k + 1), Vec::new);
        }
        g.edges[k] = edges;
    }
    g.edges.resize_with(g.pkgs.len(), Vec::new);
    Ok(g)
}

/// The locked package a declaration resolved to: among the dependent's locked dependencies, those of the declared
/// package, then the one whose version the requirement allows (and whose kind of source matches)
fn pick(lock: &Lock, ldeps: &[LockDep], d: &Dep) -> Option<usize> {
    let req = VersionReq::parse(&d.req);
    let cands: Vec<usize> = ldeps.iter().filter(|x| x.name == d.package).filter_map(|x| lock.find(x)).collect();
    let ok = |&&li: &&usize| {
        let p = &lock.packages[li];
        let v_ok = match (&req, Version::parse(&p.version)) {
            (Some(r), Some(v)) => r.matches(&v) || (d.path.is_some() && d.req == "*"),
            _ => true,
        };
        // (the entry comes from the source the dependency names: a path entry for a path; a git entry of that
        //  repository for a git dependency; another registry's entry for `registry = ..`; crates.io's for the rest,
        //  or a path entry that `[patch]` put there. A mismatch is no candidate, so the caller asks for a new lock)
        let src_ok = match (&d.path, &d.git, &d.registry, p.source.as_deref()) {
            (Some(_), _, _, s) => s.is_none(),
            (None, Some(url), _, Some(s)) => {
                let norm = |u: &str| u.trim_end_matches('/').trim_end_matches(".git").to_string();
                s.strip_prefix("git+").and_then(|r| r.split(['?', '#']).next()).is_some_and(|r| norm(r) == norm(url))
            }
            (None, None, Some(_), Some(s)) => !lock::is_registry(s) && (s.starts_with("registry+") || s.starts_with("sparse+")),
            (None, None, None, Some(s)) => lock::is_registry(s),
            (None, None, None, None) => true,
            (None, _, _, None) => false,
        };
        v_ok && src_ok
    };
    let fit: Vec<&usize> = cands.iter().filter(ok).collect();
    // (none fits: the lock is older than the manifest, and the caller says so)
    fit.first().map(|x| **x)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Unified,
    Split,
}

pub struct Opts<'a> {
    pub mode: Mode,
    /// include the members' dev-dependencies
    pub dev: bool,
    /// the target (`Mode::Split`; `Mode::Unified` ignores platforms)
    pub target: Option<&'a Target>,
    pub host: &'a Target,
    /// the features asked for on the command line (`--features`), for the members
    pub features: Vec<String>,
    pub all_features: bool,
    pub no_default: bool,
    /// the members whose features are activated (a build's selection); `None`: every member (metadata)
    pub only: Option<Vec<usize>>,
}

/// A package on one side: `(package, built for the host)`
pub type Node = (usize, bool);

#[derive(Clone, Debug, Default)]
pub struct NodeState {
    pub features: BTreeSet<String>,
    /// the active edges: (index in `Graph::edges[package]`, the node it reaches)
    pub edges: Vec<(usize, Node)>,
    /// optional dependencies turned on (by their name in the manifest)
    pub on: BTreeSet<String>,
}

pub struct Resolved {
    pub nodes: BTreeMap<Node, NodeState>,
}

impl Resolved {
    /// An error if an activated package has no sources on disk
    pub fn check_sources(&self, g: &Graph) -> R<()> {
        let mut miss: Vec<String> = self.nodes.keys().filter(|n| g.pkgs[n.0].missing).map(|n| format!("`{} v{}`", g.pkgs[n.0].m.name, g.pkgs[n.0].m.version)).collect();
        miss.dedup();
        if miss.is_empty() {
            return Ok(());
        }
        Err(format!("the sources of {} are not in {}/registry/src: run `cargo fetch` first", miss.join(", "), lock::cargo_home().display()))
    }
}

struct St<'g> {
    g: &'g Graph,
    o: &'g Opts<'g>,
    nodes: BTreeMap<Node, NodeState>,
    /// features asked of a dependency by `x/f` (and `x?/f` once `x` is on)
    dep_feats: BTreeMap<(Node, String), BTreeSet<String>>,
    /// `x?/f` waiting for `x` to be turned on by something else
    weak: BTreeMap<(Node, String), BTreeSet<String>>,
}

pub fn features(g: &Graph, o: &Opts) -> Resolved {
    let mut st = St { g, o, nodes: BTreeMap::new(), dep_feats: BTreeMap::new(), weak: BTreeMap::new() };
    for &m in o.only.as_ref().unwrap_or(&g.members) {
        let n = (m, false);
        st.activate(n);
        let p = &g.pkgs[m].m;
        if !o.no_default {
            st.feature(n, "default");
        }
        if o.all_features {
            for f in p.features.keys() {
                st.feature(n, f);
            }
            for d in p.deps.iter().filter(|d| d.optional) {
                st.entry(n, &format!("dep:{}", d.name));
            }
        }
        // (`--features`: `member/feat` names a member's feature; anything else, `feat` or a dependency's `dep/feat`,
        //  is for the selected packages: a build's selection. Metadata selects the whole workspace, so there it goes to
        //  every member that has the feature (or the dependency), as cargo's resolver 2 does; and to the root package
        //  or the only member also when none has it, so that the error names it)
        let has = |f: &str| match f.split_once('/') {
            Some((dep, _)) => p.deps.iter().any(|d| d.name == dep.trim_end_matches('?')),
            None => p.features.contains_key(f) || p.deps.iter().any(|d| d.optional && d.name == f),
        };
        for f in &o.features {
            let selected = match &o.only {
                Some(only) => only.contains(&m),
                None => has(f) || Some(m) == g.root || g.members.len() == 1,
            };
            match f.split_once('/') {
                Some((pk, ff)) if g.pkgs[m].m.name == pk => st.entry(n, ff),
                _ if selected => st.entry(n, f),
                _ => {}
            }
        }
    }
    Resolved { nodes: st.nodes }
}

impl St<'_> {
    fn edge_ok(&self, n: Node, e: &Edge) -> bool {
        let d = self.g.decl(n.0, e);
        if d.kind == DepKind::Dev && !(self.o.dev && self.g.pkgs[n.0].member) {
            return false;
        }
        let Some(p) = &d.platform else { return true };
        let t = match self.o.mode {
            Mode::Unified => return true,
            Mode::Split => {
                if n.1 || d.kind == DepKind::Build {
                    self.o.host
                } else {
                    self.o.target.unwrap_or(self.o.host)
                }
            }
        };
        t.matches(p).unwrap_or(false)
    }

    fn target_of(&self, n: Node, e: &Edge) -> Node {
        match self.o.mode {
            Mode::Unified => (e.to, false),
            Mode::Split => (e.to, n.1 || self.g.decl(n.0, e).kind == DepKind::Build || self.g.pkgs[e.to].m.is_proc_macro()),
        }
    }

    fn activate(&mut self, n: Node) {
        if self.nodes.contains_key(&n) {
            return;
        }
        self.nodes.insert(n, NodeState::default());
        for (ei, e) in self.g.edges[n.0].iter().enumerate() {
            if !self.g.decl(n.0, e).optional && self.edge_ok(n, e) {
                self.activate_edge(n, ei);
            }
        }
    }

    fn activate_edge(&mut self, n: Node, ei: usize) {
        let e = &self.g.edges[n.0][ei];
        let t = self.target_of(n, e);
        if self.nodes[&n].edges.iter().any(|x| x.0 == ei) {
            return;
        }
        self.nodes.get_mut(&n).unwrap().edges.push((ei, t));
        self.activate(t);
        let d = self.g.decl(n.0, e);
        if d.default_features {
            self.feature(t, "default");
        }
        for f in &d.features {
            self.entry(t, f);
        }
        let asked: Vec<String> = self.dep_feats.get(&(n, d.name.clone())).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        for f in asked {
            self.entry(t, &f);
        }
    }

    /// Whether `x` is an optional dependency with an implicit feature of its name (no `dep:x` anywhere)
    fn implicit(&self, p: usize, x: &str) -> bool {
        let m = &self.g.pkgs[p].m;
        m.deps.iter().any(|d| d.optional && d.name == x)
            && !m.features.contains_key(x)
            && !m.features.values().flatten().any(|v| v.strip_prefix("dep:") == Some(x))
    }

    fn feature(&mut self, n: Node, f: &str) {
        let m = &self.g.pkgs[n.0].m;
        let defined = m.features.contains_key(f);
        let implicit = !defined && self.implicit(n.0, f);
        if !defined && !implicit {
            // (`default` without a `[features] default`, or a feature the package does not have)
            return;
        }
        if !self.nodes.get_mut(&n).unwrap().features.insert(f.to_string()) {
            return;
        }
        if implicit {
            self.enable_dep(n, f);
            return;
        }
        for e in m.features[f].clone() {
            self.entry(n, &e);
        }
    }

    /// One value of a feature's list, or a feature asked for: `f`, `dep:x`, `x/f`, `x?/f`
    fn entry(&mut self, n: Node, e: &str) {
        if let Some(x) = e.strip_prefix("dep:") {
            self.enable_dep(n, x);
            return;
        }
        if let Some((x, f)) = e.split_once('/') {
            match x.strip_suffix('?') {
                Some(x) if self.o.mode == Mode::Unified => {
                    self.enable_dep(n, x);
                    self.dep_feature(n, x, f);
                }
                Some(x) => {
                    if self.dep_on(n, x) {
                        self.dep_feature(n, x, f);
                    } else {
                        self.weak.entry((n, x.to_string())).or_default().insert(f.to_string());
                    }
                }
                None => {
                    // (`x/f` turns the dependency on, and, when `x` is optional, the feature named `x` if there is
                    //  one: explicit, or implicit)
                    let m = &self.g.pkgs[n.0].m;
                    if m.deps.iter().any(|d| d.optional && d.name == x) && (m.features.contains_key(x) || self.implicit(n.0, x)) {
                        self.feature(n, x);
                    }
                    self.enable_dep(n, x);
                    self.dep_feature(n, x, f);
                }
            }
            return;
        }
        self.feature(n, e);
    }

    fn dep_on(&self, n: Node, x: &str) -> bool {
        self.nodes[&n].on.contains(x) || self.g.edges[n.0].iter().any(|e| {
            let d = self.g.decl(n.0, e);
            d.name == x && !d.optional && self.edge_ok(n, e)
        })
    }

    fn dep_feature(&mut self, n: Node, x: &str, f: &str) {
        if !self.dep_feats.entry((n, x.to_string())).or_default().insert(f.to_string()) {
            return;
        }
        let targets: Vec<Node> = self.nodes[&n].edges.iter().filter(|(ei, _)| self.g.decl(n.0, &self.g.edges[n.0][*ei]).name == x).map(|x| x.1).collect();
        for t in targets {
            self.entry(t, f);
        }
    }

    fn enable_dep(&mut self, n: Node, x: &str) {
        if !self.nodes.get_mut(&n).unwrap().on.insert(x.to_string()) {
            return;
        }
        for ei in 0..self.g.edges[n.0].len() {
            let e = &self.g.edges[n.0][ei];
            if self.g.decl(n.0, e).name == x && self.edge_ok(n, e) {
                self.activate_edge(n, ei);
            }
        }
        if let Some(fs) = self.weak.remove(&(n, x.to_string())) {
            for f in fs {
                self.dep_feature(n, x, &f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, files: &[(&str, &str)]) {
        for (p, text) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
    }

    /// A workspace of path packages: features through `dep:`, `x/f`, `x?/f`, implicit features, default-features,
    /// a build dependency (its own side in Split mode) and a platform dependency
    #[test]
    fn features_like_cargo() {
        let dir = std::env::temp_dir().join(format!("zurgo-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir,
            &[
                ("app/Cargo.toml", "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\na = { path = \"../a\", features = [\"x\"] }\nb = { path = \"../b\", default-features = false }\n[build-dependencies]\nb = { path = \"../b\" }\n[target.'cfg(windows)'.dependencies]\nw = { path = \"../w\" }\n"),
                ("app/src/main.rs", "fn main() {}\n"),
                ("a/Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\n[dependencies]\nb = { path = \"../b\", optional = true, default-features = false }\nc = { path = \"../c\", optional = true }\n[features]\nx = [\"dep:b\", \"b/fb\", \"c?/fc\"]\ny = [\"c\"]\n"),
                ("a/src/lib.rs", ""),
                ("b/Cargo.toml", "[package]\nname = \"b\"\nversion = \"0.1.0\"\n[features]\ndefault = [\"d\"]\nd = []\nfb = []\n"),
                ("b/src/lib.rs", ""),
                ("c/Cargo.toml", "[package]\nname = \"c\"\nversion = \"0.1.0\"\n[features]\nfc = []\n"),
                ("c/src/lib.rs", ""),
                ("w/Cargo.toml", "[package]\nname = \"w\"\nversion = \"0.1.0\"\n"),
                ("w/src/lib.rs", ""),
                (
                    "app/Cargo.lock",
                    "version = 4\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\ndependencies = [\"b\", \"c\"]\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"a\", \"b\", \"w\"]\n[[package]]\nname = \"b\"\nversion = \"0.1.0\"\n[[package]]\nname = \"c\"\nversion = \"0.1.0\"\n[[package]]\nname = \"w\"\nversion = \"0.1.0\"\n",
                ),
            ],
        );
        let g = load(&dir.join("app/Cargo.toml")).unwrap();
        let host = Target::host();
        let by = |r: &Resolved, name: &str, h: bool| -> Option<Vec<String>> {
            let p = g.pkgs.iter().position(|p| p.m.name == name)?;
            r.nodes.get(&(p, h)).map(|s| s.features.iter().cloned().collect())
        };
        // (unified, as cargo's dependency resolver: b's features from every side; the weak `c?/fc` pulls c in;
        //  w is resolved too (platforms are only a filter of what metadata shows))
        let u = features(&g, &Opts { mode: Mode::Unified, dev: true, target: Some(&host), host: &host, features: vec![], all_features: false, no_default: false, only: None });
        assert_eq!(by(&u, "a", false), Some(vec!["x".to_string()]));
        assert_eq!(by(&u, "b", false), Some(vec!["d".to_string(), "default".to_string(), "fb".to_string()]));
        assert_eq!(by(&u, "c", false), Some(vec!["fc".to_string()]));
        assert_eq!(by(&u, "w", false), Some(vec![]));
        // (split: c is not on (only weakly asked for), and w is not for this platform)
        let s0 = features(&g, &Opts { mode: Mode::Split, dev: false, target: Some(&host), host: &host, features: vec![], all_features: false, no_default: false, only: None });
        assert_eq!(by(&s0, "c", false), None);
        assert_eq!(by(&s0, "w", false), None);
        // (split: the target side's b has no default features; the build dependency's b is its own node)
        let s = features(&g, &Opts { mode: Mode::Split, dev: false, target: Some(&host), host: &host, features: vec![], all_features: false, no_default: false, only: None });
        assert_eq!(by(&s, "b", false), Some(vec!["fb".to_string()]));
        assert_eq!(by(&s, "b", true), Some(vec!["d".to_string(), "default".to_string()]));
        // (`a/y` turns c on through its implicit feature, and then the weak `c?/fc` applies)
        let y = features(&g, &Opts { mode: Mode::Unified, dev: true, target: Some(&host), host: &host, features: vec!["a/y".into()], all_features: false, no_default: false, only: None });
        assert_eq!(by(&y, "a", false), Some(vec!["c".to_string(), "x".to_string(), "y".to_string()]));
        assert_eq!(by(&y, "c", false), Some(vec!["fc".to_string()]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
