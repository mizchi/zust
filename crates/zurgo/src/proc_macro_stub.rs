//! Proc-macro dependencies, until they can run (mizchi/zust#29).
//!
//! A proc-macro crate is built as a stub library of its name, so a crate that only re-exports it (`pub use
//! thiserror_impl::*;`, serde's `derive` feature) still builds. The stub has, for each function-like macro, a
//! `macro_rules!` of that name that expands to a `compile_error!` naming #29. Derives and attribute macros cannot be
//! written that way. zustc would drop an unknown attribute silently, so a crate of the build that uses one of them
//! (or one of the derives) is an error before anything is built.

use std::path::{Path, PathBuf};

/// The macros a proc-macro crate defines (read from its root file)
#[derive(Debug, Default, PartialEq)]
pub struct Macros {
    pub derives: Vec<String>,
    pub attributes: Vec<String>,
    pub functions: Vec<String>,
}

/// `#[proc_macro_derive(Name, ..)]`, `#[proc_macro_attribute] pub fn name`, `#[proc_macro] pub fn name`
pub fn macros_of(src: &str) -> Macros {
    let src = strip_comments(src);
    let mut m = Macros::default();
    let mut rest = src.as_str();
    while let Some(i) = rest.find("#[proc_macro") {
        rest = &rest[i + "#[proc_macro".len()..];
        if let Some(r) = rest.strip_prefix("_derive(") {
            let name: String = r.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !name.is_empty() && !m.derives.contains(&name) {
                m.derives.push(name);
            }
            continue;
        }
        let list = if rest.starts_with("_attribute]") {
            &mut m.attributes
        } else if rest.starts_with(']') {
            &mut m.functions
        } else {
            continue;
        };
        // (the name of the next fn)
        if let Some(k) = rest.find("fn ") {
            let name: String = rest[k + 3..].trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if !name.is_empty() && !list.contains(&name) {
                list.push(name);
            }
        }
    }
    m
}

/// The stub library: a `compile_error!` macro for each function-like macro
pub fn stub_text(pkg: &str, m: &Macros) -> String {
    let mut s = format!("// (zurgo: `{pkg}` is a proc-macro crate, which cannot run yet: mizchi/zust#29)\n");
    for f in &m.functions {
        s.push_str(&format!(
            "#[macro_export]\nmacro_rules! {f} {{\n    ($($t:tt)*) => {{\n        compile_error!(\"`{f}!` is a proc macro of `{pkg}`, which cannot run yet (mizchi/zust#29)\")\n    }};\n}}\n"
        ));
    }
    s
}

/// Attributes of the language and of the tools, never a proc macro's when written without a path
const BUILTIN_ATTRS: &[&str] = &[
    "allow", "automatically_derived", "bench", "cfg", "cfg_attr", "clippy", "cold", "coverage", "crate_name", "crate_type", "deny", "deprecated", "derive",
    "diagnostic", "doc", "expect", "export_name", "feature", "forbid", "global_allocator", "ignore", "inline", "instruction_set", "link", "link_name",
    "link_section", "macro_export", "macro_use", "must_use", "naked", "no_main", "no_mangle", "no_std", "non_exhaustive", "panic_handler", "path",
    "proc_macro", "proc_macro_attribute", "proc_macro_derive", "recursion_limit", "repr", "rustfmt", "should_panic", "target_feature", "test", "test_case",
    "track_caller", "type_length_limit", "unsafe", "used", "warn", "windows_subsystem",
];

/// Derives (`#[derive(.., Name, ..)]`, also `path::Name`) and attributes (`#[name ..]`, `#[path::name ..]`) used in the
/// .rs files under dir
pub fn uses(dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut files = vec![];
    rs_files(dir, &mut files);
    let mut derives: Vec<String> = vec![];
    let mut attrs: Vec<String> = vec![];
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let text = strip_comments(&text);
        let mut rest = text.as_str();
        while let Some(i) = rest.find("#[") {
            rest = &rest[i + 2..];
            let body = rest.trim_start();
            if let Some(inner) = body.strip_prefix("derive(") {
                let list = inner.split(')').next().unwrap_or("");
                for d in list.split(',') {
                    let last = d.trim().rsplit("::").next().unwrap_or("").trim().to_string();
                    if !last.is_empty() && !derives.contains(&last) {
                        derives.push(last);
                    }
                }
                continue;
            }
            // (the attribute's path: `name` or `a::b::name`)
            let path: String = body.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':').collect();
            let qualified = path.contains("::");
            let last = path.rsplit("::").next().unwrap_or("").to_string();
            if last.is_empty() || !qualified && BUILTIN_ATTRS.contains(&last.as_str()) {
                continue;
            }
            if !attrs.contains(&last) {
                attrs.push(last);
            }
        }
    }
    (derives, attrs)
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut es: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    es.sort();
    for e in es {
        if e.is_dir() {
            rs_files(&e, out);
        } else if e.extension().is_some_and(|x| x == "rs") {
            out.push(e);
        }
    }
}

/// The source without comments (`//` to the end of the line, `/* .. */` nested) and string literals (their text
/// becomes `""`): what a scan for attributes must not read
fn strip_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if b[i] == b'"' {
            // (a raw string `r#".."#` is read as a plain one: the scan only needs to skip its text)
            out.push_str("\"\"");
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
        } else if b[i] == b'\'' && b.get(i + 2) == Some(&b'\'') {
            // (a char literal such as '#' or '[')
            out.push_str("' '");
            i += 3;
        } else {
            let st = i;
            i += 1;
            while i < b.len() && !matches!(b[i], b'/' | b'"' | b'\'') {
                i += 1;
            }
            out.push_str(&s[st..i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_macros_and_their_uses() {
        let src = "/// #[proc_macro_attribute] in a doc comment\n#[proc_macro_derive(Error, attributes(error, from))]\npub fn derive_error(i: TokenStream) -> TokenStream { i }\n#[proc_macro_attribute]\npub fn route(a: TokenStream, i: TokenStream) -> TokenStream { i }\n#[proc_macro]\npub fn html(i: TokenStream) -> TokenStream { i }\n";
        let m = macros_of(src);
        assert_eq!(m, Macros { derives: vec!["Error".into()], attributes: vec!["route".into()], functions: vec!["html".into()] });
        assert!(stub_text("web", &m).contains("macro_rules! html"));
        let d = std::env::temp_dir().join(format!("zurgo-pm-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.rs"), "#[derive(Debug, thiserror::Error)]\nstruct E;\n// #[derive(Fake)]\nconst S: &str = \"#[web::route]\";\n#[test]\nfn t() {}\n").unwrap();
        std::fs::write(d.join("sub/b.rs"), "#[web::route(\"/\")]\nfn r() {}\n#[inline]\nfn i() {}\n").unwrap();
        let (derives, attrs) = uses(&d);
        assert_eq!(derives, ["Debug", "Error"]);
        assert_eq!(attrs, ["route"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
