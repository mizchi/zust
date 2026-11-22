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
    /// A path at k (`a::b::c`, `::a`): its last name and the token after it
    fn path(&self, mut k: usize) -> Option<(String, usize)> {
        if self.text(k) == ":" && self.text(k + 1) == ":" {
            k += 2;
        }
        let (mut last, mut j) = self.name(k)?;
        while self.text(j) == ":" && self.text(j + 1) == ":" {
            let (n, jj) = self.name(j + 2)?;
            last = n;
            j = jj;
        }
        Some((last, j))
    }
}

/// `#[proc_macro_derive(Name, ..)]`, `#[proc_macro_attribute] pub fn name`, `#[proc_macro] pub fn name`
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

/// What a crate's source uses: derives (`#[derive(.., Name, ..)]`, also `path::Name`), attributes (`#[name ..]`,
/// `#[path::name ..]`), and renames (`use a::Name as Other;`: (Name, Other))
#[derive(Debug, Default, PartialEq)]
pub struct Uses {
    pub derives: Vec<String>,
    pub attrs: Vec<String>,
    pub renames: Vec<(String, String)>,
}

impl Uses {
    /// The first of a proc-macro crate's derives or attributes this source uses, also under another name
    pub fn hit(&self, m: &Macros) -> Option<String> {
        let named = |n: &String, list: &[String]| list.contains(n) || self.renames.iter().any(|(orig, alias)| alias == n && list.contains(orig));
        if let Some(d) = self.derives.iter().find(|d| named(d, &m.derives)) {
            return Some(format!("the derive `{d}`"));
        }
        self.attrs.iter().find(|a| named(a, &m.attributes)).map(|a| format!("the attribute `#[{a}]`"))
    }
}

/// The uses in a crate's source as zustc reads it (its module tree, cfgs evaluated: srcmap::load_crate)
pub fn uses_in(src: &str) -> Uses {
    let tk = Toks::new(src);
    let mut u = Uses::default();
    let mut k = 0;
    while k < tk.t.len() {
        // (`use a::B as C;`, also in a group: `use a::{B as C, ..};`)
        if tk.ident(k) && tk.text(k) == "as" && k > 0 {
            if let (Some((orig, _)), Some((alias, _))) = (tk.name(k - 1).filter(|_| tk.ident(k - 1)), tk.name(k + 1)) {
                if !u.renames.contains(&(orig.clone(), alias.clone())) {
                    u.renames.push((orig, alias));
                }
            }
        }
        if !(tk.text(k) == "#" && tk.text(k + 1) == "[") {
            k += 1;
            continue;
        }
        let Some((last, after)) = tk.path(k + 2) else {
            k += 2;
            continue;
        };
        let qualified = tk.text(k + 3) == ":";
        if last == "derive" && !qualified && tk.text(after) == "(" {
            let mut j = after + 1;
            while j < tk.t.len() && tk.text(j) != ")" {
                match tk.path(j) {
                    Some((d, jj)) => {
                        if !u.derives.contains(&d) {
                            u.derives.push(d);
                        }
                        j = jj;
                    }
                    None => j += 1,
                }
            }
            k = j;
            continue;
        }
        if (qualified || !BUILTIN_ATTRS.contains(&last.as_str())) && !u.attrs.contains(&last) {
            u.attrs.push(last);
        }
        k = after;
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
        assert_eq!(u.derives, ["Debug", "Error", "Clone"]);
        assert_eq!(u.attrs, ["route", "get"]);
        assert_eq!(u.renames, [("route".to_string(), "get".to_string())]);
        assert_eq!(u.hit(&m).as_deref(), Some("the derive `Error`"));
        let only_alias = uses_in("use web::route as get;\n#[get]\nfn g() {}\n");
        assert_eq!(only_alias.hit(&m).as_deref(), Some("the attribute `#[get]`"));
        assert_eq!(uses_in("#[derive(Debug)]\nstruct P;\n").hit(&m), None);
        // (a derive under another name too: `use web::Error as WebError;`)
        let derive_alias = uses_in("use web::Error as WebError;\n#[derive(Debug, WebError)]\nstruct E;\n");
        assert_eq!(derive_alias.hit(&m).as_deref(), Some("the derive `WebError`"));
    }
}
