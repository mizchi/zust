//! E0038: a `dyn Trait` of a trait of the program's that is not dyn compatible, as rustc 1.100 reads it. A trait is
//! not, when:
//! - it requires `Self: Sized` (a supertrait `Sized`, or one of std's that does: `Clone`, `Copy`, `Default`, ..), or a
//!   supertrait of its own that is not dyn compatible;
//! - a supertrait names `Self` in its arguments (`trait Baz: Bar<Self>`), as `PartialEq` / `Eq` / `PartialOrd` / `Ord`
//!   do by default;
//! - it has an associated const, a generic associated type, or an associated type whose bounds name `Self`;
//! - it has a method (not exempted by `where Self: Sized`) without a receiver, with type or const parameters, with
//!   `Self` in a parameter or the result, or that is `async` / returns `impl Trait`.
//! Only the program's own traits are judged, and a name two of its traits share is left alone.

use crate::parse::{Bound, GArg, GParam, Generics, ItemKind, Path, Ty};
use std::collections::HashMap;

/// std traits that require `Self: Sized` or name `Self` as an argument: a trait with one as a supertrait is not dyn
/// compatible
const STD_INCOMPATIBLE: &[&str] = &["Sized", "Clone", "Copy", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "From", "Into", "FromStr"];

/// Whether a path names `Self` itself (`Self`, `Bar<Self>`), not a projection of it (`Self::Item`)
fn path_names_self(p: &Path) -> bool {
    (p.segs.len() == 1 && p.segs[0].name == "Self")
        || p.segs.iter().any(|s| {
            s.args.iter().any(|a| match a {
                GArg::Ty(t) | GArg::Bind(_, t) => ty_names_self(t),
                _ => false,
            })
                || s.fn_args.as_ref().is_some_and(|(xs, r)| xs.iter().any(ty_names_self) || r.as_deref().is_some_and(ty_names_self))
        })
}

fn ty_names_self(t: &Ty) -> bool {
    match t {
        // (`<Self as Tr>::Item` is a projection: only its own arguments count)
        Ty::Path(Some(_), p) => p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Ty(t) | GArg::Bind(_, t) if ty_names_self(t)))),
        Ty::Path(None, p) => path_names_self(p),
        Ty::Ref(_, _, i) | Ty::Ptr(_, i) | Ty::Slice(i) | Ty::Array(i, _) => ty_names_self(i),
        Ty::Tuple(ts) => ts.iter().any(ty_names_self),
        Ty::Fn(ps, r) => ps.iter().any(ty_names_self) || ty_names_self(r),
        Ty::Impl(bs) | Ty::Dyn(bs) => bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if path_names_self(p))),
        _ => false,
    }
}

fn last(p: &Path) -> &str {
    p.segs.last().map_or("", |s| s.name.as_str())
}

/// `where Self: Sized` among a fn's or trait's predicates, or `Self:` a trait of the program's that requires it
fn self_sized(g: &Generics, sized: &[String]) -> bool {
    g.preds.iter().any(|(t, bs)| {
        matches!(t, Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "Self")
            && bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if last(p) == "Sized" || sized.iter().any(|s| s == last(p))))
    })
}

/// Whether a trait's own definition makes it dyn incompatible, and the supertraits of the program's it names
fn own_reason(g: &Generics, supers: &[Bound], members: &[crate::parse::Item], sized: &[String]) -> (bool, Vec<String>) {
    let mut named = vec![];
    if self_sized(g, sized) {
        return (true, named);
    }
    for b in supers {
        let Bound::Trait(_, p) = b else { continue };
        let n = last(p);
        if STD_INCOMPATIBLE.contains(&n) || path_names_self(p) {
            return (true, named);
        }
        named.push(n.to_string());
    }
    for m in members {
        match &m.kind {
            ItemKind::Const(..) => return (true, named),
            ItemKind::Type(_, tg, bs, _) => {
                if !self_sized(tg, sized) && (!tg.params.is_empty() || bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if path_names_self(p)))) {
                    return (true, named);
                }
            }
            ItemKind::Fn(sig, _) => {
                if self_sized(&sig.generics, sized) {
                    continue;
                }
                let generic = sig.generics.params.iter().any(|p| !matches!(p, GParam::Lifetime(_)));
                let impl_ret = sig.ret.as_ref().is_some_and(|r| matches!(r, Ty::Impl(_)));
                if sig.recv.is_none()
                    || generic
                    || sig.quals.iter().any(|q| q == "async")
                    || impl_ret
                    || sig.params.iter().any(|(_, t)| ty_names_self(t))
                    || sig.ret.as_ref().is_some_and(ty_names_self)
                {
                    return (true, named);
                }
            }
            _ => {}
        }
    }
    (false, named)
}

/// The first `dyn Trait` of a trait of the program's that is not dyn compatible: `line:1: msg`. `src` is the
/// prepared text, `items` the program's own items
pub(super) fn check(src: &str, items: &[crate::lexer::SrcItem]) -> Option<String> {
    use crate::lexer::ItemKind as K;
    // (a program with no trait of its own, or no `dyn` in its own items, has nothing to judge: the prepared text holds
    // the prelude too, which is not read)
    if !items.iter().any(|it| it.kind == K::TraitHeader) || !items.iter().any(|it| src.get(it.start..it.end).is_some_and(|t| t.contains("dyn"))) {
        return None;
    }
    // (each trait whole: its header, its members, and the closing brace)
    let mut parsed = vec![];
    for (h, it) in items.iter().enumerate() {
        if it.kind != K::TraitHeader {
            continue;
        }
        let end = items.iter().filter(|m| m.kind == K::TraitMember(h as u32)).map(|m| m.end).max().unwrap_or(it.end);
        let Some(text) = src.get(it.start..end) else { continue };
        let Ok(f) = crate::parse::parse_file(&format!("{text}}}")) else { continue };
        if let Some(item) = f.items.into_iter().next() {
            parsed.push(item);
        }
    }
    // (the traits that require `Sized` of their own: `Self:` one of them exempts a method as `Self: Sized` does)
    let sized: Vec<String> = parsed.iter().filter_map(|i| match &i.kind {
        ItemKind::Trait(n, _, supers, _) if supers.iter().any(|b| matches!(b, Bound::Trait(_, p) if last(p) == "Sized")) => Some(n.clone()),
        _ => None,
    }).collect();
    let mut traits: HashMap<String, Option<(bool, Vec<String>)>> = HashMap::new();
    for i in &parsed {
        let ItemKind::Trait(name, g, supers, members) = &i.kind else { continue };
        let r = own_reason(g, supers, members, &sized);
        // (two traits of one name: neither is judged)
        traits.entry(name.clone()).and_modify(|e| *e = None).or_insert(Some(r));
    }
    if traits.is_empty() {
        return None;
    }
    // (through supertraits of the program's: a fixpoint)
    // (each trait that is not, with the one rustc names: the trait whose own definition makes it so, through its
    // supertraits: `dyn Child` of `trait Child: Super` names `Super`)
    let mut bad: HashMap<&str, &str> = traits.iter().filter_map(|(n, r)| r.as_ref().filter(|r| r.0).map(|_| (n.as_str(), n.as_str()))).collect();
    loop {
        let mut changed = false;
        for (n, r) in &traits {
            let Some((_, sup)) = r else { continue };
            if bad.contains_key(n.as_str()) {
                continue;
            }
            if let Some(c) = sup.iter().find_map(|s| bad.get(s.as_str()).copied()) {
                bad.insert(n.as_str(), c);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    if bad.is_empty() {
        return None;
    }
    // (a `dyn` and the path after it, in the program's items)
    for it in items {
        let Some(text) = src.get(it.start..it.end) else { continue };
        if !text.contains("dyn") {
            continue;
        }
        let b = text.as_bytes();
        let toks = crate::lexer::tokenize(b);
        let tk = |k: usize| &text[toks[k].1..toks[k].2];
        for k in 0..toks.len() {
            if toks[k].0 != 0 || tk(k) != "dyn" {
                continue;
            }
            // (`dyn a::b::Name`: the last segment)
            let mut j = k + 1;
            let mut name = None;
            while j < toks.len() && toks[j].0 == 0 {
                name = Some((tk(j), toks[j].1));
                if j + 2 < toks.len() && tk(j + 1) == ":" && tk(j + 2) == ":" {
                    j += 3;
                } else {
                    break;
                }
            }
            let Some((n, npos)) = name else { continue };
            if let Some(c) = bad.get(n) {
                let line = it.line + b[..toks[k].1].iter().filter(|&&c| c == b'\n').count();
                // (at the `dyn`, as rustc; at the trait's name when a supertrait is the one that is not)
                let at = it.start + if *c == n { toks[k].1 } else { npos };
                let col = at - src[..at].rfind('\n').map_or(0, |p| p + 1) + 1;
                return Some(format!("{line}:{col}: the trait `{c}` is not dyn compatible (E0038)"));
            }
        }
    }
    None
}
