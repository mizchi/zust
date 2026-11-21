//! Cargo.toml: the package, its targets (with Cargo's auto-discovery), its dependencies of the three kinds (also
//! under `[target.<platform>]`), its features, and `[workspace]` (members, and what members inherit from it).

use crate::toml::{self, Table, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DepKind {
    Normal,
    Build,
    Dev,
}

impl DepKind {
    /// (as `cargo metadata` writes it: `null`, `"build"`, `"dev"`)
    pub fn json(self) -> Option<&'static str> {
        match self {
            DepKind::Normal => None,
            DepKind::Build => Some("build"),
            DepKind::Dev => Some("dev"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dep {
    /// the key in the manifest: the name the dependent uses (a rename, if `package` is set)
    pub name: String,
    /// the package's own name
    pub package: String,
    /// the version requirement (`*` when none is given)
    pub req: String,
    pub path: Option<PathBuf>,
    pub git: Option<String>,
    pub registry: Option<String>,
    pub optional: bool,
    pub default_features: bool,
    pub features: Vec<String>,
    pub kind: DepKind,
    /// `[target.<platform>.dependencies]`
    pub platform: Option<String>,
}

impl Dep {
    pub fn renamed(&self) -> bool {
        self.name != self.package
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub name: String,
    /// (`lib`, `rlib`, `proc-macro`, `bin`, ...: what `cargo metadata` calls `kind`)
    pub kind: Vec<String>,
    pub crate_types: Vec<String>,
    pub path: PathBuf,
    pub edition: String,
}

impl Target {
    pub fn is_proc_macro(&self) -> bool {
        self.kind.iter().any(|k| k == "proc-macro")
    }
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub edition: String,
    pub links: Option<String>,
    /// the build script (absolute), if the package has one
    pub build: Option<PathBuf>,
    pub lib: Option<Target>,
    pub bins: Vec<Target>,
    pub deps: Vec<Dep>,
    pub features: BTreeMap<String, Vec<String>>,
    pub resolver: Option<String>,
    pub manifest_path: PathBuf,
    pub dir: PathBuf,
    /// the rest of `[package]` (description, license, authors, rust-version ...), for `CARGO_PKG_*`
    pub meta: Table,
}

impl Package {
    /// A package known only by name and version (its sources are not on disk)
    pub fn placeholder(name: &str, version: &str) -> Package {
        Package {
            name: name.to_string(),
            version: version.to_string(),
            edition: "2015".to_string(),
            links: None,
            build: None,
            lib: None,
            bins: vec![],
            deps: vec![],
            features: BTreeMap::new(),
            resolver: None,
            manifest_path: PathBuf::new(),
            dir: PathBuf::new(),
            meta: Table::new(),
        }
    }
    /// A package whose sources are not on disk, from its registry index entry (dependencies and features; the
    /// library is assumed to be the package's name, and has no path)
    pub fn from_index(t: &Table) -> Package {
        let name = toml::get_str(t, &["name"]).unwrap_or_default().to_string();
        let version = toml::get_str(t, &["vers"]).unwrap_or_default().to_string();
        let mut p = Package::placeholder(&name, &version);
        for k in ["features", "features2"] {
            for (f, v) in t.get(k).and_then(Value::as_table).into_iter().flatten() {
                p.features.entry(f.clone()).or_default().extend(v.str_list());
            }
        }
        p.links = toml::get_str(t, &["links"]).map(str::to_string);
        for d in t.get("deps").and_then(Value::as_array).unwrap_or(&[]) {
            let Some(d) = d.as_table() else { continue };
            let key = toml::get_str(d, &["name"]).unwrap_or_default().to_string();
            p.deps.push(Dep {
                package: toml::get_str(d, &["package"]).unwrap_or(&key).to_string(),
                name: key,
                req: toml::get_str(d, &["req"]).unwrap_or("*").to_string(),
                path: None,
                git: None,
                registry: toml::get_str(d, &["registry"]).map(str::to_string),
                optional: d.get("optional").and_then(Value::as_bool).unwrap_or(false),
                default_features: d.get("default_features").and_then(Value::as_bool).unwrap_or(true),
                features: d.get("features").map(Value::str_list).unwrap_or_default(),
                kind: match toml::get_str(d, &["kind"]) {
                    Some("build") => DepKind::Build,
                    Some("dev") => DepKind::Dev,
                    _ => DepKind::Normal,
                },
                platform: toml::get_str(d, &["target"]).map(crate::platform::normalize),
            });
        }
        p.lib = Some(Target { name: name.replace('-', "_"), kind: vec!["lib".into()], crate_types: vec!["lib".into()], path: PathBuf::new(), edition: "2015".into() });
        p
    }
    pub fn targets(&self) -> impl Iterator<Item = &Target> {
        self.lib.iter().chain(self.bins.iter())
    }
    pub fn is_proc_macro(&self) -> bool {
        self.lib.as_ref().is_some_and(Target::is_proc_macro)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub members: Vec<String>,
    pub exclude: Vec<String>,
    pub default_members: Vec<String>,
    pub resolver: Option<String>,
    /// `[workspace.package]` and `[workspace.dependencies]`, for `x.workspace = true`
    pub package: Table,
    pub dependencies: Table,
    pub dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub package: Option<Package>,
    pub workspace: Option<Workspace>,
}

type R<T> = Result<T, String>;

pub fn read(path: &Path) -> R<Table> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The `[workspace]` of a manifest, if it has one
pub fn workspace_of(t: &Table, dir: &Path) -> Option<Workspace> {
    let w = t.get("workspace")?.as_table()?;
    let list = |k: &str| w.get(k).map(Value::str_list).unwrap_or_default();
    Some(Workspace {
        members: list("members"),
        exclude: list("exclude"),
        default_members: list("default-members"),
        resolver: toml::get_str(w, &["resolver"]).map(str::to_string),
        package: w.get("package").and_then(Value::as_table).cloned().unwrap_or_default(),
        dependencies: w.get("dependencies").and_then(Value::as_table).cloned().unwrap_or_default(),
        dir: dir.to_path_buf(),
    })
}

/// Load a manifest. `ws` is the workspace it belongs to, for inherited fields
pub fn load(path: &Path, ws: Option<&Workspace>) -> R<Manifest> {
    let t = read(path)?;
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let workspace = workspace_of(&t, &dir);
    let ws = workspace.as_ref().or(ws);
    let package = match t.get("package").or_else(|| t.get("project")).and_then(Value::as_table) {
        Some(p) => Some(package(&t, p, &dir, path, ws).map_err(|e| format!("{}: {e}", path.display()))?),
        None => None,
    };
    Ok(Manifest { package, workspace })
}

/// A field of `[package]`, or the workspace's one for `field.workspace = true`
fn pkg_field(p: &Table, ws: Option<&Workspace>, key: &str) -> R<Option<Value>> {
    match p.get(key) {
        Some(Value::Table(t)) if t.get("workspace").and_then(Value::as_bool) == Some(true) => {
            let w = ws.ok_or(format!("`{key}.workspace = true` outside a workspace"))?;
            Ok(Some(w.package.get(key).cloned().ok_or(format!("`workspace.package.{key}` is not set"))?))
        }
        v => Ok(v.cloned()),
    }
}

fn package(t: &Table, p: &Table, dir: &Path, path: &Path, ws: Option<&Workspace>) -> R<Package> {
    let s = |k: &str| -> R<Option<String>> { Ok(pkg_field(p, ws, k)?.and_then(|v| v.as_str().map(str::to_string))) };
    let name = s("name")?.ok_or("no `package.name`")?;
    let version = s("version")?.unwrap_or_else(|| "0.0.0".to_string());
    let edition = s("edition")?.unwrap_or_else(|| "2015".to_string());
    let mut meta = Table::new();
    for k in ["description", "license", "license-file", "homepage", "repository", "documentation", "readme", "rust-version", "authors", "keywords", "categories"] {
        if let Some(v) = pkg_field(p, ws, k)? {
            meta.insert(k.to_string(), v);
        }
    }
    let build = match p.get("build") {
        Some(Value::Bool(false)) => None,
        Some(Value::String(b)) => Some(dir.join(b)),
        _ => Some(dir.join("build.rs")).filter(|b| b.is_file()),
    };
    let auto = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(true);
    // (the library: `[lib]`, or src/lib.rs)
    let lib_t = t.get("lib").and_then(Value::as_table);
    let lib_path = lib_t.and_then(|l| toml::get_str(l, &["path"])).map(|x| dir.join(x)).or_else(|| Some(dir.join("src/lib.rs")).filter(|x| x.is_file()));
    let lib = match lib_path {
        Some(lp) => {
            let l = lib_t.cloned().unwrap_or_default();
            let proc_macro = l.get("proc-macro").or_else(|| l.get("proc_macro")).and_then(Value::as_bool) == Some(true);
            let crate_types = l.get("crate-type").or_else(|| l.get("crate_type")).map(Value::str_list).filter(|v| !v.is_empty());
            let crate_types = if proc_macro { vec!["proc-macro".to_string()] } else { crate_types.unwrap_or_else(|| vec!["lib".to_string()]) };
            Some(Target {
                name: toml::get_str(&l, &["name"]).map(str::to_string).unwrap_or_else(|| name.replace('-', "_")),
                kind: crate_types.clone(),
                crate_types,
                path: lp,
                edition: toml::get_str(&l, &["edition"]).unwrap_or(&edition).to_string(),
            })
        }
        None => None,
    };
    // (binaries: `[[bin]]`, then src/main.rs and src/bin/ unless `autobins = false`)
    let mut bins: Vec<Target> = vec![];
    for b in t.get("bin").and_then(Value::as_array).unwrap_or(&[]) {
        let b = b.as_table().ok_or("`[[bin]]` is not a table")?;
        let bname = toml::get_str(b, &["name"]).ok_or("a `[[bin]]` without a name")?.to_string();
        let bpath = match toml::get_str(b, &["path"]) {
            Some(x) => dir.join(x),
            None => [format!("src/bin/{bname}.rs"), format!("src/bin/{bname}/main.rs")]
                .iter()
                .map(|x| dir.join(x))
                .find(|x| x.is_file())
                .unwrap_or_else(|| dir.join("src/main.rs")),
        };
        bins.push(Target { name: bname, kind: vec!["bin".into()], crate_types: vec!["bin".into()], path: bpath, edition: toml::get_str(b, &["edition"]).unwrap_or(&edition).to_string() });
    }
    if auto("autobins") {
        let mut found: Vec<(String, PathBuf)> = vec![];
        if dir.join("src/main.rs").is_file() {
            found.push((name.clone(), dir.join("src/main.rs")));
        }
        if let Ok(rd) = std::fs::read_dir(dir.join("src/bin")) {
            let mut es: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            es.sort();
            for e in es {
                if e.extension().is_some_and(|x| x == "rs") {
                    found.push((e.file_stem().unwrap().to_string_lossy().into_owned(), e));
                } else if e.join("main.rs").is_file() {
                    found.push((e.file_name().unwrap().to_string_lossy().into_owned(), e.join("main.rs")));
                }
            }
        }
        for (n, x) in found {
            if !bins.iter().any(|b| b.name == n || b.path == x) {
                bins.push(Target { name: n, kind: vec!["bin".into()], crate_types: vec!["bin".into()], path: x, edition: edition.clone() });
            }
        }
    }
    let mut deps = vec![];
    let tables: [(DepKind, &[&str]); 3] =
        [(DepKind::Normal, &["dependencies"]), (DepKind::Build, &["build-dependencies", "build_dependencies"]), (DepKind::Dev, &["dev-dependencies", "dev_dependencies"])];
    for (kind, keys) in tables {
        for k in keys {
            if let Some(d) = t.get(*k).and_then(Value::as_table) {
                deps_of(d, kind, None, dir, ws, &mut deps)?;
            }
            for (plat, pt) in t.get("target").and_then(Value::as_table).into_iter().flatten() {
                if let Some(d) = pt.as_table().and_then(|x| x.get(*k)).and_then(Value::as_table) {
                    deps_of(d, kind, Some(plat), dir, ws, &mut deps)?;
                }
            }
        }
    }
    let features = t.get("features").and_then(Value::as_table).map(|f| f.iter().map(|(k, v)| (k.clone(), v.str_list())).collect()).unwrap_or_default();
    Ok(Package {
        name,
        version,
        edition,
        links: s("links")?,
        build,
        lib,
        bins,
        deps,
        features,
        resolver: s("resolver")?,
        manifest_path: path.to_path_buf(),
        dir: dir.to_path_buf(),
        meta,
    })
}

fn deps_of(t: &Table, kind: DepKind, platform: Option<&String>, dir: &Path, ws: Option<&Workspace>, out: &mut Vec<Dep>) -> R<()> {
    for (key, v) in t {
        let mut d = Dep {
            name: key.clone(),
            package: key.clone(),
            req: "*".to_string(),
            path: None,
            git: None,
            registry: None,
            optional: false,
            default_features: true,
            features: vec![],
            kind,
            platform: platform.map(|p| crate::platform::normalize(p)),
        };
        match v {
            Value::String(req) => d.req = req.clone(),
            Value::Table(x) => {
                if x.get("workspace").and_then(Value::as_bool) == Some(true) {
                    let w = ws.ok_or(format!("dependency `{key}`: `workspace = true` outside a workspace"))?;
                    match w.dependencies.get(key) {
                        Some(Value::String(req)) => d.req = req.clone(),
                        Some(Value::Table(wt)) => fill(&mut d, wt, &w.dir),
                        _ => return Err(format!("dependency `{key}`: not in `[workspace.dependencies]`")),
                    }
                }
                // (a member's own `features` add to the workspace's; `optional` is the member's)
                let mut own = x.clone();
                own.remove("workspace");
                fill(&mut d, &own, dir);
            }
            _ => return Err(format!("dependency `{key}`: expected a version string or a table")),
        }
        out.push(d);
    }
    Ok(())
}

/// The fields of a dependency's table (`base`: where a `path` is relative to)
fn fill(d: &mut Dep, x: &Table, base: &Path) {
    if let Some(v) = toml::get_str(x, &["version"]) {
        d.req = v.to_string();
    }
    if let Some(p) = toml::get_str(x, &["path"]) {
        d.path = Some(base.join(p));
    }
    if let Some(p) = toml::get_str(x, &["package"]) {
        d.package = p.to_string();
    }
    if let Some(g) = toml::get_str(x, &["git"]) {
        d.git = Some(g.to_string());
    }
    if let Some(r) = toml::get_str(x, &["registry"]) {
        d.registry = Some(r.to_string());
    }
    if let Some(o) = x.get("optional").and_then(Value::as_bool) {
        d.optional = o;
    }
    if let Some(f) = x.get("default-features").or_else(|| x.get("default_features")).and_then(Value::as_bool) {
        d.default_features = f;
    }
    d.features.extend(x.get("features").map(Value::str_list).unwrap_or_default());
}
