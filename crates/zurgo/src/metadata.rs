//! `zurgo metadata`: the JSON of `cargo metadata --format-version 1`, for the fields tools read (packages with their
//! targets, dependencies and features; the resolve graph with enabled features and dependency kinds).

use crate::manifest::{Dep, Package};
use crate::platform::Target;
use crate::resolve::{Graph, Resolved};
use crate::toml::Value;
use std::collections::BTreeMap;
use std::fmt::Write;

/// A JSON value, written with the keys in the order given
pub enum J {
    Null,
    Bool(bool),
    Num(i64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(&'static str, J)>),
    /// (an object whose keys are data, such as `features`)
    Map(Vec<(String, J)>),
}

fn s(x: &str) -> J {
    J::Str(x.to_string())
}

fn opt(x: Option<&str>) -> J {
    x.map(s).unwrap_or(J::Null)
}

fn esc(out: &mut String, x: &str) {
    out.push('"');
    for c in x.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl J {
    pub fn write(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) => {
                let _ = write!(out, "{n}");
            }
            J::Str(x) => esc(out, x),
            J::Arr(v) => {
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            J::Obj(v) => {
                out.push('{');
                for (i, (k, x)) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    esc(out, k);
                    out.push(':');
                    x.write(out);
                }
                out.push('}');
            }
            J::Map(v) => {
                out.push('{');
                for (i, (k, x)) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    esc(out, k);
                    out.push(':');
                    x.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// The name a dependent uses for a dependency (`use name::..`): the rename, or the dependency's library name
pub fn extern_name(d: &Dep, to: &Package) -> String {
    if d.renamed() {
        d.name.replace('-', "_")
    } else {
        to.lib.as_ref().map(|l| l.name.clone()).unwrap_or_else(|| to.name.replace('-', "_"))
    }
}

fn req_json(r: &str) -> String {
    // (cargo writes a bare version as the caret requirement it means)
    if r.starts_with(|c: char| c.is_ascii_digit()) { format!("^{r}") } else { r.to_string() }
}

fn package(g: &Graph, p: usize) -> J {
    let pk = &g.pkgs[p];
    let m = &pk.m;
    let meta = |k: &str| opt(m.meta.get(k).and_then(Value::as_str));
    let list = |k: &str| J::Arr(m.meta.get(k).map(Value::str_list).unwrap_or_default().iter().map(|x| s(x)).collect());
    let deps = m
        .deps
        .iter()
        .map(|d| {
            let source = if d.path.is_some() { None } else { Some(crate::lock::CRATES_IO) };
            J::Obj(vec![
                ("name", s(&d.package)),
                ("source", opt(source)),
                ("req", s(&req_json(&d.req))),
                ("kind", opt(d.kind.json())),
                ("rename", if d.renamed() { s(&d.name) } else { J::Null }),
                ("optional", J::Bool(d.optional)),
                ("uses_default_features", J::Bool(d.default_features)),
                ("features", J::Arr(d.features.iter().map(|f| s(f)).collect())),
                ("target", opt(d.platform.as_deref())),
                ("registry", opt(d.registry.as_deref())),
            ])
        })
        .collect();
    let mut targets = vec![];
    for t in m.targets() {
        targets.push(J::Obj(vec![
            ("kind", J::Arr(t.kind.iter().map(|k| s(k)).collect())),
            ("crate_types", J::Arr(t.crate_types.iter().map(|k| s(k)).collect())),
            ("name", s(&t.name)),
            ("src_path", s(&t.path.display().to_string())),
            ("edition", s(&t.edition)),
            ("doc", J::Bool(true)),
            ("doctest", J::Bool(t.kind.iter().any(|k| k == "lib" || k == "rlib"))),
            ("test", J::Bool(true)),
        ]));
    }
    if let Some(b) = &m.build {
        targets.push(J::Obj(vec![
            ("kind", J::Arr(vec![s("custom-build")])),
            ("crate_types", J::Arr(vec![s("bin")])),
            ("name", s("build-script-build")),
            ("src_path", s(&b.display().to_string())),
            ("edition", s(&m.edition)),
            ("doc", J::Bool(false)),
            ("doctest", J::Bool(false)),
            ("test", J::Bool(false)),
        ]));
    }
    J::Obj(vec![
        ("name", s(&m.name)),
        ("version", s(&m.version)),
        ("id", s(&pk.id)),
        ("license", meta("license")),
        ("license_file", meta("license-file")),
        ("description", meta("description")),
        ("source", opt(pk.source.as_deref())),
        ("dependencies", J::Arr(deps)),
        ("targets", J::Arr(targets)),
        ("features", J::Map(m.features.iter().map(|(k, v)| (k.clone(), J::Arr(v.iter().map(|x| s(x)).collect()))).collect())),
        ("manifest_path", s(&m.manifest_path.display().to_string())),
        ("metadata", J::Null),
        ("publish", J::Null),
        ("authors", list("authors")),
        ("categories", list("categories")),
        ("keywords", list("keywords")),
        ("readme", meta("readme")),
        ("repository", meta("repository")),
        ("homepage", meta("homepage")),
        ("documentation", meta("documentation")),
        ("edition", s(&m.edition)),
        ("links", opt(m.links.as_deref())),
        ("default_run", J::Null),
        ("rust_version", meta("rust-version")),
    ])
}

/// `filter`: `--filter-platform`. An edge is shown if one of its declarations applies to that platform; then all
/// its declarations are listed in `dep_kinds`, and only packages reached through shown edges are listed
pub fn json(g: &Graph, r: &Resolved, target_dir: &std::path::Path, filter: Option<&Target>) -> String {
    // (per package: the dependency packages shown, with their name and the kinds and platforms of the declarations)
    type Kinds<'g> = Vec<(Option<&'static str>, Option<&'g str>)>;
    let shown = |p: usize| -> BTreeMap<usize, (String, Kinds<'_>)> {
        let mut by: BTreeMap<usize, (String, Kinds, bool)> = BTreeMap::new();
        if let Some(st) = r.nodes.get(&(p, false)) {
            for &(ei, (to, _)) in &st.edges {
                let d = g.decl(p, &g.edges[p][ei]);
                let ent = by.entry(to).or_insert_with(|| (extern_name(d, &g.pkgs[to].m), vec![], false));
                let k = (d.kind.json(), d.platform.as_deref());
                if !ent.1.contains(&k) {
                    ent.1.push(k);
                }
                ent.2 |= match (filter, &d.platform) {
                    (Some(t), Some(pl)) => t.matches(pl).unwrap_or(false),
                    _ => true,
                };
            }
        }
        by.into_iter().filter(|(_, v)| v.2).map(|(k, v)| (k, (v.0, v.1))).collect()
    };
    let mut nodes: BTreeMap<usize, J> = BTreeMap::new();
    let mut todo: Vec<usize> = g.members.clone();
    while let Some(p) = todo.pop() {
        if nodes.contains_key(&p) {
            continue;
        }
        let by = shown(p);
        let mut deps: Vec<(&String, J)> = vec![];
        for (to, (name, mut kinds)) in by {
            todo.push(to);
            kinds.sort();
            let kinds = kinds.iter().map(|(k, t)| J::Obj(vec![("kind", opt(*k)), ("target", opt(*t))])).collect();
            deps.push((&g.pkgs[to].id, J::Obj(vec![("name", s(&name)), ("pkg", s(&g.pkgs[to].id)), ("dep_kinds", J::Arr(kinds))])));
        }
        deps.sort_by(|a, b| a.0.cmp(b.0));
        let features: Vec<J> = r.nodes.get(&(p, false)).map(|st| st.features.iter().map(|f| s(f)).collect()).unwrap_or_default();
        nodes.insert(
            p,
            J::Obj(vec![
                ("id", s(&g.pkgs[p].id)),
                ("dependencies", J::Arr(deps.iter().map(|d| s(d.0)).collect())),
                ("deps", J::Arr(deps.into_iter().map(|d| d.1).collect())),
                ("features", J::Arr(features)),
            ]),
        );
    }
    let mut ids: Vec<usize> = nodes.keys().copied().collect();
    ids.sort_by(|a, b| g.pkgs[*a].id.cmp(&g.pkgs[*b].id));
    let members = J::Arr(g.members.iter().map(|&m| s(&g.pkgs[m].id)).collect());
    let out = J::Obj(vec![
        ("packages", J::Arr(ids.iter().map(|&p| package(g, p)).collect())),
        ("workspace_members", members),
        ("workspace_default_members", J::Arr(g.members.iter().map(|&m| s(&g.pkgs[m].id)).collect())),
        (
            "resolve",
            J::Obj(vec![
                ("nodes", J::Arr(ids.iter().filter_map(|p| nodes.remove(p)).collect())),
                ("root", g.root.map(|r| s(&g.pkgs[r].id)).unwrap_or(J::Null)),
            ]),
        ),
        ("target_directory", s(&target_dir.display().to_string())),
        ("version", J::Num(1)),
        ("workspace_root", s(&g.ws_dir.display().to_string())),
        ("metadata", J::Null),
    ]);
    let mut text = String::new();
    out.write(&mut text);
    text
}
