//! Proc-macro dependencies, until they can run (mizchi/zust#29).
//!
//! A proc-macro crate is built as a stub library of its name, so a crate that only re-exports it (`pub use
//! thiserror_impl::*;`, serde's `derive` feature) still builds. The stub has, for each function-like macro, a
//! `macro_rules!` of that name that expands to a `compile_error!` naming #29. Derives and attribute macros cannot be
//! written that way. zustc would drop an unknown attribute silently, so a crate of the build that uses one of them
//! (or one of the derives) is an error before anything is built. The check reads each crate as zustc does (its module
//! tree, its cfgs evaluated) by zustc's tokens, and follows `use .. as` renames.

/// The macros a proc-macro crate defines (read from its root file)
#[derive(Debug, Default, PartialEq)]
pub struct Macros {
    pub derives: Vec<String>,
    pub attributes: Vec<String>,
    pub functions: Vec<String>,
}

/// Tokens of a source (zustc's lexer: comments skipped, string literals one token), with their text
struct Toks<'a> {
    s: &'a [u8],
    t: Vec<(u8, usize, usize)>,
}

impl<'a> Toks<'a> {
    fn new(src: &'a str) -> Toks<'a> {
        Toks { s: src.as_bytes(), t: zustc::lexer::tokenize(src.as_bytes()) }
    }
    fn text(&self, k: usize) -> &str {
        match self.t.get(k) {
            Some(&(_, a, b)) => std::str::from_utf8(&self.s[a..b]).unwrap_or(""),
            None => "",
        }
    }
    fn ident(&self, k: usize) -> bool {
        self.t.get(k).is_some_and(|x| x.0 == 0)
    }
    /// An identifier at k, also a raw one (`r#type`): (its text, the token after it)
    fn name(&self, k: usize) -> Option<(String, usize)> {
        if self.text(k) == "r" && self.text(k + 1) == "#" && self.ident(k + 2) {
            return Some((format!("r#{}", self.text(k + 2)), k + 3));
        }
        self.ident(k).then(|| (self.text(k).to_string(), k + 1))
    }
    /// A path at k (`a::b::c`, `::a`)
    fn path(&self, mut k: usize) -> Option<Path> {
        if self.text(k) == ":" && self.text(k + 1) == ":" {
            k += 2;
        }
        let (first, mut j) = self.name(k)?;
        let (mut last, mut segs) = (first.clone(), 1);
        while self.text(j) == ":" && self.text(j + 1) == ":" {
            let (n, jj) = self.name(j + 2)?;
            last = n;
            j = jj;
            segs += 1;
        }
        Some(Path { first, last, after: j, segs })
    }
    /// The bracket that closes the one at k
    fn close_of(&self, k: usize) -> usize {
        let mut depth = 0usize;
        let mut j = k;
        while j < self.t.len() {
            if self.t[j].0 == 1 {
                match self.text(j) {
                    "(" | "[" | "{" => depth += 1,
                    ")" | "]" | "}" => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return j;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        self.t.len()
    }
    /// Whether the outer attributes just before the token at k have `#[macro_export]`
    fn exported(&self, k: usize) -> bool {
        let mut j = k;
        while j >= 3 && self.text(j - 1) == "]" {
            // (back to its `#[`)
            let mut depth = 0usize;
            let mut o = j - 1;
            loop {
                if self.t[o].0 == 1 {
                    match self.text(o) {
                        ")" | "]" | "}" => depth += 1,
                        "(" | "[" | "{" => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                }
                if depth == 0 || o == 0 {
                    break;
                }
                o -= 1;
            }
            if o == 0 || self.text(o - 1) != "#" {
                return false;
            }
            if self.text(o + 1) == "macro_export" {
                return true;
            }
            j = o - 1;
        }
        false
    }
}

/// A path's first and last names, the token after it, and how many names it has
struct Path {
    first: String,
    last: String,
    after: usize,
    segs: usize,
}

/// A name without `r#` (`r#route` and `route` are one identifier)
fn plain(s: &str) -> &str {
    s.strip_prefix("r#").unwrap_or(s)
}

/// A path into std (`std::fmt::Debug`, `core::prelude::v1::test`): never a proc macro's
fn std_root(s: &str) -> bool {
    matches!(s, "std" | "core" | "alloc")
}

/// `#[proc_macro_derive(Name, ..)]`, `#[proc_macro_attribute] pub fn name`, `#[proc_macro] pub fn name` (src: the root
/// file with its cfgs evaluated, so `#[cfg_attr(p, proc_macro_attribute)]` counts when p holds)
pub fn macros_of(src: &str) -> Macros {
    let tk = Toks::new(src);
    let mut m = Macros::default();
    let mut k = 0;
    while k < tk.t.len() {
        if !(tk.text(k) == "#" && tk.text(k + 1) == "[") {
            k += 1;
            continue;
        }
        let kind = tk.text(k + 2).to_string();
        let list = match kind.as_str() {
            "proc_macro_derive" if tk.text(k + 3) == "(" => {
                if let Some((n, _)) = tk.name(k + 4) {
                    if !m.derives.contains(&n) {
                        m.derives.push(n);
                    }
                }
                k += 4;
                continue;
            }
            "proc_macro_attribute" => &mut m.attributes,
            "proc_macro" => &mut m.functions,
            _ => {
                k += 2;
                continue;
            }
        };
        // (the name after the next `fn`)
        let mut j = k + 3;
        while j < tk.t.len() && !(tk.ident(j) && tk.text(j) == "fn") {
            j += 1;
        }
        if let Some((n, _)) = tk.name(j + 1) {
            if !list.contains(&n) {
                list.push(n);
            }
        }
        k = j + 1;
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

/// Derives of the language: a proc macro's only when the crate imports one of that name (`use derive_more::Debug;`)
const BUILTIN_DERIVES: &[&str] = &["Clone", "Copy", "Debug", "Default", "Eq", "Hash", "Ord", "PartialEq", "PartialOrd"];

/// What a crate's source uses: derives (`#[derive(.., Name, ..)]`, also `path::Name`), attributes (`#[name ..]`,
/// `#[path::name ..]`), and renames (`use a::Name as Other;`: (Name, Other)). Paths into std are not counted
#[derive(Debug, Default, PartialEq)]
pub struct Uses {
    pub derives: Vec<String>,
    pub attrs: Vec<String>,
    pub renames: Vec<(String, String)>,
}

impl Uses {
    /// The first of a proc-macro crate's derives or attributes this source uses, also under another name (renames
    /// followed to their end)
    pub fn hit(&self, m: &Macros) -> Option<String> {
        let named = |n: &String, list: &[String]| {
            let mut names = vec![plain(n).to_string()];
            let mut k = 0;
            while k < names.len() {
                for (orig, alias) in &self.renames {
                    if plain(alias) == names[k] && !names.iter().any(|x| x == plain(orig)) {
                        names.push(plain(orig).to_string());
                    }
                }
                k += 1;
            }
            names.iter().any(|x| list.iter().any(|l| plain(l) == x))
        };
        if let Some(d) = self.derives.iter().find(|d| named(d, &m.derives)) {
            return Some(format!("the derive `{d}`"));
        }
        self.attrs.iter().find(|a| named(a, &m.attributes)).map(|a| format!("the attribute `#[{a}]`"))
    }
}

/// The uses in a crate's source as zustc reads it (its module tree, cfgs evaluated: srcmap::load_crate). The body
/// of a `macro_rules!` that is neither called in the source nor exported is not read: it expands to nothing
pub fn uses_in(src: &str) -> Uses {
    let tk = Toks::new(src);
    let n = tk.t.len();
    let mut u = Uses::default();
    // (first: what `use` brings in from outside std (`use a::{B as C, D};`), and the macros called (`name!`))
    let mut imported: Vec<String> = vec![];
    let mut called: Vec<String> = vec![];
    for k in 0..n {
        if tk.ident(k) && tk.text(k + 1) == "!" && tk.text(k) != "macro_rules" {
            let c = plain(tk.text(k)).to_string();
            if !called.contains(&c) {
                called.push(c);
            }
        }
        if !(tk.ident(k) && tk.text(k) == "use") {
            continue;
        }
        let mut j = k + 1;
        if tk.text(j) == ":" && tk.text(j + 1) == ":" {
            j += 2;
        }
        if std_root(tk.text(j)) {
            continue;
        }
        while j < n && tk.text(j) != ";" {
            if let Some((name, after)) = tk.name(j).filter(|_| tk.ident(j)) {
                if tk.text(after) == "as" {
                    if let Some((alias, aa)) = tk.name(after + 1) {
                        if !u.renames.contains(&(name.clone(), alias.clone())) {
                            u.renames.push((name, alias));
                        }
                        j = aa;
                        continue;
                    }
                }
                if matches!(tk.text(after), "," | ";" | "}") && !imported.iter().any(|x| x == plain(&name)) {
                    imported.push(plain(&name).to_string());
                }
                j = after;
                continue;
            }
            j += 1;
        }
    }
    let mut k = 0;
    while k < n {
        // (`macro_rules! name { .. }` neither called nor exported)
        if tk.text(k) == "macro_rules" && tk.text(k + 1) == "!" && matches!(tk.text(k + 3), "{" | "(" | "[") {
            if let Some((name, _)) = tk.name(k + 2) {
                if !called.iter().any(|c| c == plain(&name)) && !tk.exported(k) {
                    k = tk.close_of(k + 3) + 1;
                    continue;
                }
            }
        }
        if !(tk.text(k) == "#" && tk.text(k + 1) == "[") {
            k += 1;
            continue;
        }
        let Some(p) = tk.path(k + 2) else {
            k += 2;
            continue;
        };
        if p.last == "derive" && p.segs == 1 && tk.text(p.after) == "(" {
            let mut j = p.after + 1;
            while j < n && tk.text(j) != ")" {
                match tk.path(j) {
                    Some(d) => {
                        let counts = if d.segs > 1 { !std_root(&d.first) } else { !BUILTIN_DERIVES.contains(&plain(&d.last)) || imported.iter().any(|x| x == plain(&d.last)) };
                        if counts && !u.derives.contains(&d.last) {
                            u.derives.push(d.last);
                        }
                        j = d.after;
                    }
                    None => j += 1,
                }
            }
            k = j;
            continue;
        }
        let counts = if p.segs > 1 { !std_root(&p.first) } else { !BUILTIN_ATTRS.contains(&p.last.as_str()) };
        if counts && !u.attrs.contains(&p.last) {
            u.attrs.push(p.last);
        }
        k = p.after;
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_macros_and_their_uses() {
        let src = "/// #[proc_macro_attribute] in a doc comment\n#[proc_macro_derive(Error, attributes(error, from))]\npub fn derive_error(i: TokenStream) -> TokenStream { i }\n#[proc_macro_attribute]\npub fn route(a: TokenStream, i: TokenStream) -> TokenStream { i }\n#[proc_macro]\npub fn\n    html(i: TokenStream) -> TokenStream { i }\n# [proc_macro]\npub fn r#type(i: TokenStream) -> TokenStream { i }\n";
        let m = macros_of(src);
        assert_eq!(m, Macros { derives: vec!["Error".into()], attributes: vec!["route".into()], functions: vec!["html".into(), "r#type".into()] });
        assert!(stub_text("web", &m).contains("macro_rules! html"));
        let u = uses_in(
            "#[derive(Debug, thiserror::Error)]\nstruct E;\n// #[derive(Fake)]\nconst S: &str = r#\"#[web::route] \"quoted\"\"#;\n#[test]\nfn t() {}\n# [ web :: route ( \"/\" ) ]\nfn r() {}\n#[inline]\nfn i() {}\nuse web::route as get;\n#[get]\nfn g() {}\n#[derive (Clone)]\nstruct C;\n",
        );
        // (`Debug` and `Clone` are the language's: no crate imports one of those names)
        assert_eq!(u.derives, ["Error"]);
        assert_eq!(u.attrs, ["route", "get"]);
        assert_eq!(u.renames, [("route".to_string(), "get".to_string())]);
        assert_eq!(u.hit(&m).as_deref(), Some("the derive `Error`"));
        let only_alias = uses_in("use web::route as get;\n#[get]\nfn g() {}\n");
        assert_eq!(only_alias.hit(&m).as_deref(), Some("the attribute `#[get]`"));
        assert_eq!(uses_in("#[derive(Debug)]\nstruct P;\n").hit(&m), None);
        // (a quote inside a raw string before `#[`: the string goes on to its `"##`)
        assert_eq!(uses_in("const EX: &str = r##\"example \"#[pm::traced]\"##;\nfn f() {}\n"), Uses::default());
        // (a derive under another name too: `use web::Error as WebError;`)
        let derive_alias = uses_in("use web::Error as WebError;\n#[derive(Debug, WebError)]\nstruct E;\n");
        assert_eq!(derive_alias.hit(&m).as_deref(), Some("the derive `WebError`"));
        // (`r#route` is `route`; renames are followed to their end)
        assert_eq!(uses_in("#[web::r#route]\nfn f() {}\n").hit(&m).as_deref(), Some("the attribute `#[r#route]`"));
        let chained = uses_in("use web::route as first;\nuse crate::first as second;\n#[second]\nfn f() {}\n");
        assert_eq!(chained.hit(&m).as_deref(), Some("the attribute `#[second]`"));
        assert_eq!(uses_in("use a::b as c;\nuse a::c as b;\n#[b]\nfn f() {}\n").hit(&m), None);
        // (a derive of the language's name: a proc macro's only when imported from outside std)
        let dm = Macros { derives: vec!["Debug".into()], ..Default::default() };
        assert_eq!(uses_in("use derive_more::Debug;\n#[derive(Debug)]\nstruct P;\n").hit(&dm).as_deref(), Some("the derive `Debug`"));
        assert_eq!(uses_in("use derive_more::{Display, Debug};\n#[derive(Debug)]\nstruct P;\n").hit(&dm).as_deref(), Some("the derive `Debug`"));
        assert_eq!(uses_in("use std::fmt::Debug;\n#[derive(Debug, std::fmt::Debug)]\nstruct P;\n").hit(&dm), None);
        assert_eq!(uses_in("#[derive(derive_more::Debug)]\nstruct P;\n").hit(&dm).as_deref(), Some("the derive `Debug`"));
        assert_eq!(uses_in("#[core::prelude::v1::test]\nfn t() {}\n"), Uses::default());
        // (a macro_rules! body: read when the macro is called or exported)
        let tmpl = "macro_rules! m {\n    () => {\n        #[web::route]\n        fn f() {}\n    };\n}\n";
        assert_eq!(uses_in(&format!("{tmpl}fn main() {{}}\n")).hit(&m), None);
        assert_eq!(uses_in(&format!("{tmpl}m!();\n")).hit(&m).as_deref(), Some("the attribute `#[route]`"));
        assert_eq!(uses_in(&format!("#[macro_export]\n#[doc(hidden)]\n{tmpl}")).hit(&m).as_deref(), Some("the attribute `#[route]`"));
        // (a declaration under `cfg_attr`: macros_of reads the root with its cfgs evaluated)
        let gated = "#[cfg_attr(feature = \"on\", proc_macro_attribute)]\npub fn gated(a: TokenStream, i: TokenStream) -> TokenStream { i }\n#[cfg_attr(feature = \"off\", proc_macro_attribute)]\npub fn hidden(a: TokenStream, i: TokenStream) -> TokenStream { i }\n";
        let c = zustc::cfg::CfgSet { features: vec!["on".into()], ..Default::default() };
        assert_eq!(macros_of(&zustc::cfg::strip(gated, &c).unwrap()).attributes, ["gated"]);
    }
}
