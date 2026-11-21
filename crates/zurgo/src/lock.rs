//! Cargo.lock (versions 1 to 4), and where the sources of locked packages are.

use crate::toml::{self, Value};
use std::path::{Path, PathBuf};

pub const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

#[derive(Clone, Debug, PartialEq)]
pub struct LockDep {
    pub name: String,
    pub version: Option<String>,
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LockPkg {
    pub name: String,
    pub version: String,
    /// `None` for path packages (workspace members and path dependencies)
    pub source: Option<String>,
    pub checksum: Option<String>,
    pub deps: Vec<LockDep>,
}

#[derive(Clone, Debug, Default)]
pub struct Lock {
    pub version: i64,
    pub packages: Vec<LockPkg>,
}

pub fn read(path: &Path) -> Result<Lock, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn parse(text: &str) -> Result<Lock, String> {
    let t = toml::parse(text)?;
    let version = match t.get("version") {
        Some(Value::Integer(v)) => *v,
        _ => 1,
    };
    let mut packages = vec![];
    // (a version 1 lock keeps the root package in `[root]`)
    let root = t.get("root").into_iter();
    for p in root.chain(t.get("package").and_then(Value::as_array).unwrap_or(&[]).iter()) {
        let p = p.as_table().ok_or("`[[package]]` is not a table")?;
        let s = |k: &str| toml::get_str(p, &[k]).map(str::to_string);
        let deps = p
            .get("dependencies")
            .map(Value::str_list)
            .unwrap_or_default()
            .iter()
            .map(|d| {
                // (`name`, `name version`, `name version (source)`)
                let mut it = d.splitn(3, ' ');
                let name = it.next().unwrap_or_default().to_string();
                let version = it.next().map(str::to_string);
                let source = it.next().map(|s| s.trim_start_matches('(').trim_end_matches(')').to_string());
                LockDep { name, version, source }
            })
            .collect();
        packages.push(LockPkg {
            name: s("name").ok_or("a locked package without a name")?,
            version: s("version").ok_or("a locked package without a version")?,
            source: s("source"),
            checksum: s("checksum"),
            deps,
        });
    }
    Ok(Lock { version, packages })
}

impl Lock {
    /// The locked package a dependency entry names
    pub fn find(&self, d: &LockDep) -> Option<usize> {
        let mut it = self.packages.iter().enumerate().filter(|(_, p)| {
            p.name == d.name && d.version.as_ref().is_none_or(|v| *v == p.version) && d.source.as_ref().is_none_or(|s| p.source.as_ref() == Some(s))
        });
        let first = it.next()?.0;
        if it.next().is_some() { None } else { Some(first) }
    }
}

pub fn cargo_home() -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(h) => PathBuf::from(h),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cargo"),
    }
}

/// cargo's directories for crates.io under `registry/src` and `registry/index`: `index.crates.io-<hash>` (the sparse
/// index) and `github.com-<hash>` (the git index). Only these, so that a package of the same name and version from
/// another registry is never taken for the crates.io one
fn crates_io_dirs(under: &Path) -> Vec<PathBuf> {
    let mut ds: Vec<PathBuf> = std::fs::read_dir(under)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|d| d.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("index.crates.io-") || n.starts_with("github.com-")))
        .collect();
    ds.sort();
    ds
}

/// The unpacked sources of a crates.io package (`$CARGO_HOME/registry/src/<index>/<name>-<version>`, where `cargo
/// fetch` and `cargo build` put them)
pub fn registry_dir(name: &str, version: &str) -> Option<PathBuf> {
    crates_io_dirs(&cargo_home().join("registry/src")).into_iter().map(|d| d.join(format!("{name}-{version}"))).find(|d| d.join("Cargo.toml").is_file())
}

/// The registry index's entry for one version of a package, from cargo's cache of the sparse index
/// (`$CARGO_HOME/registry/index/<index>/.cache/<prefix>/<name>`: a header, then `version \0 json \0` pairs). cargo keeps
/// an entry for every package it resolved, also those it did not download (packages for other platforms)
pub fn index_entry(name: &str, version: &str) -> Option<crate::toml::Table> {
    let lower = name.to_lowercase();
    let prefix = match lower.len() {
        1 => "1".to_string(),
        2 => "2".to_string(),
        3 => format!("3/{}", &lower[..1]),
        _ => format!("{}/{}", &lower[..2], &lower[2..4]),
    };
    for d in crates_io_dirs(&cargo_home().join("registry/index")) {
        let Ok(bytes) = std::fs::read(d.join(".cache").join(&prefix).join(&lower)) else { continue };
        let parts: Vec<&[u8]> = bytes.split(|&b| b == 0).collect();
        // (the header and the etag line come first; then the pairs)
        let mut i = 2;
        while i + 1 < parts.len() {
            if parts[i] == version.as_bytes() {
                let v = crate::json::parse(std::str::from_utf8(parts[i + 1]).ok()?).ok()?;
                return v.as_table().cloned();
            }
            i += 2;
        }
    }
    None
}

pub fn is_registry(source: &str) -> bool {
    source == CRATES_IO || source.starts_with("sparse+https://index.crates.io")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockfile() {
        let l = parse(
            r#"
# This file is automatically @generated by Cargo.
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = [
 "rustc-hash 1.1.0",
 "rustc-hash 2.1.3",
 "fnv",
]

[[package]]
name = "fnv"
version = "1.0.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "3f9eec918d3f24069decb9af1554cad7c880e2da24a9afd88aca000531ab82c1"

[[package]]
name = "rustc-hash"
version = "1.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "rustc-hash"
version = "2.1.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
        )
        .unwrap();
        assert_eq!(l.version, 4);
        assert_eq!(l.packages.len(), 4);
        let app = &l.packages[0];
        assert_eq!(app.source, None);
        assert_eq!(app.deps[0], LockDep { name: "rustc-hash".into(), version: Some("1.1.0".into()), source: None });
        assert_eq!(l.find(&app.deps[1]), Some(3));
        assert_eq!(l.find(&app.deps[2]), Some(1));
        assert_eq!(l.find(&LockDep { name: "rustc-hash".into(), version: None, source: None }), None);
    }

    #[test]
    fn version_1_root() {
        let l = parse("[root]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\"fnv 1.0.7 (registry+https://github.com/rust-lang/crates.io-index)\"]\n\n[[package]]\nname = \"fnv\"\nversion = \"1.0.7\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n").unwrap();
        assert_eq!(l.packages.len(), 2);
        assert_eq!(l.packages[0].name, "app");
        assert_eq!(l.find(&l.packages[0].deps[0]), Some(1));
    }
}
