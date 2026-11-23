//! Proc macros on the proc_macro API alone (no syn): what zustc hands them and places their output where rustc does
extern crate proc_macro;
use proc_macro::{Delimiter, TokenStream, TokenTree};

/// The name after the first `fn` / `struct` / `enum`
fn item_name(item: &TokenStream) -> String {
    let mut it = item.clone().into_iter();
    while let Some(t) = it.next() {
        if let TokenTree::Ident(i) = &t {
            if matches!(i.to_string().as_str(), "fn" | "struct" | "enum") {
                if let Some(TokenTree::Ident(n)) = it.next() {
                    return n.to_string();
                }
            }
        }
    }
    panic!("no item name in `{item}`")
}

/// `#[scaled(k)] fn f() -> u32 { .. }`: f as it is, and `f_scaled()`, k times f
#[proc_macro_attribute]
pub fn scaled(attr: TokenStream, item: TokenStream) -> TokenStream {
    let k = attr.to_string();
    let n = item_name(&item);
    format!("{item} fn {n}_scaled() -> u32 {{ {n}() * {k} }}").parse().unwrap()
}

/// `#[rules] fn f() { .. }`: f as it is, and `const RULES_<NAME>: usize` with the number of `macro_rules!` in it
#[proc_macro_attribute]
pub fn rules(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let n = item.to_string().matches("macro_rules").count();
    format!("{item} const RULES_{}: usize = {n};", item_name(&item).to_uppercase()).parse().unwrap()
}

/// `#[docs] item`: the item, and `const DOCS_<NAME>: usize` with the number of its doc attributes (doc comments come in
/// as `#[doc = ".."]`)
#[proc_macro_attribute]
pub fn docs(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut n = 0;
    let mut prev_pound = false;
    for t in item.clone() {
        match &t {
            TokenTree::Group(g) if prev_pound && g.delimiter() == Delimiter::Bracket => {
                if g.stream().into_iter().next().is_some_and(|x| x.to_string() == "doc") {
                    n += 1;
                }
            }
            _ => {}
        }
        prev_pound = matches!(&t, TokenTree::Punct(p) if p.as_char() == '#');
    }
    let name = item_name(&item).to_uppercase();
    format!("{item} const DOCS_{name}: usize = {n};").parse().unwrap()
}

/// `#[derive(Describe)]`: `fn describe() -> String` with the name and the number of fields (`#[describe(skip)]` on a
/// field leaves it out); it calls `re::label`
#[proc_macro_derive(Describe, attributes(describe))]
pub fn derive_describe(item: TokenStream) -> TokenStream {
    let name = item_name(&item);
    let body = item.into_iter().filter_map(|t| match t {
        TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => Some(g.stream()),
        _ => None,
    });
    let mut fields = 0;
    let mut skip = false;
    for b in body {
        let toks: Vec<TokenTree> = b.into_iter().collect();
        let mut k = 0;
        while k < toks.len() {
            match &toks[k] {
                TokenTree::Punct(p) if p.as_char() == '#' => {
                    if toks.get(k + 1).is_some_and(|g| g.to_string().contains("skip")) {
                        skip = true;
                    }
                    k += 2;
                    continue;
                }
                TokenTree::Punct(p) if p.as_char() == ':' => {
                    if !skip {
                        fields += 1;
                    }
                    skip = false;
                }
                _ => {}
            }
            k += 1;
        }
    }
    format!("impl {name} {{ pub fn describe() -> String {{ ::re::label(\"{name}\", {fields}) }} }}").parse().unwrap()
}

/// `sum!(a, b, ..)`: `(a + b + ..)`
#[proc_macro]
pub fn sum(input: TokenStream) -> TokenStream {
    let parts: Vec<String> = input.to_string().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    format!("({})", parts.join(" + ")).parse().unwrap()
}

/// `make_fn!(name, value)`: `fn name() -> u32 { value }` (an item)
#[proc_macro]
pub fn make_fn(input: TokenStream) -> TokenStream {
    let s = input.to_string();
    let (n, v) = s.split_once(',').unwrap();
    format!("fn {}() -> u32 {{ {} }}", n.trim(), v.trim()).parse().unwrap()
}

/// `pkg!()`: the package that uses it, as the environment of its expansion says (`CARGO_PKG_NAME`)
#[proc_macro]
pub fn pkg(_input: TokenStream) -> TokenStream {
    let name = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    format!("{name:?}").parse().unwrap()
}

/// `shared!()`: a file outside this package (`include_str!`), as a string: rebuilt when it changes
#[proc_macro]
pub fn shared(_input: TokenStream) -> TokenStream {
    format!("{:?}", include_str!("../../shared.txt").trim()).parse().unwrap()
}

/// `add!(a, b)`: `a + b`, not grouped (its place groups it: `pm::add!(1, 2) * 3` is 9)
#[proc_macro]
pub fn add(input: TokenStream) -> TokenStream {
    input.to_string().replace(',', " + ").parse().unwrap()
}
