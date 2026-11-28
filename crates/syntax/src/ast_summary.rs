//! Item summaries (front.rs's `summarize` / `parse_sig` / `lift_closures` / `parse_and_summarize`)
//! done on our own syntax tree (src/parse.rs). tests/ast_summary.rs checks that every file gets the same summaries as the syn version.
//! It depends on no external crates (only parse / types / ast_types / ast_closure / lexer), so zustc itself can compile it too.

use crate::ast_closure;
use crate::ast_types::tysyn;
use crate::lexer::{ItemKind as LexKind, SrcItem};
use crate::parse::{Block, Bound, Expr, Fields, FnSig as PSig, GParam, Generics, Item, ItemKind, Pat, Path, Stmt, Ty as PTy, parse_file};
use crate::types::{
    AssocSyn, CONST_GENERIC, ClosureSyn, IMPL_RET, OPAQUE_NESTED, OPAQUE_RET, Owner, SigSyn, Summary, Ty, TySyn, VariantKind, VariantSyn,
};

pub type R<T> = Result<T, String>;

fn err<T>(msg: &str) -> R<T> {
    Err(msg.to_string())
}

/// Attribute text with whitespace removed (`#[derive(Debug,Clone)]`)
fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Whether `#[name]` or `#[unsafe(name)]` is present
fn has_attr(attrs: &[String], name: &str) -> bool {
    attrs.iter().any(|a| {
        let c = compact(a);
        c == format!("#[{name}]") || c.starts_with(&format!("#[{name}(")) || c.starts_with(&format!("#[{name}=")) || c == format!("#[unsafe({name})]")
    })
}

/// The names in `#[derive(A, B)]` (only single-identifier ones)
/// (also as std's prelude writes it, `#[::core::prelude::v1::derive(::core::clone::Clone, ::core::marker::Copy)]`, as
/// libc 0.2's `s!` does: a std trait by its path is its name)
fn derive_names(attrs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in attrs {
        let c = compact(a);
        let Some(rest) = c.strip_prefix("#[") else { continue };
        let rest = rest.strip_prefix("::").unwrap_or(rest);
        let rest = ["core::prelude::v1::", "std::prelude::v1::"].iter().find_map(|p| rest.strip_prefix(p)).unwrap_or(rest);
        let Some(inner) = rest.strip_prefix("derive(").and_then(|x| x.strip_suffix(")]")) else { continue };
        for part in inner.split(',') {
            let p = part.strip_prefix("::").unwrap_or(part);
            let std_path = ["core::", "std::", "alloc::"].iter().any(|r| p.starts_with(r));
            let name = if std_path { p.rsplit("::").next().unwrap_or(p) } else { p };
            if !name.is_empty() && name.chars().all(|ch| ch == '_' || ch.is_alphanumeric()) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The std crate a derive's path names `name` right under, `#[derive(core::name)]` / `#[derive(::std::name)]`
#[cold]
fn derive_root_of(attrs: &[String], name: &str) -> Option<&'static str> {
    for a in attrs {
        let c = compact(a);
        let Some(inner) = c.find("derive(").and_then(|i| c[i + 7..].strip_suffix(")]")) else { continue };
        for part in inner.split(',') {
            let p = part.strip_prefix("::").unwrap_or(part);
            if let Some(k) = ["core", "std", "alloc"].into_iter().find(|k| p.strip_prefix(k).and_then(|r| r.strip_prefix("::")) == Some(name)) {
                return Some(k);
            }
        }
    }
    None
}

/// The integer type of `#[repr(u8)]` / `#[repr(C, i16)]`
fn int_repr(attrs: &[String]) -> Option<String> {
    for a in attrs {
        let c = compact(a);
        let Some(inner) = c.strip_prefix("#[repr(").and_then(|x| x.strip_suffix(")]")) else { continue };
        for part in inner.split(',') {
            if matches!(part, "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "usize" | "isize") {
                return Some(part.to_string());
            }
        }
    }
    None
}

/// `#[repr(packed)]` / `#[repr(packed(N))]` / `#[repr(align(N))]`: "packed:N" / "align:N" (Summary's reprs keeps them
/// beside the integer type of an enum), and "C" for `#[repr(C)]`. N is an unsuffixed integer literal (`16`, `0x10`,
/// `1_024`), a power of two up to 2^29 (E0589)
fn layout_reprs(attrs: &[String]) -> R<Vec<String>> {
    let value = |x: &str| -> R<u32> {
        let x = x.replace('_', "");
        let (digits, radix) = match x.get(..2) {
            Some("0x") => (&x[2..], 16),
            Some("0o") => (&x[2..], 8),
            Some("0b") => (&x[2..], 2),
            _ => (x.as_str(), 10),
        };
        let Ok(n) = u64::from_str_radix(digits, radix) else {
            return err("invalid alignment value: not an unsuffixed integer (E0589)");
        };
        if n > 1 << 29 {
            return err("invalid alignment value: larger than 2^29 (E0589)");
        }
        if !n.is_power_of_two() {
            return err("invalid alignment value: not a power of two (E0589)");
        }
        Ok(n as u32)
    };
    let mut v = vec![];
    for a in attrs {
        let c = compact(a);
        let Some(inner) = c.strip_prefix("#[repr(").and_then(|x| x.strip_suffix(")]")) else { continue };
        for part in inner.split(',') {
            if part == "C" {
                v.push("C".to_string());
            } else if part == "packed" {
                v.push("packed:1".to_string());
            } else if let Some(n) = part.strip_prefix("packed(").and_then(|x| x.strip_suffix(')')) {
                v.push(format!("packed:{}", value(n)?));
            } else if let Some(n) = part.strip_prefix("align(").and_then(|x| x.strip_suffix(')')) {
                v.push(format!("align:{}", value(n)?));
            }
        }
    }
    Ok(v)
}

/// The name in `#[link_name = "malloc"]`
fn link_name(attrs: &[String]) -> Option<String> {
    for a in attrs {
        let c = compact(a);
        if let Some(v) = c.strip_prefix("#[link_name=\"").and_then(|x| x.strip_suffix("\"]")) {
            return Some(v.to_string());
        }
    }
    None
}

fn self_of(name: &str, generics: &[String]) -> TySyn {
    if generics.is_empty() {
        TySyn::Named(name.to_string())
    } else {
        TySyn::App(name.to_string(), generics.iter().map(|g| TySyn::Named(g.clone())).collect())
    }
}

/// A discriminant that is an integer literal, negated or in parentheses / a block (`3`, `-1`, `{ 3 }`)
fn lit_discr(e: &Expr) -> bool {
    match e {
        Expr::Lit(t, _) => t.starts_with(|c: char| c.is_ascii_digit()),
        Expr::Unary(op, x, _) if op == "-" => lit_discr(x),
        Expr::Paren(x) => lit_discr(x),
        Expr::Block(_, _, b) if b.stmts.len() == 1 => matches!(&b.stmts[0], crate::parse::Stmt::Expr(x, false) if lit_discr(x)),
        _ => false,
    }
}

pub fn const_int(e: &Expr) -> R<i64> {
    match e {
        Expr::Lit(t, _) if t.starts_with(|c: char| c.is_ascii_digit()) => {
            let t: String = t.chars().filter(|&c| c != '_').collect();
            let (radix, body) = match t.get(..2) {
                Some("0x") => (16, &t[2..]),
                Some("0o") => (8, &t[2..]),
                Some("0b") => (2, &t[2..]),
                _ => (10, &t[..]),
            };
            // (a type suffix: `5u8`, `0xFFi64`)
            let digits: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
            u64::from_str_radix(&digits, radix).map(|v| v as i64).map_err(|e| e.to_string())
        }
        Expr::Unary(op, x, _) if op == "-" => Ok(const_int(x)?.wrapping_neg()),
        Expr::Unary(op, x, _) if op == "!" => Ok(!const_int(x)?),
        Expr::Paren(x) => const_int(x),
        Expr::Cast(x, _) => const_int(x),
        // (`{ 3 }`)
        Expr::Block(_, _, b) if b.stmts.len() == 1 => match &b.stmts[0] {
            crate::parse::Stmt::Expr(x, false) => const_int(x),
            _ => err("only integer literals are supported as discriminants"),
        },
        Expr::Binary(op, a, b, _) => {
            let (a, b) = (const_int(a)?, const_int(b)?);
            Ok(match op.as_str() {
                "+" => a.wrapping_add(b),
                "-" => a.wrapping_sub(b),
                "*" => a.wrapping_mul(b),
                "/" if b != 0 => a / b,
                "%" if b != 0 => a % b,
                "<<" => a.wrapping_shl(b as u32),
                ">>" => a.wrapping_shr(b as u32),
                "|" => a | b,
                "&" => a & b,
                "^" => a ^ b,
                _ => return err("only integer literals are supported as discriminants"),
            })
        }
        _ => err("only integer literals are supported as discriminants"),
    }
}

/// Bound names (the same rules as front.rs's bound_names)
pub fn bound_names(bs: &[Bound]) -> R<Vec<String>> {
    let mut v = Vec::new();
    for b in bs {
        match b {
            // `?Sized` only relaxes a constraint, so it is not counted
            Bound::Trait(true, _) => {}
            Bound::Trait(false, p) => {
                let last = p.segs.last().ok_or("empty bound")?;
                // (`iter::FromIterator<char>` skips the module part)
                if last.fn_args.is_some() || !last.args.is_empty() {
                    v.push(last.text.clone());
                } else {
                    v.push(last.name.clone());
                }
            }
            Bound::Lifetime(_) => {}
        }
    }
    Ok(v)
}

fn collect_generics(g: &Generics, out: &mut Vec<(String, Vec<String>)>) -> R<()> {
    // (`T: Iterator<Item = impl Foo>`)
    for p in &g.params {
        if let GParam::Ty(_, bs, _) = p {
            for b in bs {
                if let Bound::Trait(_, bp) = b {
                    if bp.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, crate::parse::GArg::Bind(_, PTy::Impl(_)) | crate::parse::GArg::Ty(PTy::Impl(_))))) {
                        return err("`impl Trait` is not allowed in generics");
                    }
                }
            }
        }
    }
    // (E0403 and the parser's checks on the parameter list)
    let mut seen: Vec<&str> = vec![];
    let mut after_ty = false;
    for p in &g.params {
        let n = match p {
            GParam::Lifetime(l) => {
                if after_ty {
                    return err("lifetime parameters must be declared prior to type and const parameters");
                }
                if l == "'self" || l == "'static" || l == "'_" {
                    return Err(format!("lifetimes cannot use keyword names (`{l}`)"));
                }
                l.as_str()
            }
            GParam::Ty(n, _, _) | GParam::Const(n, ..) => {
                after_ty = true;
                // (the type of a const parameter: integers, bool, char)
                if let GParam::Const(_, t, _) = p {
                    let ok = matches!(t, PTy::Path(None, q) if q.segs.len() == 1 && matches!(q.segs[0].name.as_str(),
                        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "bool" | "char"));
                    if !ok {
                        let tn = ty_compact(t);
                        let tn = if tn.is_empty() { "this type".to_string() } else { format!("`{tn}`") };
                        return Err(format!("{tn} is forbidden as the type of a const generic parameter"));
                    }
                }
                if n == "Self" {
                    return err("unexpected keyword `Self` in generic parameters");
                }
                n.as_str()
            }
        };
        if seen.contains(&n) {
            return Err(format!("the name `{n}` is already used for a generic parameter in this item's generic parameters"));
        }
        seen.push(n);
    }
    for p in &g.params {
        match p {
            GParam::Lifetime(_) => {}
            GParam::Ty(n, bs, _) => out.push((n.clone(), bound_names(bs)?)),
            // (`const B: bool`: the parameter's type when it is not usize, as `__const:bool` after the mark)
            GParam::Const(n, t, _) => out.push((n.clone(), match t {
                PTy::Path(None, q) if q.segs.len() == 1 && q.segs[0].name != "usize" => vec![CONST_GENERIC.to_string(), format!("{CONST_GENERIC}:{}", q.segs[0].name)],
                _ => vec![CONST_GENERIC.to_string()],
            })),
        }
    }
    for (t, bs) in &g.preds {
        // (Bounds on anything other than type parameters are skipped)
        let PTy::Path(None, p) = t else { continue };
        if p.global || p.segs.len() != 1 || !p.segs[0].args.is_empty() || p.segs[0].fn_args.is_some() {
            continue;
        }
        // (Lifetime predicates `'a: 'b`)
        if p.segs[0].name.starts_with('\'') {
            continue;
        }
        let id = p.segs[0].name.clone();
        let bs = bound_names(bs)?;
        match out.iter_mut().find(|g| g.0 == id) {
            Some(g) => g.1.extend(bs),
            // (a bound on a concrete type, `where i32: Add<T>`: Program checks that the type exists (where_types))
            None => {}
        }
    }
    Ok(())
}

/// The names of the concrete types bounded in the where clauses of an item (`where u64: Sized` → u64 is not a
/// parameter): Program checks that each names a type. Primitive types are left out
fn where_types(item: &Item, owner: Option<&Owner>, out: &mut Vec<String>) {
    let g = match &item.kind {
        ItemKind::Fn(fs, _) => &fs.generics,
        ItemKind::Struct(_, g, _) | ItemKind::Enum(_, g, _) | ItemKind::Union(_, g, _) | ItemKind::Impl(g, ..) | ItemKind::Trait(_, g, ..) => g,
        _ => return,
    };
    for (t, _) in &g.preds {
        let PTy::Path(None, p) = t else { continue };
        if p.global || p.segs.len() != 1 || !p.segs[0].args.is_empty() || p.segs[0].fn_args.is_some() {
            continue;
        }
        let n = p.segs[0].name.as_str();
        if n.starts_with('\'') || n == "Self" || n == "str" || Ty::prim(n).is_some() || crate::ast_types::ffi_alias(n).is_some() {
            continue;
        }
        let param = g.params.iter().any(|q| matches!(q, GParam::Ty(x, ..) | GParam::Const(x, ..) if x == n));
        if param || owner.is_some_and(|o| o.generics.iter().any(|x| x.0 == n)) || out.iter().any(|x| x == n) {
            continue;
        }
        out.push(n.to_string());
    }
}

/// The plain trait bounds (one-segment paths without arguments, not `?Sized`) on a struct / enum's parameters
/// The type parameters declared `?Sized` (in the parameter list or a where clause)
fn adt_unsized(g: &Generics) -> Vec<String> {
    let maybe = |bs: &[Bound]| bs.iter().any(|b| matches!(b, Bound::Trait(true, p) if p.segs.last().is_some_and(|s| s.name == "Sized")));
    let mut v: Vec<String> = g.params.iter().filter_map(|p| match p {
        GParam::Ty(n, bs, _) if maybe(bs) => Some(n.clone()),
        _ => None,
    }).collect();
    for (t, bs) in &g.preds {
        if let PTy::Path(None, tp) = t {
            if tp.segs.len() == 1 && maybe(bs) && !v.contains(&tp.segs[0].name) {
                v.push(tp.segs[0].name.clone());
            }
        }
    }
    v
}

fn adt_bounds(g: &Generics) -> Vec<(String, Vec<String>)> {
    let mut v = Vec::new();
    for p in &g.params {
        // (`struct Flag<const B: bool>`: a const parameter's declared type when it is not usize, `__const:bool`)
        if let GParam::Const(n, PTy::Path(None, q), _) = p {
            if q.segs.len() == 1 && q.segs[0].name != "usize" {
                v.push((n.clone(), vec![format!("{CONST_GENERIC}:{}", q.segs[0].name)]));
            }
        }
        if let GParam::Ty(n, bs, _) = p {
            let trs: Vec<String> = bs
                .iter()
                .filter_map(|b| match b {
                    Bound::Trait(false, bp) if bp.segs.len() == 1 && bp.segs[0].args.is_empty() && bp.segs[0].fn_args.is_none() => Some(bp.segs[0].name.clone()),
                    _ => None,
                })
                .collect();
            if !trs.is_empty() {
                v.push((n.clone(), trs));
            }
        }
    }
    // (`where T: Tr`: the same, for a parameter named by a one-segment path)
    for (t, bs) in &g.preds {
        let PTy::Path(None, tp) = t else { continue };
        if tp.segs.len() != 1 || !tp.segs[0].args.is_empty() {
            continue;
        }
        let n = &tp.segs[0].name;
        if !g.params.iter().any(|p| matches!(p, GParam::Ty(pn, ..) if pn == n)) {
            continue;
        }
        let trs: Vec<String> = bs
            .iter()
            .filter_map(|b| match b {
                Bound::Trait(false, bp) if bp.segs.len() == 1 && bp.segs[0].args.is_empty() && bp.segs[0].fn_args.is_none() => Some(bp.segs[0].name.clone()),
                _ => None,
            })
            .collect();
        if trs.is_empty() {
            continue;
        }
        match v.iter_mut().find(|x| &x.0 == n) {
            Some(e) => e.1.extend(trs),
            None => v.push((n.clone(), trs)),
        }
    }
    v
}

/// The default of each parameter of adt_params (`F = fn() -> T`; a const parameter's as its argument would be,
/// `const N: usize = DEFAULT` as `[(); DEFAULT]`), by place; empty when none has one
fn adt_defaults(g: &Generics, names: &[String]) -> R<Vec<Option<TySyn>>> {
    if !g.params.iter().any(|p| matches!(p, GParam::Ty(_, _, Some(_)) | GParam::Const(_, _, Some(_)))) {
        return Ok(vec![]);
    }
    let mut v = vec![None; names.len()];
    for p in &g.params {
        let (n, d) = match p {
            GParam::Ty(n, _, Some(d)) => (n, tysyn(d, None)?),
            // (one zustc cannot read stays without a default: a use that leaves it out is an error, as before)
            GParam::Const(n, _, Some(d)) => match crate::ast_types::const_arg_syn(d) {
                Ok(t) => (n, t),
                Err(_) => continue,
            },
            _ => continue,
        };
        if let Some(k) = names.iter().position(|x| x == n) {
            v[k] = Some(d);
        }
    }
    Ok(v)
}

fn adt_params(g: &Generics) -> R<Vec<String>> {
    let mut v = Vec::new();
    collect_generics(g, &mut v)?;
    Ok(v.into_iter().map(|x| x.0).collect())
}

/// Replace the projections on the parameters (`T::Assoc`) in a type by `_`
fn strip_projections(t: &mut PTy, params: &[&str]) {
    match t {
        PTy::Path(None, p) if p.segs.len() > 1 && params.contains(&p.segs[0].name.as_str()) => *t = PTy::Infer,
        PTy::Path(q, p) => {
            if let Some(q) = q {
                strip_projections(q, params);
            }
            for s in &mut p.segs {
                for a in &mut s.args {
                    if let crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) = a {
                        strip_projections(t, params);
                    }
                }
            }
        }
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => strip_projections(i, params),
        PTy::Tuple(ts) => ts.iter_mut().for_each(|t| strip_projections(t, params)),
        _ => {}
    }
}

/// The names of the paths in a type (`Box<dyn Foo>` → Box, Foo)
fn ty_names(t: &PTy, out: &mut Vec<String>) {
    match t {
        PTy::Path(q, p) => {
            if let Some(q) = q {
                ty_names(q, out);
            }
            for s in &p.segs {
                out.push(s.name.clone());
                for a in &s.args {
                    if let crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) = a {
                        ty_names(t, out);
                    }
                }
                if let Some((ps, r)) = &s.fn_args {
                    ps.iter().for_each(|t| ty_names(t, out));
                    if let Some(r) = r {
                        ty_names(r, out);
                    }
                }
            }
        }
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => ty_names(i, out),
        PTy::Tuple(ts) => ts.iter().for_each(|t| ty_names(t, out)),
        PTy::Fn(ps, r) => {
            ps.iter().for_each(|t| ty_names(t, out));
            ty_names(r, out);
        }
        PTy::Dyn(bs) | PTy::Impl(bs) => {
            for b in bs {
                if let Bound::Trait(_, p) = b {
                    ty_names(&PTy::Path(None, p.clone()), out);
                }
            }
        }
        _ => {}
    }
}

/// The text of a type without whitespace (same as syn's token sequence with whitespace removed: for comparing `PartialEq<[U;N]>`)
fn ty_compact(t: &PTy) -> String {
    match t {
        PTy::Path(None, p) => path_compact(p),
        PTy::Path(Some(q), p) => format!("<{}as{}>", ty_compact(q), path_compact(p)),
        PTy::Ref(l, m, i) => format!("&{}{}{}", l.clone().unwrap_or_default(), if *m { "mut" } else { "" }, ty_compact(i)),
        PTy::Ptr(m, i) => format!("*{}{}", if *m { "mut" } else { "const" }, ty_compact(i)),
        PTy::Slice(i) => format!("[{}]", ty_compact(i)),
        PTy::Array(i, n) => format!("[{};{}]", ty_compact(i), expr_compact(n)),
        PTy::Tuple(v) => {
            let v: Vec<String> = v.iter().map(ty_compact).collect();
            if v.len() == 1 { format!("({},)", v[0]) } else { format!("({})", v.join(",")) }
        }
        PTy::Never => "!".to_string(),
        PTy::Infer => "_".to_string(),
        _ => String::new(),
    }
}

fn path_compact(p: &Path) -> String {
    let mut s = String::new();
    if p.global {
        s.push_str("::");
    }
    for (k, seg) in p.segs.iter().enumerate() {
        if k > 0 {
            s.push_str("::");
        }
        s.push_str(&compact(&seg.text));
    }
    s
}

fn expr_compact(e: &Expr) -> String {
    match e {
        Expr::Lit(t, _) => t.clone(),
        Expr::Path(None, p) => path_compact(p),
        _ => String::new(),
    }
}

/// A fn signature (front.rs's parse_sig)
/// `-> impl Trait + use<..>`: the precise capture list names the function's own parameters
/// A receiver's type (`&Pair<&str, isize>`, `Box<Foo<'_>>`) that writes the impl's self type (by its name) with a
/// lifetime elided in it, where the self type has a region: the position of its path
fn elided_in_self_recv(rt: &PTy, o: &Owner) -> Option<crate::parse::Pos> {
    fn syn_has_ref(t: &TySyn) -> bool {
        match t {
            TySyn::Ref(..) => true,
            TySyn::App(_, ts) | TySyn::Tuple(ts) => ts.iter().any(syn_has_ref),
            TySyn::Array(i, _) | TySyn::Slice(i) | TySyn::Ptr(_, i) => syn_has_ref(i),
            _ => false,
        }
    }
    fn elided(t: &PTy, depth: u32) -> bool {
        if depth > 16 {
            return false;
        }
        match t {
            PTy::Ref(l, _, i) => matches!(l.as_deref(), None | Some("'_")) || elided(i, depth + 1),
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => elided(i, depth + 1),
            PTy::Tuple(ts) => ts.iter().any(|t| elided(t, depth + 1)),
            PTy::Path(None, p) => p.segs.iter().all(|s| s.fn_args.is_none()) && p.segs.iter().any(|s| s.args.iter().any(|a| match a {
                crate::parse::GArg::Lifetime(l) => l == "'_",
                crate::parse::GArg::Ty(t) => elided(t, depth + 1),
                _ => false,
            })),
            _ => false,
        }
    }
    // (the self type has a region: a reference in it, or a lifetime of the header's in its slots)
    // (not R_ELIDED_ADT, a path the header writes without lifetimes (`impl<'a> S`), which may have none)
    let self_slots = o.regions.as_deref().map_or(0, |r| r.self_slots.iter().filter(|&&c| c != crate::types::R_ELIDED_ADT).count());
    if self_slots == 0 && !syn_has_ref(&o.self_ty) {
        return None;
    }
    if matches!(o.self_ty, TySyn::Ref(..) | TySyn::Ptr(..)) {
        return None;
    }
    let mut t = rt;
    loop {
        match t {
            PTy::Ref(_, _, i) => t = i,
            PTy::Path(None, p) if p.segs.len() == 1 && matches!(p.segs[0].name.as_str(), "Box" | "Rc" | "Arc" | "Pin") => match p.segs[0].args.first() {
                Some(crate::parse::GArg::Ty(i)) => t = i,
                _ => return None,
            },
            _ => break,
        }
    }
    let PTy::Path(None, p) = t else { return None };
    let [s] = p.segs.as_slice() else { return None };
    let base = o.base.rsplit("::").next().unwrap_or("");
    if s.name != base || s.fn_args.is_some() {
        return None;
    }
    // (`self: Foo` of `impl<'a> Foo<'a>`: its lifetime elided)
    let bare = s.args.is_empty() && self_slots > 0;
    (bare || elided(t, 0)).then_some(p.pos)
}

fn check_impl_ret(sig: &PSig, owner: Option<&Owner>) -> R<()> {
    let Some(PTy::Impl(bs)) = &sig.ret else { return Ok(()) };
    if !bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name != "use"))) {
        return err("at least one trait must be specified");
    }
    let mut lts: Vec<&str> = vec![];
    let mut tys: Vec<&str> = vec![];
    for p in &sig.generics.params {
        match p {
            GParam::Lifetime(l) => lts.push(l),
            GParam::Ty(n, _, _) | GParam::Const(n, ..) => tys.push(n),
        }
    }
    for b in bs {
        let Bound::Trait(_, p) = b else { continue };
        let Some(s) = p.segs.last().filter(|s| s.name == "use" && p.segs.len() == 1) else { continue };
        for a in &s.args {
            match a {
                crate::parse::GArg::Lifetime(l) if l == "'_" => {
                    // (fine when an argument has an elided lifetime for it to name, or the arguments name one
                    // lifetime only (`fn f<'a>(x: &'a u8) -> impl Tr + use<'_>`): the elision rules give it)
                    let mut named: Vec<String> = vec![];
                    let elided = sig.recv_ty.is_some()
                        || sig.params.iter().any(|p| {
                            let (mut v, mut e, mut o) = (vec![], false, false);
                            lifetimes(&p.1, &mut v, &mut e, &mut o);
                            named.extend(v);
                            e
                        });
                    named.sort();
                    named.dedup();
                    if !elided && named.len() != 1 {
                        return err("missing lifetime specifier");
                    }
                }
                crate::parse::GArg::Lifetime(l) if l == "'static" => {
                    return err("expected lifetime parameter in `use<...>` precise captures list, found `'static`")
                }
                crate::parse::GArg::Lifetime(l) if !lts.contains(&l.as_str()) && owner.is_none() => {
                    return Err(format!("use of undeclared lifetime name `{l}`"))
                }
                crate::parse::GArg::Ty(PTy::Path(None, q)) if q.segs.len() == 1 => {
                    let n = q.segs[0].name.as_str();
                    if n == "Self" && owner.is_some_and(|o| !o.is_trait) {
                        return err("`Self` can't be captured in `use<...>` precise captures list, since it is an alias");
                    }
                    if !tys.contains(&n) && owner.is_none_or(|o| !o.generics.iter().any(|g| g.0 == n)) {
                        return Err(format!("cannot find type or const parameter `{n}` in this scope"));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn parse_sig(sig: &PSig, is_extern: bool, is_main: bool, owner: Option<&Owner>) -> R<SigSyn> {
    check_impl_ret(sig, owner)?;
    // (E0106 in fn pointer types and `Fn() -> &T` bounds of the signature: also of trait and extern declarations,
    // which are not summarized as items)
    if generics_elision_err(&sig.generics) || sig.params.iter().any(|p| fn_elision_err(&p.1)) || sig.ret.as_ref().is_some_and(fn_elision_err) {
        return err("missing lifetime specifier (E0106)");
    }
    let mut generics = owner.map(|o| o.generics.clone()).unwrap_or_default();
    // (the lifetime bounds of the parameters' `impl Trait`s, by the type parameters they are lifted to: regions)
    let mut impl_lts: Vec<(String, String)> = vec![];
    collect_generics(&sig.generics, &mut generics)?;
    if is_extern && !generics.is_empty() {
        return err("extern fns cannot be generic");
    }
    if is_main && !generics.is_empty() {
        return err("`main` cannot be generic");
    }
    if is_main && !sig.generics.preds.is_empty() {
        return err("`main` function is not allowed to have a `where` clause (E0646)");
    }
    let self_ty = owner.map(|o| &o.self_ty);
    let mut params = Vec::new();
    let mut has_self = false;
    if let Some(rt) = &sig.recv_ty {
        if owner.is_none() {
            return err("`self` is only allowed in impls");
        }
        has_self = true;
        params.push(tysyn(rt, self_ty)?);
    }
    // (E0261: a free fn's signature names only the lifetimes it declares; fn pointer, `impl` and `dyn` types may
    // bind their own (`for<'a>`) and are not looked into)
    // (a method's: its own and its impl's or trait's)
    // (E0308 too: a receiver of the self type with an elided lifetime in it (`self: &Pair<&str, isize>` in `impl
    // Pair<&str, isize>`, `self: Foo<'_>` or `self: Foo` in `impl<'a> Foo<'a>`): a lifetime of the method's
    // own, not one of the impl's self type. Not in a fn pointer's or a `dyn Fn(&u8)`'s types, which bind theirs)
    if let (Some(o), Some(rt)) = (owner.filter(|o| !o.is_trait), &sig.recv_ty) {
        if let Some(p) = elided_in_self_recv(rt, o) {
            let m = "mismatched `self` parameter type: its lifetimes are not the impl's (E0308)";
            return Err(if p.line != 0 { format!("{}:{}: {m}", p.line, p.col + 1) } else { m.to_string() });
        }
    }
    if crate::regions::sig_names_lt(sig) {
        let outer: &[String] = owner.and_then(|o| o.regions.as_ref()).map(|r| r.lts.as_slice()).unwrap_or(&[]);
        let own: Vec<&str> = sig.generics.params.iter().filter_map(|g| match g {
            GParam::Lifetime(l) => Some(l.as_str()),
            _ => None,
        }).collect();
        if let Some(l) = own.iter().find(|l| outer.iter().any(|o| o == *l)) {
            return Err(format!("lifetime name `{l}` shadows a lifetime name that is already in scope (E0496)"));
        }
        let declared: Vec<&str> = outer.iter().map(|s| s.as_str()).chain(own.iter().copied()).collect();
        let mut used = vec![];
        // (with no `for<..>` in the item, the lifetimes in its `impl` and `dyn` types are the declared ones too:
        // `Box<dyn X<Y<'a> = &'a ()>>` with no `'a`, E0261)
        let deep = !HRTB_TEXT.with(|c| c.get());
        let names = |t: &PTy, used: &mut Vec<String>| if deep { named_lifetimes_deep(t, used, 0) } else { named_lifetimes(t, used) };
        if let Some(r) = &sig.recv_ty {
            names(r, &mut used);
        }
        for (_, t) in &sig.params {
            names(t, &mut used);
        }
        if let Some(r) = &sig.ret {
            names(r, &mut used);
        }
        bound_lifetimes(&sig.generics, &mut used);
        if let Some(l) = undeclared_lifetime(&used, &declared) {
            return Err(format!("use of undeclared lifetime name `{l}` (E0261)"));
        }
        // (E0308: a receiver of the self type names its lifetimes as the impl's self type does: `self: Foo<'b, 'a>`
        // in `impl<'a, 'b> Foo<'a, 'b>` is not Self)
        // (not of an impl for a reference (`impl Tr for &'a View<'static>`): its slots begin with the reference's)
        if let (Some(o), Some(rt)) = (owner.filter(|o| !o.is_trait && !matches!(o.self_ty, TySyn::Ref(..) | TySyn::Ptr(..))), &sig.recv_ty) {
            if let Some(r) = o.regions.as_deref() {
                let mut t = rt;
                loop {
                    match t {
                        PTy::Ref(_, _, i) => t = i,
                        PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "Box" => match p.segs[0].args.first() {
                            Some(crate::parse::GArg::Ty(i)) => t = i,
                            _ => break,
                        },
                        _ => break,
                    }
                }
                if let PTy::Path(None, p) = t {
                    let last = p.segs.last();
                    let base = o.base.rsplit("::").next().unwrap_or("");
                    if last.is_some_and(|s| s.name == base) {
                        let written: Vec<u8> = last
                            .unwrap()
                            .args
                            .iter()
                            .filter_map(|a| match a {
                                crate::parse::GArg::Lifetime(l) if l == "'static" => Some(crate::types::R_STATIC),
                                crate::parse::GArg::Lifetime(l) => r.lts.iter().position(|x| x == l).map(|i| i as u8),
                                _ => None,
                            })
                            .collect();
                        let own: Vec<u8> = r.self_slots.iter().copied().take(written.len()).collect();
                        if !written.is_empty() && own.len() == written.len() && own.iter().all(|&c| (c as usize) < r.lts.len() || c == crate::types::R_STATIC) && own != written {
                            let m = "mismatched `self` parameter type: its lifetimes are not the impl's (E0308)";
                            return Err(if p.pos.line != 0 { format!("{}:{}: {m}", p.pos.line, p.pos.col + 1) } else { m.to_string() });
                        }
                    }
                }
            }
        }
        // (the types the bounds and where clauses write are well formed, as the signature's types imply)
        if crate::wf::bounds_have_refs(&sig.generics) {
            let mut k = owner.map(crate::wf::Known::of_owner).unwrap_or_default();
            k.add_generics(&sig.generics);
            for t in sig.recv_ty.iter().chain(sig.params.iter().map(|p| &p.1)).chain(sig.ret.iter()) {
                k.implied(t, 0);
            }
            if let Some(e) = k.check_bounds(&sig.generics) {
                return Err(crate::wf::at(e));
            }
        }
    }
    if owner.is_none() {
        // (E0106: an elided lifetime in the return type needs exactly one lifetime among the parameters)
        // (not the prelude's own `__exr_` helpers, written against this compiler rather than rustc)
        if sig.recv_ty.is_none() && !sig.name.starts_with("__") {
            if let Some(r) = &sig.ret {
                if lifetime_positions(r).0 > 0 {
                    // (the distinct input lifetimes: `x: &'a u8, y: &'a u8` has one)
                    let ps: Vec<PTy> = sig.params.iter().map(|(_, t)| t.clone()).collect();
                    let inputs = input_lifetimes(&ps).0;
                    // (none written, but a parameter's type may be a struct (or an alias) with lifetimes it leaves out:
                    // `fn f(s: S) -> &T` with `struct S<'a>`; the summary does not see it)
                    let hidden = inputs == 0 && sig.params.iter().any(|(_, t)| may_hide_lifetimes(t));
                    if inputs != 1 && !hidden {
                        // (at the elided reference of the result)
                        let m = "missing lifetime specifier (E0106)";
                        return Err(match elided_pos(r).or_else(|| crate::wf::pos_of(r)) {
                            Some(p) => format!("{}:{}: {m}", p.line, p.col + 1),
                            None => m.to_string(),
                        });
                    }
                }
            }
        }
    }
    // (E0277: a returned value needs a size too, a `str` or `[T]` (a `dyn Trait` is E0746, checked elsewhere); a
    // trait's declaration without a body does not)
    if let Some(r) = sig.ret.as_ref().filter(|_| !owner.is_some_and(|o| o.is_trait) && !is_extern) {
        if let Some(u) = unsized_value(r).filter(|u| !u.starts_with("dyn")) {
            let m = format!("the size for values of type `{u}` cannot be known at compilation time (E0277)");
            return Err(match crate::wf::pos_of(r) {
                Some(p) => format!("{}:{}: {m}", p.line, p.col + 1),
                None => m,
            });
        }
    }
    let mut lifted_params: Vec<(usize, PTy)> = vec![];
    for (k, (_, t)) in sig.params.iter().enumerate() {
        // (E0277: a parameter's value needs a size; a trait's declaration without a body does not)
        if !owner.is_some_and(|o| o.is_trait) && !is_extern {
            if let Some(u) = unsized_value(t) {
                return Err(format!("the size for values of type `{u}` cannot be known at compilation time (E0277)"));
            }
        }
        // An `impl Trait` in argument position is an unnamed type parameter
        let (inner, rf) = match t {
            PTy::Ref(_, m, i) => (&**i, Some(*m)),
            t => (t, None),
        };
        if let PTy::Impl(bs) = inner {
            let g = fresh_impl_name(&generics);
            impl_lts.extend(bs.iter().filter_map(|b| if let Bound::Lifetime(l) = b { Some((g.clone(), l.clone())) } else { None }));
            generics.push((g.clone(), bound_names(bs)?));
            let n = TySyn::Named(g);
            params.push(match rf {
                Some(m) => TySyn::Ref(m, Box::new(n)),
                None => n,
            });
        } else if nested_impl(t) {
            // (`Option<&[impl AsRef<str>]>`: each `impl Trait` inside is a type parameter without a name, too)
            let lifted = lift_impls(t, &mut generics, &mut impl_lts)?;
            params.push(tysyn(&lifted, self_ty)?);
            lifted_params.push((k, lifted));
        } else {
            params.push(tysyn(t, self_ty)?);
        }
    }
    // (the regions of a parameter with an `impl Trait` inside are those of the type it is lifted to, with the hidden
    // type parameter in place of the `impl`: `Option<impl Copy + 'static>` holds what its `__impl0` does, which the
    // bound requires to be 'static at the call (`take(Some(&x))`, E0597))
    let lifted_sig = (!lifted_params.is_empty()).then(|| {
        let mut rs = sig.clone();
        for (k, t) in std::mem::take(&mut lifted_params) {
            rs.params[k].1 = t;
        }
        // (with their lifetime bounds, which the lifted type no longer writes: the signature names a lifetime)
        for (g, _) in generics.iter().filter(|g| g.0.starts_with("__impl")) {
            if !rs.generics.params.iter().any(|p| matches!(p, GParam::Ty(n, ..) if n == g)) {
                let bs = impl_lts.iter().filter(|(n, _)| n == g).map(|(_, l)| Bound::Lifetime(l.clone())).collect();
                rs.generics.params.push(GParam::Ty(g.clone(), bs, None));
            }
        }
        rs
    });
    let ret = match &sig.ret {
        None => TySyn::Prim(Ty::Unit),
        // (`-> impl ?Sized` with no other trait: a return value must have a size)
        Some(PTy::Impl(bs)) if bs.iter().any(|b| matches!(b, Bound::Trait(true, p) if p.segs.last().is_some_and(|s| s.name == "Sized")))
            && !bs.iter().any(|b| matches!(b, Bound::Trait(false, _))) =>
        {
            return err("the size for values of type `impl ?Sized` cannot be known at compilation time");
        }
        Some(PTy::Impl(_)) => TySyn::Named(IMPL_RET.into()),
        // (`-> Option<impl Debug>`: the whole type comes from the body, as for `-> impl Trait`)
        Some(PTy::Path(Some(q), _)) if nested_impl(q) => return err("`impl Trait` is not allowed in paths"),
        Some(PTy::Dyn(_)) => return err("return type cannot be a trait object without pointer indirection (E0746)"),
        Some(t) if has_infer(t) => return err("the placeholder `_` is not allowed within types on item signatures for return types (E0121)"),
        Some(t) if nested_impl(t) => {
            TySyn::App(OPAQUE_NESTED.into(), vec![tysyn(&impl_as_infer(t), self_ty)?])
        }
        Some(t) => tysyn(t, self_ty)?,
    };
    let name = sig.name.clone();
    // (an impl for a reference, `impl Tr for &S` beside `impl Tr for S`: its own symbol, `_xS.__ref_v` (one
    // `__ref_` a level), as its name is `S::__ref_v`)
    let refs = {
        let (mut pre, mut t) = (String::new(), owner.map(|o| &o.self_ty));
        while let Some(TySyn::Ref(_, i)) = t {
            pre.push_str("__ref_");
            t = Some(i);
        }
        pre
    };
    // (the trait's part of the symbol where two traits of one type have a method of one name: Display / Debug's
    // `fmt`, PartialEq<X> / PartialOrd<X>; for an impl for a reference too, `_xS.Debug.__ref_fmt`)
    let disc = match owner.and_then(|o| o.impl_trait.as_deref()) {
        Some(t) if t == "Display" || t == "Debug" => format!("{t}."),
        Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => {
            let t: String = t.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            format!("{t}.")
        }
        _ => String::new(),
    };
    let symbol = match owner {
        _ if is_extern || is_main => name.clone(),
        Some(o) => format!("_x{}.{disc}{refs}{name}", o.base),
        None => format!("_x{name}"),
    };
    let is_trait = owner.is_some_and(|o| o.is_trait);
    let preds = {
        let names: Vec<&str> = generics.iter().map(|g| g.0.as_str()).collect();
        other_preds(&sig.generics, self_ty, &names)
    };
    Ok(SigSyn {
        params: params,
        ret: ret,
        symbol: symbol,
        is_extern: is_extern,
        is_main: is_main,
        impl_of: owner.filter(|o| !o.is_trait).map(|o| o.base.clone()),
        has_self: has_self,
        generics: generics,
        trait_of: owner.filter(|o| o.is_trait).map(|o| o.base.clone()),
        has_body: true,
        impl_trait: owner.and_then(|o| o.impl_trait.clone()),
        self_ty: owner.filter(|_| !is_trait).map(|o| o.self_ty.clone()),
        export: false,
        panic_handler: false,
        ret_from: ret_from(sig),
        regions: {
            let otp: Vec<&str> = owner.map(|o| o.generics.iter().map(|g| g.0.as_str()).collect()).unwrap_or_default();
            crate::regions::sig_regions(lifted_sig.as_ref().unwrap_or(sig), owner.and_then(|o| o.regions.as_deref()), &otp, &impl_lts)
        },
        is_unsafe: sig.quals.iter().any(|q| q == "unsafe") || (is_extern && !sig.quals.iter().any(|q| q == "safe")),
        preds: preds,
        own_unsized: adt_unsized(&sig.generics),
    })
}

/// The where clauses on other than a parameter (SigSyn::preds): `where T::A: Bar`, `where <T as Tr>::A: Bar`,
/// `where W<T>: Bar`, `where S: Bar`. Lifetimes', `Self`'s and bare parameters' (of `params`) are not (collect_generics has those), nor one whose type is not
/// written in a way the summary takes
fn other_preds(g: &Generics, self_ty: Option<&TySyn>, params: &[&str]) -> Vec<(TySyn, String, Vec<String>)> {
    let mut out = vec![];
    for (t, bs) in &g.preds {
        let tr = match t {
            // (a parameter's, `Self`'s and a lifetime's are not; a concrete type's are, `where S: Bar`)
            PTy::Path(None, p)
                if !p.global
                    && p.segs.len() == 1
                    && p.segs[0].args.is_empty()
                    && p.segs[0].fn_args.is_none()
                    && (params.contains(&p.segs[0].name.as_str()) || p.segs[0].name == "Self" || p.segs[0].name.starts_with('\'')) =>
            {
                continue
            }
            // (`<T as Tr>::A`: the trait is the segment before the name)
            PTy::Path(Some(_), p) => p.segs.len().checked_sub(2).map(|i| p.segs[i].name.clone()).unwrap_or_default(),
            _ => String::new(),
        };
        let (Ok(ts), Ok(names)) = (tysyn(t, self_ty), bound_names(bs)) else { continue };
        if !names.is_empty() {
            out.push((ts, tr, names));
        }
    }
    out
}

thread_local! {
    /// The item being summarized writes `for<..>` (a higher-ranked binder, which the parser does not keep): the
    /// lifetimes in its `impl` and `dyn` types may be the binder's, and are not checked against the declared ones
    static HRTB_TEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// named_lifetimes, and in `impl` and `dyn` types too (their bounds, associated type bindings, `Fn(..)` sugar)
fn named_lifetimes_deep(t: &PTy, out: &mut Vec<String>, depth: u32) {
    fn path(p: &crate::parse::Path, out: &mut Vec<String>, depth: u32) {
        for sg in &p.segs {
            for a in &sg.args {
                match a {
                    crate::parse::GArg::Lifetime(l) => out.push(l.clone()),
                    crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) => named_lifetimes_deep(t, out, depth + 1),
                    _ => {}
                }
            }
            if let Some((ps, r)) = &sg.fn_args {
                ps.iter().for_each(|t| named_lifetimes_deep(t, out, depth + 1));
                if let Some(r) = r {
                    named_lifetimes_deep(r, out, depth + 1);
                }
            }
        }
    }
    if depth > 16 {
        return;
    }
    match t {
        PTy::Ref(l, _, i) => {
            if let Some(l) = l {
                out.push(l.clone());
            }
            named_lifetimes_deep(i, out, depth + 1);
        }
        PTy::Path(None, p) => path(p, out, depth),
        PTy::Impl(bs) | PTy::Dyn(bs) => {
            for b in bs {
                match b {
                    Bound::Lifetime(l) => out.push(l.clone()),
                    Bound::Trait(_, p) => path(p, out, depth),
                }
            }
        }
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => named_lifetimes_deep(i, out, depth + 1),
        PTy::Tuple(ts) => ts.iter().for_each(|t| named_lifetimes_deep(t, out, depth + 1)),
        _ => {}
    }
}

/// Checks on a struct / enum / union definition: unused parameters (E0392), `Self` in defaults (E0735), union fields
/// a slice or array whose element is unsized (`[[T]]`, `[str; 2]`, `[dyn Tr]`), anywhere in t: its text
/// The lifetimes a type names, outside fn pointer, `impl` and `dyn` types (which may bind their own with `for<'a>`)
pub fn named_lifetimes(t: &PTy, out: &mut Vec<String>) {
    match t {
        PTy::Ref(l, _, i) => {
            if let Some(l) = l {
                out.push(l.clone());
            }
            named_lifetimes(i, out);
        }
        PTy::Path(None, p) => {
            for sg in &p.segs {
                for a in &sg.args {
                    match a {
                        crate::parse::GArg::Lifetime(l) => out.push(l.clone()),
                        crate::parse::GArg::Ty(t) => named_lifetimes(t, out),
                        _ => {}
                    }
                }
            }
        }
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => named_lifetimes(i, out),
        PTy::Tuple(ts) => ts.iter().for_each(|t| named_lifetimes(t, out)),
        _ => {}
    }
}

/// The lifetimes generics bound with: `T: 'a`, `'a: 'b` (a trait bound may bind its own with `for<'a>`, which the
/// parser does not keep: those are not looked into)
fn bound_lifetimes(g: &Generics, out: &mut Vec<String>) {
    let bs_of = |bs: &[Bound], out: &mut Vec<String>| {
        for b in bs {
            if let Bound::Lifetime(l) = b {
                out.push(l.clone());
            }
        }
    };
    for p in &g.params {
        if let GParam::Ty(_, bs, _) = p {
            bs_of(bs, out);
        }
    }
    for (t, bs) in &g.preds {
        if let PTy::Path(None, p) = t {
            if let [sg] = p.segs.as_slice() {
                if sg.name.starts_with('\'') {
                    out.push(sg.name.clone());
                    bs_of(bs, out);
                    continue;
                }
            }
        }
        // (`where for<'a> &'a T: Tr` binds its own: only the bounds of a type that is a plain parameter; with no
        // `for<..>` in the item, the type's and its bounds' lifetimes too: `&'missing Self: Add<Self>`)
        if matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty()) {
            bs_of(bs, out);
        } else if !HRTB_TEXT.with(|c| c.get()) {
            named_lifetimes_deep(t, out, 0);
            named_lifetimes_deep(&PTy::Impl(bs.clone()), out, 0);
        }
    }
}

/// t names a type with no lifetime arguments that may have lifetimes (a struct or an alias of the program's)
fn may_hide_lifetimes(t: &PTy) -> bool {
    match t {
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => may_hide_lifetimes(i),
        PTy::Tuple(ts) => ts.iter().any(may_hide_lifetimes),
        PTy::Path(None, p) => p.segs.last().is_some_and(|s| {
            // (a module's item flattened with its path, `parse__Cursor`: its own name decides. A name of the program's
            //  own with `__` in it, `Foo__bar`, is not one: the part after `__` of a flattened type is its name, which
            //  starts with a capital letter)
            let base = match s.name.rsplit_once("__") {
                Some((_, n)) if n.starts_with(|c: char| c.is_ascii_uppercase()) => n,
                _ => s.name.as_str(),
            };
            let std = matches!(base, "String" | "Vec" | "Box" | "Option" | "Result" | "Rc" | "Arc" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "str" | "Self")
                || base.chars().next().is_some_and(|c| c.is_ascii_lowercase());
            let lts = s.args.iter().any(|a| matches!(a, crate::parse::GArg::Lifetime(_)));
            (!std && !lts) || s.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(t) if may_hide_lifetimes(t)))
        }),
        _ => false,
    }
}

/// Where t's first elided reference is (the type it refers to)
fn elided_pos(t: &PTy) -> Option<crate::parse::Pos> {
    match t {
        PTy::Ref(l, _, i) if l.as_deref().is_none_or(|l| l == "'_") => crate::wf::pos_of(i),
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => elided_pos(i),
        PTy::Tuple(ts) => ts.iter().find_map(elided_pos),
        PTy::Path(None, p) => p.segs.iter().flat_map(|s| s.args.iter()).find_map(|a| match a {
            crate::parse::GArg::Ty(t) => elided_pos(t),
            _ => None,
        }),
        _ => None,
    }
}

/// A trait object in t is bounded by `'_`
fn anon_dyn(t: &PTy) -> bool {
    match t {
        PTy::Dyn(bs) => bs.iter().any(|b| matches!(b, Bound::Lifetime(l) if l == "'_")),
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => anon_dyn(i),
        PTy::Tuple(ts) => ts.iter().any(anon_dyn),
        PTy::Path(None, p) => p.segs.iter().flat_map(|s| s.args.iter()).any(|a| matches!(a, crate::parse::GArg::Ty(t) if anon_dyn(t))),
        _ => false,
    }
}

/// The lifetime positions of a type, outside fn pointer, `impl` and `dyn` types: (elided ones (`&T`, `'_`), all)
fn lifetime_positions(t: &PTy) -> (usize, usize) {
    match t {
        PTy::Ref(l, _, i) => {
            let (e, a) = lifetime_positions(i);
            let el = l.as_ref().is_none_or(|l| l == "'_") as usize;
            (e + el, a + 1)
        }
        PTy::Path(None, p) => p.segs.iter().flat_map(|sg| sg.args.iter()).fold((0, 0), |(e, a), g| match g {
            crate::parse::GArg::Lifetime(l) => (e + (l == "'_") as usize, a + 1),
            crate::parse::GArg::Ty(t) => {
                let (e2, a2) = lifetime_positions(t);
                (e + e2, a + a2)
            }
            _ => (e, a),
        }),
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => lifetime_positions(i),
        PTy::Tuple(ts) => ts.iter().map(lifetime_positions).fold((0, 0), |(e, a), (e2, a2)| (e + e2, a + a2)),
        _ => (0, 0),
    }
}

/// The first of `used` that is not `'static` / `'_` nor in `declared` (E0261)
fn undeclared_lifetime(used: &[String], declared: &[&str]) -> Option<String> {
    used.iter().find(|l| !matches!(l.as_str(), "'static" | "'_") && !declared.contains(&l.as_str()) && !l.starts_with("'r#")).cloned()
}

/// A type that is unsized itself (`str`, `[T]`, `dyn Tr`, or a tuple with one of them), as its text
fn unsized_value(t: &PTy) -> Option<String> {
    match t {
        PTy::Slice(e) => Some(format!("[{}]", ty_text(e))),
        PTy::Dyn(_) => Some("dyn ..".to_string()),
        PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "str" && p.segs[0].args.is_empty() => Some("str".to_string()),
        PTy::Tuple(xs) => xs.iter().find_map(unsized_value),
        _ => None,
    }
}

fn ty_text(t: &PTy) -> String {
    match t {
        PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => p.segs[0].name.clone(),
        _ => "..".to_string(),
    }
}

fn unsized_elem(t: &PTy) -> Option<String> {
    fn is_unsized(t: &PTy) -> bool {
        matches!(t, PTy::Slice(_) | PTy::Dyn(_)) || matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "str")
    }
    match t {
        PTy::Slice(e) | PTy::Array(e, _) => {
            if is_unsized(e) {
                return Some(match &**e {
                    PTy::Slice(_) => "[..]".to_string(),
                    PTy::Dyn(_) => "dyn ..".to_string(),
                    _ => "str".to_string(),
                });
            }
            unsized_elem(e)
        }
        PTy::Ref(_, _, e) | PTy::Ptr(_, e) => unsized_elem(e),
        PTy::Tuple(xs) => xs.iter().find_map(unsized_elem),
        PTy::Path(_, p) => p.segs.iter().flat_map(|s| s.args.iter()).find_map(|a| match a {
            crate::parse::GArg::Ty(t) => unsized_elem(t),
            _ => None,
        }),
        _ => None,
    }
}

/// A struct's, an enum's or a union's field whose type passes a type parameter of the item to a struct or enum of the
/// program that bounds that parameter by 'static (`bar: Bar<U>` with `struct Bar<T: 'static>`). Unlike `T: 'a`,
/// rustc does not infer `U: 'static` from the fields: the item must declare it (E0310). statics: the short names of
/// such structs and enums, and the indexes of their type and const parameters that must be 'static. (line, col, msg)
/// The markers among a trait header's supertraits and `where Self: ..` bounds (not `?` ones, nor a generic
/// parameter's bounds or another type's where clause) that make an implementing type Sized: `trait Tr: Clone`,
/// `trait Tr where Self: Sized`
pub fn trait_sized_supers(src: &str, it: &SrcItem) -> Vec<String> {
    let Some(text) = src.get(it.start..it.end) else { return vec![] };
    let text = match it.kind {
        LexKind::TraitHeader => format!("{text}}}"),
        _ => text.to_string(),
    };
    let Ok(f) = parse_file(&text) else { return vec![] };
    let Some(ItemKind::Trait(_, g, sups, _)) = f.items.first().map(|i| &i.kind) else { return vec![] };
    let of_self = g.preds.iter().filter(|(t, _)| matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "Self" && p.segs[0].args.is_empty())).flat_map(|(_, bs)| bs.iter());
    let mut out: Vec<String> = vec![];
    for b in sups.iter().chain(of_self) {
        if let Bound::Trait(false, p) = b {
            if let Some(n) = p.segs.last().map(|s| s.name.as_str()).filter(|n| matches!(*n, "Sized" | "Clone" | "Copy" | "Default")) {
                if !out.iter().any(|x| x == n) {
                    out.push(n.to_string());
                }
            }
        }
    }
    out
}

/// A trait bound's equations (`Iterator<Item = u8>`: ("Item", "u8")), their values as text with the names rename
/// gives (None where a name has none, or the value is not one eq_ty_text writes)
fn bound_eqs(p: &Path, rename: &dyn Fn(&str) -> Option<String>) -> Vec<(String, Option<String>)> {
    let Some(s) = p.segs.last() else { return vec![] };
    s.args.iter().filter_map(|a| if let crate::parse::GArg::Bind(n, t) = a { Some((n.clone(), eq_ty_text(t, rename, 0))) } else { None }).collect()
}

/// A written type as text, without positions, its one-segment names through rename (a type parameter's to another
/// item's): None for one not written here (fn pointers, arrays, `dyn` and `impl` types, qualified paths)
fn eq_ty_text(t: &PTy, rename: &dyn Fn(&str) -> Option<String>, depth: u32) -> Option<String> {
    use crate::parse::GArg;
    if depth > 12 {
        return None;
    }
    Some(match t {
        PTy::Path(None, p) => {
            if let [s] = p.segs.as_slice() {
                if s.args.is_empty() && s.fn_args.is_none() {
                    return rename(&s.name);
                }
            }
            let mut out = vec![];
            for s in &p.segs {
                if s.fn_args.is_some() {
                    return None;
                }
                let mut seg = s.name.clone();
                if !s.args.is_empty() {
                    let mut args = vec![];
                    for a in &s.args {
                        args.push(match a {
                            GArg::Ty(t) => eq_ty_text(t, rename, depth + 1)?,
                            GArg::Lifetime(l) => rename(l)?,
                            GArg::Bind(n, t) => format!("{n} = {}", eq_ty_text(t, rename, depth + 1)?),
                            GArg::Const(_) => return None,
                        });
                    }
                    seg = format!("{seg}<{}>", args.join(", "));
                }
                out.push(seg);
            }
            out.join("::")
        }
        PTy::Ref(l, m, i) => {
            let l = match l {
                Some(l) => format!("{} ", rename(l)?),
                None => String::new(),
            };
            format!("&{l}{}{}", if *m { "mut " } else { "" }, eq_ty_text(i, rename, depth + 1)?)
        }
        PTy::Ptr(m, i) => format!("*{} {}", if *m { "mut" } else { "const" }, eq_ty_text(i, rename, depth + 1)?),
        PTy::Slice(i) => format!("[{}]", eq_ty_text(i, rename, depth + 1)?),
        PTy::Tuple(ts) => {
            let v: Option<Vec<String>> = ts.iter().map(|t| eq_ty_text(t, rename, depth + 1)).collect();
            let v = v?;
            if v.len() == 1 { format!("({},)", v[0]) } else { format!("({})", v.join(", ")) }
        }
        PTy::Never => "!".to_string(),
        _ => return None,
    })
}

/// Whether two equations' values (eq_ty_text's, in the definition's names) are surely of different types: apart with
/// each path taken by its last segment (`std::string::String` is `String`), and naming only primitives, std's
/// nominal types, the program's structs, enums and unions and the definition's parameters (lts, tys), none of which
/// the program declares an alias of (another name may be an alias of the other side's: `type B = u8`)
fn eq_values_apart(src: &str, a: &str, b: &str, lts: &[&str], tys: &[&str]) -> bool {
    // (`struct Foo` in the program's text, a whole name: a nominal type of its own)
    let declares = |pat: &str| src.match_indices(pat).any(|(i, _)| !src[i + pat.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_'));
    let norm = |s: &str| -> Option<String> {
        let cs: Vec<char> = s.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < cs.len() {
            let c = cs[i];
            if c.is_alphabetic() || c == '_' || c == '\'' {
                let st = i;
                i += 1;
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                    i += 1;
                }
                let w: String = cs[st..i].iter().collect();
                // (a path's prefix: dropped)
                if cs.get(i) == Some(&':') && cs.get(i + 1) == Some(&':') {
                    i += 2;
                    continue;
                }
                if w.starts_with('\'') {
                    if w != "'static" && !lts.contains(&w.as_str()) {
                        return None;
                    }
                } else if !matches!(w.as_str(), "mut" | "const") {
                    let known = matches!(w.as_str(), "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "f32" | "f64" | "bool" | "char" | "str"
                        | "String" | "Vec" | "Option" | "Box" | "Rc" | "Arc" | "Cell" | "RefCell" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque")
                        || tys.contains(&w.as_str())
                        || ["struct ", "enum ", "union "].iter().any(|k| declares(&format!("{k}{w}")));
                    if !known || src.contains(&format!("type {w} ")) || src.contains(&format!("type {w}<")) || src.contains(&format!("type {w}=")) {
                        return None;
                    }
                }
                out.push_str(&w);
            } else {
                out.push(c);
                i += 1;
            }
        }
        Some(out)
    };
    matches!((norm(a), norm(b)), (Some(x), Some(y)) if x != y)
}

/// A `Drop` impl (its header imp) against the definition of its struct, enum or union (adt): its self type's arguments
/// are its own parameters, each once (E0366), and what its header and where clauses require of them is what the
/// definition requires, or what its fields imply (E0367, as rustc's `ensure_drop_predicates_are_implied_by_item_defn`).
/// supers: each of the program's traits' supertraits. The line in imp's text, and the message
pub fn drop_impl_unmet(src: &str, imp: &SrcItem, adt: &SrcItem, supers: &crate::fxhash::FxMap<String, Vec<String>>) -> Option<(usize, String)> {
    use crate::parse::GArg;
    let it = format!("{}}}", src.get(imp.start..imp.end)?);
    let f = parse_file(&it).ok()?;
    let ItemKind::Impl(ig, Some((false, tp)), PTy::Path(None, sp), _) = &f.items.first()?.kind else { return None };
    if tp.segs.last().is_none_or(|s| s.name != "Drop") {
        return None;
    }
    let at = src.get(adt.start..adt.end)?;
    let af = parse_file(at).ok()?;
    let ad = af.items.first()?;
    let (aname, ag, tys, what): (&String, &Generics, Vec<&PTy>, &str) = match &ad.kind {
        ItemKind::Struct(n, g, fields) | ItemKind::Union(n, g, fields) => (n, g, match fields {
            Fields::Named(v) => v.iter().map(|x| &x.1).collect(),
            Fields::Tuple(v) => v.iter().collect(),
            Fields::Unit => vec![],
        }, if matches!(ad.kind, ItemKind::Union(..)) { "union" } else { "struct" }),
        ItemKind::Enum(n, g, vs) => (n, g, vs.iter().flat_map(|v| match &v.1 {
            Fields::Named(f) => f.iter().map(|x| &x.1).collect::<Vec<_>>(),
            Fields::Tuple(f) => f.iter().collect(),
            Fields::Unit => vec![],
        }).collect(), "enum"),
        _ => return None,
    };
    let seg = sp.segs.last()?;
    if sp.segs.len() != 1 || seg.name != *aname || seg.fn_args.is_some() {
        return None;
    }
    // (the definition's parameters: lifetimes, then the others in their order, as a path writes its arguments)
    let a_lts: Vec<&str> = ag.params.iter().filter_map(|p| if let GParam::Lifetime(l) = p { Some(l.as_str()) } else { None }).collect();
    let a_rest: Vec<&str> = ag.params.iter().filter_map(|p| match p {
        GParam::Ty(n, ..) | GParam::Const(n, ..) => Some(n.as_str()),
        GParam::Lifetime(_) => None,
    }).collect();
    let w_lts: Vec<&str> = seg.args.iter().filter_map(|a| if let GArg::Lifetime(l) = a { Some(l.as_str()) } else { None }).collect();
    let w_rest: Vec<&GArg> = seg.args.iter().filter(|a| !matches!(a, GArg::Lifetime(_) | GArg::Bind(..))).collect();
    // (lifetimes left out (`impl Drop for M`) or another count: other errors' (E0726, E0107))
    if w_lts.len() != a_lts.len() || w_rest.len() != a_rest.len() {
        return None;
    }
    let i_lts: Vec<&str> = ig.params.iter().filter_map(|p| if let GParam::Lifetime(l) = p { Some(l.as_str()) } else { None }).collect();
    let i_rest: Vec<&str> = ig.params.iter().filter_map(|p| match p {
        GParam::Ty(n, ..) | GParam::Const(n, ..) => Some(n.as_str()),
        GParam::Lifetime(_) => None,
    }).collect();
    // (E0366: each argument one of the impl's parameters, each once; `'_` is one of its own)
    let mut map: Vec<(String, String)> = vec![];
    let specialized = "`Drop` impls cannot be specialized (E0366)".to_string();
    for (w, a) in w_lts.iter().zip(&a_lts) {
        if *w == "'_" {
            continue;
        }
        if !i_lts.contains(w) || map.iter().any(|(x, _)| x == w) {
            return Some((1, specialized));
        }
        map.push((w.to_string(), a.to_string()));
    }
    for (w, a) in w_rest.iter().zip(&a_rest) {
        let n = match w {
            GArg::Ty(PTy::Path(None, p)) if p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() => p.segs[0].name.as_str(),
            GArg::Const(crate::parse::Expr::Path(_, p)) if p.segs.len() == 1 => p.segs[0].name.as_str(),
            _ => return Some((1, specialized)),
        };
        if !i_rest.contains(&n) || map.iter().any(|(x, _)| x == n) {
            return Some((1, specialized));
        }
        map.push((n.to_string(), a.to_string()));
    }
    let to_adt = |n: &str| map.iter().find(|(x, _)| x == n).map(|(_, a)| a.clone());
    // (what the definition gives: its bounds and where clauses, and what its fields imply (RFC 2093's inferred
    // outlives: `&'a T` gives T: 'a))
    let mut k = crate::wf::Known::default();
    k.add_generics(ag);
    for t in &tys {
        k.implied(t, 0);
    }
    // (each parameter's trait bounds there, with their supertraits)
    let short = |p: &Path| p.segs.last().map(|s| s.name.clone()).unwrap_or_default();
    // (each parameter's traits there, with the equations the bound writes: (name, the value's text, None where it is
    // not compared))
    type Eqs = Vec<(String, Option<String>)>;
    let mut a_traits: Vec<(String, String, Eqs)> = vec![];
    let mut a_unsized: Vec<String> = vec![];
    let note = |n: &str, bs: &[Bound], a_traits: &mut Vec<(String, String, Eqs)>, a_unsized: &mut Vec<String>| {
        for b in bs {
            if let Bound::Trait(maybe, p) = b {
                if *maybe {
                    a_unsized.push(n.to_string());
                    continue;
                }
                let eqs = bound_eqs(p, &|x: &str| Some(x.to_string()));
                let mut todo = vec![short(p)];
                let first = short(p);
                while let Some(t) = todo.pop() {
                    // (the same trait again (`T: Iterator` and `where T: Iterator<Item = u8>`): its equations too)
                    if let Some(x) = a_traits.iter_mut().find(|x| x.0 == n && x.1 == t) {
                        if t == first {
                            x.2.extend(eqs.iter().cloned());
                        }
                        continue;
                    }
                    todo.extend(supers.get(&t).into_iter().flatten().cloned());
                    a_traits.push((n.to_string(), t.clone(), if t == first { eqs.clone() } else { vec![] }));
                }
            }
        }
    };
    for p in &ag.params {
        if let GParam::Ty(n, bs, _) = p {
            note(n, bs, &mut a_traits, &mut a_unsized);
        }
    }
    for (t, bs) in &ag.preds {
        if let PTy::Path(None, p) = t {
            if let [s] = p.segs.as_slice() {
                if s.args.is_empty() && !s.name.starts_with('\'') {
                    note(&s.name, bs, &mut a_traits, &mut a_unsized);
                }
            }
        }
    }
    let unmet = |pred: String| format!("`Drop` impl requires `{pred}` but the {what} it is implemented for does not (E0367)");
    // (the impl's requirements, of a parameter of its self type: one not there may be any (`impl<'r, T: 'r> Drop for
    // R<T>`))
    let mut preds: Vec<(&str, &[Bound], u32)> = vec![];
    let mut i_unsized: Vec<&str> = vec![];
    for p in &ig.params {
        if let GParam::Ty(n, bs, _) = p {
            preds.push((n.as_str(), bs.as_slice(), 0));
        }
    }
    for (t, bs) in &ig.preds {
        if let PTy::Path(None, p) = t {
            if let [s] = p.segs.as_slice() {
                if s.args.is_empty() && s.fn_args.is_none() {
                    preds.push((s.name.as_str(), bs.as_slice(), p.pos.line));
                }
            }
        }
    }
    for &(n, bs, _) in &preds {
        if bs.iter().any(|b| matches!(b, Bound::Trait(true, _))) {
            i_unsized.push(n);
        }
    }
    for &(n, bs, line) in &preds {
        let Some(an) = to_adt(n) else { continue };
        let at_line = if line == 0 { 1 } else { line as usize };
        for b in bs {
            match b {
                Bound::Lifetime(l) => {
                    if l == "'static" {
                        if !k.outlives(&an, "'static") {
                            return Some((at_line, unmet(format!("{n}: {l}"))));
                        }
                        continue;
                    }
                    let Some(al) = to_adt(l) else { continue };
                    if !k.outlives(&an, &al) {
                        // (one lifetime outliving another: at the impl, as rustc has it)
                        let ln = if n.starts_with('\'') { 1 } else { at_line };
                        return Some((ln, unmet(format!("{n}: {l}"))));
                    }
                }
                Bound::Trait(true, _) => {}
                Bound::Trait(false, p) => {
                    let t = short(p);
                    // (a marker std gives every type it may (Sized aside, below): not looked at)
                    if matches!(t.as_str(), "Sized") {
                        continue;
                    }
                    match a_traits.iter().find(|x| x.0 == an && x.1 == t) {
                        // (a parameter bounded by one of the program's traits, whose supertraits are not all known:
                        // a trait not found is not reported)
                        None if a_traits.iter().any(|x| x.0 == an && x.1 == "#?") => {}
                        None => return Some((at_line, unmet(format!("{n}: {t}")))),
                        Some(x) => {
                            // (each equation the impl writes is the definition's, its value the same, in the
                            // definition's names. One whose value names a parameter of the impl not in the self type
                            // may hold (`impl<T: Trait<Assoc = U>, U> Drop for Foo<T>`), and one not compared is not
                            // reported)
                            let rename = |x: &str| if i_rest.contains(&x) || i_lts.contains(&x) { to_adt(x) } else { Some(x.to_string()) };
                            let shown = bound_eqs(p, &|x: &str| Some(x.to_string()));
                            for ((bn, bk), (_, bv)) in bound_eqs(p, &rename).into_iter().zip(shown) {
                                let Some(bk) = bk else { continue };
                                let same = match x.2.iter().find(|e| e.0 == bn) {
                                    None => false,
                                    Some((_, None)) => true,
                                    Some((_, Some(ak))) => *ak == bk || !eq_values_apart(src, ak, &bk, &a_lts, &a_rest),
                                };
                                if !same {
                                    return Some((at_line, unmet(format!("<{n} as {t}>::{bn} == {}", bv.unwrap_or(bk)))));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // (an impl's type parameter is Sized unless it says `?Sized`: the definition's `?Sized` one is not)
    for p in &ig.params {
        if let GParam::Ty(n, ..) = p {
            if let Some(an) = to_adt(n) {
                // (unless a bound there makes it Sized: `T: ?Sized + Clone`, or one of the program's traits whose header
                // names Sized or such a trait (`trait Tr: Sized`), through the supertraits)
                let sized_by_bound = a_traits.iter().any(|x| x.0 == an && matches!(x.1.as_str(), "Clone" | "Copy" | "Default" | "Sized"));
                if a_unsized.contains(&an) && !i_unsized.contains(&n.as_str()) && !sized_by_bound {
                    return Some((1, unmet(format!("{n}: Sized"))));
                }
            }
        }
    }
    None
}

pub fn static_field_unmet(src: &str, it: &SrcItem, statics: &crate::fxhash::FxMap<&str, Vec<usize>>) -> Option<(usize, usize, String)> {
    let text = src.get(it.start..it.end)?;
    let f = parse_file(text).ok()?;
    let item = f.items.first()?;
    let (g, tys): (&Generics, Vec<&PTy>) = match &item.kind {
        ItemKind::Struct(_, g, fields) | ItemKind::Union(_, g, fields) => (g, match fields {
            Fields::Named(v) => v.iter().map(|x| &x.1).collect(),
            Fields::Tuple(v) => v.iter().collect(),
            Fields::Unit => vec![],
        }),
        ItemKind::Enum(_, g, vs) => (g, vs.iter().flat_map(|v| match &v.1 {
            Fields::Named(f) => f.iter().map(|x| &x.1).collect::<Vec<_>>(),
            Fields::Tuple(f) => f.iter().collect(),
            Fields::Unit => vec![],
        }).collect()),
        _ => return None,
    };
    let mut k = crate::wf::Known::default();
    k.add_generics(g);
    if k.tparams.is_empty() {
        return None;
    }
    fn walk(t: &PTy, k: &crate::wf::Known, statics: &crate::fxhash::FxMap<&str, Vec<usize>>, depth: u32) -> Option<(crate::parse::Pos, String)> {
        if depth > 16 {
            return None;
        }
        match t {
            PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => walk(i, k, statics, depth + 1),
            PTy::Tuple(ts) => ts.iter().find_map(|t| walk(t, k, statics, depth + 1)),
            PTy::Fn(ps, r) => ps.iter().chain(std::iter::once(&**r)).find_map(|t| walk(t, k, statics, depth + 1)),
            PTy::Path(None, p) => {
                let s = p.segs.last()?;
                // (one of the program's by its name alone: not a parameter of the item's of that name)
                let req = (p.segs.len() == 1 && s.fn_args.is_none() && !k.tparams.contains(&s.name)).then(|| statics.get(s.name.as_str())).flatten();
                let args: Vec<Option<&PTy>> = s
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        crate::parse::GArg::Ty(t) => Some(Some(t)),
                        crate::parse::GArg::Const(_) => Some(None),
                        _ => None,
                    })
                    .collect();
                if let Some(req) = req {
                    for &i in req {
                        let Some(Some(a)) = args.get(i) else { continue };
                        // (the item's type parameters the argument holds, each declared to outlive 'static or not)
                        let Some(sub) = k.sub_of(a) else { continue };
                        if let Some(n) = sub.iter().find(|n| k.tparams.contains(n) && !k.vague.contains(n) && !k.outlives(n, "'static")) {
                            return Some((p.pos, format!("the parameter type `{n}` may not live long enough (it must outlive `'static`, which the type `{}` requires) (E0310)", s.name)));
                        }
                    }
                }
                args.iter().flatten().find_map(|t| walk(t, k, statics, depth + 1)).or_else(|| {
                    s.args.iter().find_map(|a| match a {
                        crate::parse::GArg::Bind(_, t) => walk(t, k, statics, depth + 1),
                        _ => None,
                    })
                })
            }
            PTy::Dyn(bs) | PTy::Impl(bs) => bs.iter().find_map(|b| match b {
                Bound::Trait(_, p) => p.segs.iter().find_map(|s| {
                    s.args.iter().find_map(|a| match a {
                        crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) => walk(t, k, statics, depth + 1),
                        _ => None,
                    })
                    .or_else(|| s.fn_args.as_ref().and_then(|(ps, r)| ps.iter().chain(r.as_deref()).find_map(|t| walk(t, k, statics, depth + 1))))
                }),
                Bound::Lifetime(_) => None,
            }),
            _ => None,
        }
    }
    let (p, m) = tys.iter().find_map(|t| walk(t, &k, statics, 0))?;
    Some((it.line + p.line.max(1) as usize - 1, p.col as usize + 1, m))
}

/// What an item's types require of traits' `Self: 'x` bounds (the driver checks them against the traits, which
/// only it sees, and only when a trait has such a bound): an impl's self type outlives what its trait requires; a
/// field's trait object's lifetime (`Box<dyn Wedding<'k> + 'r>`), what its trait requires
/// A path in a field's, a type alias's, an associated type's or a const's type that names a struct or enum with
/// lifetimes (adts: their numbers) and writes none (E0106: nothing to elide them to there), or writes another number
/// of them (E0107): `line:col: msg`. Not in fn pointer, `dyn` and `impl` types, which elide
/// (outer: the type parameters of the impl an associated type is in, which its own generics do not list)
pub fn hidden_lifetimes(src: &str, it: &SrcItem, adts: &crate::fxhash::FxMap<&str, usize>, outer: &[String]) -> Option<String> {
    let text = src.get(it.start..it.end)?;
    let f = parse_file(text).ok()?;
    let item = f.items.first()?;
    let (g, tys): (Option<&Generics>, Vec<&PTy>) = match &item.kind {
        ItemKind::Struct(_, g, fields) | ItemKind::Union(_, g, fields) => (Some(g), match fields {
            Fields::Named(v) => v.iter().map(|x| &x.1).collect(),
            Fields::Tuple(v) => v.iter().collect(),
            Fields::Unit => vec![],
        }),
        ItemKind::Enum(_, g, vs) => (Some(g), vs.iter().flat_map(|v| match &v.1 {
            Fields::Named(f) => f.iter().map(|x| &x.1).collect::<Vec<_>>(),
            Fields::Tuple(f) => f.iter().collect(),
            Fields::Unit => vec![],
        }).collect()),
        ItemKind::Type(_, g, _, Some(t)) => (Some(g), vec![t]),
        ItemKind::Const(_, Some(t), _) | ItemKind::Static(_, _, t, _) => (None, vec![t]),
        _ => return None,
    };
    // (a const's or a static's elided lifetimes are 'static (RFC 1623): only a wrong number is an error)
    let statics = matches!(item.kind, ItemKind::Const(..) | ItemKind::Static(..));
    let mut tparams: Vec<&str> = g.map(|g| g.params.iter().filter_map(|p| if let GParam::Ty(n, ..) = p { Some(n.as_str()) } else { None }).collect()).unwrap_or_default();
    tparams.extend(outer.iter().map(|x| x.as_str()));
    fn walk(t: &PTy, adts: &crate::fxhash::FxMap<&str, usize>, tparams: &[&str], statics: bool, depth: u32) -> Option<(crate::parse::Pos, String)> {
        if depth > 16 {
            return None;
        }
        match t {
            PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => walk(i, adts, tparams, statics, depth + 1),
            PTy::Tuple(ts) => ts.iter().find_map(|t| walk(t, adts, tparams, statics, depth + 1)),
            PTy::Path(None, p) => {
                let s = p.segs.last()?;
                if p.segs.len() == 1 && !tparams.contains(&s.name.as_str()) && s.fn_args.is_none() {
                    if let Some(&n) = adts.get(s.name.as_str()) {
                        let m = s.args.iter().filter(|a| matches!(a, crate::parse::GArg::Lifetime(_))).count();
                        if m == 0 && n > 0 && !statics {
                            return Some((p.pos, "missing lifetime specifier (E0106)".to_string()));
                        }
                        if m > 0 && m != n {
                            let w = |k: usize| if k == 1 { "argument" } else { "arguments" };
                            return Some((p.pos, format!("struct takes {n} lifetime {} but {m} lifetime {} supplied (E0107)", w(n), if m == 1 { "argument was" } else { "arguments were" })));
                        }
                    }
                }
                s.args.iter().find_map(|a| match a {
                    crate::parse::GArg::Ty(t) => walk(t, adts, tparams, statics, depth + 1),
                    _ => None,
                })
            }
            _ => None,
        }
    }
    let (p, m) = tys.iter().find_map(|t| walk(t, adts, &tparams, statics, 0))?;
    Some(format!("{}:{}: {m}", it.line + p.line.max(1) as usize - 1, p.col + 1))
}

pub fn trait_obls(src: &str, it: &SrcItem) -> Vec<crate::types::WfObl> {
    let text = &src[it.start..it.end];
    let text = match it.kind {
        LexKind::ImplHeader => format!("{text}}}"),
        _ => text.to_string(),
    };
    let mut out = vec![];
    let Ok(f) = parse_file(&text) else { return out };
    let Some(item) = f.items.first() else { return out };
    match &item.kind {
        ItemKind::Impl(g, Some((false, tp)), self_ty, _) => {
            let mut k = crate::wf::Known::default();
            k.add_generics(g);
            k.implied(self_ty, 0);
            if let Some(sub) = k.sub_of(self_ty).filter(|s| !s.is_empty()) {
                out.extend(k.obl(tp, sub, 1));
            }
        }
        ItemKind::Struct(_, g, fields) | ItemKind::Union(_, g, fields) => {
            let tys: Vec<&PTy> = match fields {
                Fields::Named(v) => v.iter().map(|x| &x.1).collect(),
                Fields::Tuple(v) => v.iter().collect(),
                Fields::Unit => vec![],
            };
            dyn_obls(g, &tys, &mut out);
        }
        ItemKind::Enum(_, g, vars) => {
            let mut tys: Vec<&PTy> = vec![];
            for (_, f, _) in vars {
                match f {
                    Fields::Named(v) => tys.extend(v.iter().map(|x| &x.1)),
                    Fields::Tuple(v) => tys.extend(v.iter()),
                    Fields::Unit => {}
                }
            }
            dyn_obls(g, &tys, &mut out);
        }
        _ => {}
    }
    out
}

/// The last segments' names of the paths t writes (`Erased<'a>`, `Vec<Erased<'a>>`: `Erased`, `Vec`)
fn path_names(t: &PTy, depth: u32) -> Vec<String> {
    let mut out = vec![];
    if depth > 16 {
        return out;
    }
    match t {
        PTy::Path(q, p) => {
            if let Some(q) = q {
                out.extend(path_names(q, depth + 1));
            }
            if let Some(s) = p.segs.last() {
                out.push(s.name.clone());
            }
            for a in p.segs.iter().flat_map(|s| &s.args) {
                if let crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) = a {
                    out.extend(path_names(t, depth + 1));
                }
            }
        }
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => out.extend(path_names(i, depth + 1)),
        PTy::Tuple(ts) => ts.iter().for_each(|t| out.extend(path_names(t, depth + 1))),
        PTy::Fn(ps, r) => {
            ps.iter().for_each(|t| out.extend(path_names(t, depth + 1)));
            out.extend(path_names(r, depth + 1));
        }
        _ => {}
    }
    out
}

/// An impl's associated type's value against the lifetime bounds its trait's declaration gives it (`type Value:
/// 'a;`, E0477): a lifetime of the impl's the value names that the header does not make outlive the bound (`impl<'a>
/// Foo<'static> for &'a i32 { type Value = &'a i32; }`), as (the bound, the value as written). `bounds` are the
/// declaration's, `'static` or the trait's lifetime parameter by its place (`'0`), which the impl's trait path writes
/// `is_alias`: a name of one of the program's type aliases. A value that writes one is not checked: an alias may name
/// a lifetime its body does not use (`type Erased<'a> = ();`), and its body is not read here
pub fn assoc_value_unmet(src: &str, header: &SrcItem, member: &SrcItem, bounds: &[String], is_alias: &dyn Fn(&str) -> bool, lt_bounded: &dyn Fn(&str) -> bool) -> Option<(String, String)> {
    let htext = format!("{}}}", src.get(header.start..header.end)?);
    let hf = parse_file(&htext).ok()?;
    let ItemKind::Impl(g, Some((false, tp)), self_ty, _) = &hf.items.first()?.kind else { return None };
    let mtext = src.get(member.start..member.end)?;
    let mf = parse_file(mtext).ok()?;
    let ItemKind::Type(_, mg, _, Some(value)) = &mf.items.first()?.kind else { return None };
    // (a where clause of the value (`type Value = &'a i32 where Self: 'b;`) may give what the bound needs: not read)
    if !mg.preds.is_empty() {
        return None;
    }
    let mut k = crate::wf::Known::default();
    k.add_generics(g);
    // (what the header's types imply: the self type's and the trait's type arguments' (`Foo<'a, &'a &'b ()>`, 'b: 'a))
    // (a struct or enum there whose declaration bounds its lifetimes (`struct Static<'a: 'static>`) implies those
    // bounds, which Known does not read: not checked)
    if path_names(self_ty, 0).iter().any(|n| lt_bounded(n)) {
        return None;
    }
    k.implied(self_ty, 0);
    for s in &tp.segs {
        for a in &s.args {
            if let crate::parse::GArg::Ty(t) = a {
                if path_names(t, 0).iter().any(|n| lt_bounded(n)) {
                    return None;
                }
                k.implied(t, 0);
            }
        }
    }
    let args: Vec<&String> = tp.segs.last()?.args.iter().filter_map(|a| if let crate::parse::GArg::Lifetime(l) = a { Some(l) } else { None }).collect();
    if path_names(value, 0).iter().any(|n| is_alias(n)) {
        return None;
    }
    let mut named = vec![];
    named_lifetimes_deep(value, &mut named, 0);
    for b in bounds {
        let x = if b == "'static" { b.clone() } else { (*args.get(b[1..].parse::<usize>().ok()?)?).clone() };
        if x == "'_" {
            continue;
        }
        // (the impl's own lifetimes only: one a `for<..>` in the value binds is the value's)
        for l in named.iter().filter(|l| k.lts.contains(l)) {
            if !k.vague.contains(l) && !k.outlives(l, &x) {
                return Some((x, crate::parse::ty_text(value)));
            }
        }
    }
    None
}

fn dyn_obls(g: &Generics, tys: &[&PTy], out: &mut Vec<crate::types::WfObl>) {
    let mut k = None;
    for t in tys.iter().filter(|t| has_dyn_syn(t)) {
        let k = k.get_or_insert_with(|| {
            let mut k = crate::wf::Known::default();
            k.add_generics(g);
            k
        });
        k.dyn_obls(t, None, out, 0);
    }
}

fn has_dyn_syn(t: &PTy) -> bool {
    match t {
        PTy::Dyn(_) => true,
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => has_dyn_syn(i),
        PTy::Tuple(ts) => ts.iter().any(has_dyn_syn),
        PTy::Path(_, p) => p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(t) if has_dyn_syn(t)))),
        _ => false,
    }
}

/// A field `&'static T` needs `T: 'static` declared (it is not inferred, E0310): `\u{3}line\u{3}msg`
fn static_fields(g: &Generics, tys: &[&PTy]) -> Option<String> {
    if g.params.is_empty() {
        return None;
    }
    let mut k = None;
    for t in tys.iter().filter(|t| crate::regions::mentions_static(t)) {
        let k = k.get_or_insert_with(|| {
            let mut k = crate::wf::Known::default();
            k.add_generics(g);
            k
        });
        // (a field's type has no position: the line of the first `'static`, which the driver finds)
        if let Some((p, m)) = k.check(t, true, 0) {
            return Some(match p {
                Some(p) => format!("\u{3}{}\u{3}{m}", p.line),
                None => format!("\u{1}'static\u{1}{m}"),
            });
        }
    }
    None
}

/// Whether an array length in t uses one of the type parameters ps (`[u8; size_of::<T>()]`): rustc rejects it
/// ("generic parameters may not be used in const operations") unless generic_const_exprs is on
fn type_param_in_array_len(t: &PTy, ps: &[&str]) -> bool {
    fn names_param(t: &PTy, ps: &[&str]) -> bool {
        matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && ps.contains(&p.segs[0].name.as_str()))
    }
    fn in_expr(e: &crate::parse::Expr, ps: &[&str]) -> bool {
        use crate::parse::Expr;
        match e {
            Expr::Path(None, p) => p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(t) if names_param(t, ps)))),
            Expr::Call(f, args) => in_expr(f, ps) || args.iter().any(|a| in_expr(a, ps)),
            Expr::Binary(_, l, r, _) => in_expr(l, ps) || in_expr(r, ps),
            Expr::Unary(_, x, _) | Expr::Paren(x) | Expr::Cast(x, _) => in_expr(x, ps),
            _ => false,
        }
    }
    match t {
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) => type_param_in_array_len(i, ps),
        PTy::Array(i, n) => type_param_in_array_len(i, ps) || in_expr(n, ps),
        PTy::Tuple(ts) => ts.iter().any(|t| type_param_in_array_len(t, ps)),
        PTy::Path(q, p) => {
            q.as_deref().is_some_and(|q| type_param_in_array_len(q, ps))
                || p.segs.iter().flat_map(|s| s.args.iter()).any(|a| match a {
                    crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) => type_param_in_array_len(t, ps),
                    _ => false,
                })
        }
        _ => false,
    }
}

fn adt_checks(attrs: &[String], g: &Generics, tys: &[&PTy], union: bool, nfields: usize) -> R<()> {
    // (a type parameter in an array length of a field or of a parameter's default: `struct Foo<T, U = [u8;
    //  size_of::<T>()]>` (tests/ui generics/param-in-ct-in-ty-param-default.rs))
    {
        let tps: Vec<&str> = g.params.iter().filter_map(|p| if let GParam::Ty(n, ..) = p { Some(n.as_str()) } else { None }).collect();
        let defaults = g.params.iter().filter_map(|p| if let GParam::Ty(_, _, Some(d)) = p { Some(d) } else { None });
        if !tps.is_empty() && (tys.iter().any(|t| type_param_in_array_len(t, &tps)) || defaults.into_iter().any(|d| type_param_in_array_len(d, &tps))) {
            return Err("generic parameters may not be used in const operations".to_string());
        }
    }
    // (E0261: the fields name only the lifetimes the type declares)
    {
        let declared: Vec<&str> = g.params.iter().filter_map(|p| match p {
            GParam::Lifetime(l) => Some(l.as_str()),
            _ => None,
        }).collect();
        let mut used = vec![];
        for t in tys {
            named_lifetimes(t, &mut used);
        }
        // (and the parameters' own bounds: `struct S<'a: 'missing>`, `T: 'x`)
        bound_lifetimes(g, &mut used);
        if let Some(l) = undeclared_lifetime(&used, &declared) {
            return Err(format!("use of undeclared lifetime name `{l}` (E0261)"));
        }
        // (E0106: a field's reference names its lifetime, and so does its trait object: `Box<dyn Debug + '_>`)
        if tys.iter().any(|t| lifetime_positions(t).0 > 0 || anon_dyn(t)) {
            return Err("missing lifetime specifier (E0106)".to_string());
        }
    }
    // (E0277: a slice's or array's element must be Sized)
    if let Some(t) = tys.iter().find_map(|t| unsized_elem(t)) {
        return Err(format!("the size for values of type `{t}` cannot be known at compilation time (E0277)"));
    }
    let mut names = vec![];
    let mut lts = vec![];
    for t in tys {
        ty_names(t, &mut names);
        let (mut e, mut o) = (false, false);
        lifetimes(t, &mut lts, &mut e, &mut o);
    }
    for (t, bs) in &g.preds {
        ty_names(t, &mut names);
        for b in bs {
            if let Bound::Trait(_, bp) = b {
                let bt = PTy::Path(None, bp.clone());
                ty_names(&bt, &mut names);
                let (mut e, mut o) = (false, false);
                lifetimes(&bt, &mut lts, &mut e, &mut o);
            }
        }
    }
    // (a parameter used in another one's bound counts: `struct P<A, F: FnOnce() -> [A; 10]>(F)`)
    for p in &g.params {
        if let GParam::Ty(_, bs, _) = p {
            for b in bs {
                match b {
                    Bound::Trait(_, bp) => {
                        let bt = PTy::Path(None, bp.clone());
                        ty_names(&bt, &mut names);
                        let (mut e, mut o) = (false, false);
                        lifetimes(&bt, &mut lts, &mut e, &mut o);
                    }
                    Bound::Lifetime(l) => lts.push(l.clone()),
                }
            }
        }
    }
    for p in &g.params {
        match p {
            GParam::Lifetime(l) if !lts.contains(l) => return Err(format!("parameter `{l}` is never used")),
            GParam::Ty(n, _, d) => {
                if !names.iter().any(|x| x == n) {
                    return Err(format!("parameter `{n}` is never used"));
                }
                if let Some(d) = d {
                    let mut dn = vec![];
                    ty_names(d, &mut dn);
                    if dn.iter().any(|x| x == "Self") {
                        return err("generic parameters cannot use `Self` in their defaults");
                    }
                }
            }
            _ => {}
        }
    }
    let derives = derive_names(attrs);
    for d in &derives {
        if !matches!(d.as_str(), "Clone" | "Copy" | "Debug" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Default") {
            // (`\u{5}word\u{5}msg`: at the word in the item's text, the derive's name)
            // (`#[derive(core::RustcDecodable)]`: a name std's root does not have)
            if let Some(k) = derive_root_of(attrs, d) {
                return Err(format!("\u{5}{d}\u{5}cannot find `{d}` in `{k}`"));
            }
            return Err(format!("\u{5}{d}\u{5}cannot find derive macro `{d}` in this scope"));
        }
    }
    if union {
        if nfields == 0 {
            return err("unions cannot have zero fields");
        }
        if let Some(d) = derives.iter().find(|d| !matches!(d.as_str(), "Clone" | "Copy")) {
            return Err(format!("\u{5}{d}\u{5}this trait cannot be derived for unions: `{d}`"));
        }
        if derives.iter().any(|d| d == "Clone") && !derives.iter().any(|d| d == "Copy") {
            return err("the trait bound `Copy` is not satisfied: a union can derive `Clone` only with `Copy`");
        }
        // (E0740: fields that would need dropping)
        for t in tys {
            let owning = match t {
                PTy::Dyn(_) | PTy::Slice(_) => true,
                PTy::Path(None, p) => p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "String" | "Vec" | "Box" | "Rc" | "Arc" | "HashMap" | "str")),
                _ => false,
            };
            if owning {
                // (at the field's type, as rustc)
                let m = "field must implement `Copy` or be wrapped in `ManuallyDrop<...>` to be used in a union";
                return Err(match crate::wf::pos_of(t) {
                    Some(p) => format!("\u{3}{}\u{3}{m}", p.line),
                    None => m.to_string(),
                });
            }
        }
    }
    Ok(())
}

/// Lifetimes in a type (`'_` and elided `&` count as "elided")
fn lifetimes(t: &PTy, out: &mut Vec<String>, elided: &mut bool, opaque: &mut bool) {
    match t {
        PTy::Ref(l, _, i) => {
            match l {
                None => *elided = true,
                Some(l) if l == "'_" => *elided = true,
                Some(l) => out.push(l.clone()),
            }
            lifetimes(i, out, elided, opaque);
        }
        PTy::Path(q, p) => {
            if let Some(q) = q {
                lifetimes(q, out, elided, opaque);
            }
            for s in &p.segs {
                for a in &s.args {
                    match a {
                        crate::parse::GArg::Lifetime(l) if l == "'_" => *elided = true,
                        crate::parse::GArg::Lifetime(l) => out.push(l.clone()),
                        crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) => lifetimes(t, out, elided, opaque),
                        _ => {}
                    }
                }
                // (`Fn(&'a u8) -> &'a u8`: as a fn pointer type, its own elided ones aside)
                if let Some((ps, r)) = &s.fn_args {
                    let (mut e, mut o) = (false, false);
                    for t in ps.iter().chain(r.as_deref()) {
                        lifetimes(t, out, &mut e, &mut o);
                    }
                }
            }
        }
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => lifetimes(i, out, elided, opaque),
        PTy::Tuple(ts) => {
            for t in ts {
                lifetimes(t, out, elided, opaque);
            }
        }
        PTy::Impl(bs) | PTy::Dyn(bs) => {
            *opaque = true;
            for b in bs {
                match b {
                    Bound::Lifetime(l) => out.push(l.clone()),
                    Bound::Trait(_, p) => lifetimes(&PTy::Path(None, p.clone()), out, elided, opaque),
                }
            }
        }
        PTy::Macro(_) | PTy::Infer => *opaque = true,
        PTy::Fn(ps, r) => {
            // (the lifetimes of a fn pointer type are its own: only named ones count as uses)
            let (mut e, mut o) = (false, false);
            for t in ps {
                lifetimes(t, out, &mut e, &mut o);
            }
            lifetimes(r, out, &mut e, &mut o);
        }
        PTy::Never => {}
    }
}

/// Add to ls the lifetimes the signature declares (or a parameter's projection implies) to outlive one of them
#[cold]
fn outlived_by(sig: &PSig, ls: &mut Vec<String>) {
    let mut longer: Vec<(String, String)> = vec![];
    for (t, bs) in &sig.generics.preds {
        if let PTy::Path(None, p) = t {
            if let [s] = p.segs.as_slice() {
                if s.name.starts_with('\'') {
                    for b in bs {
                        if let Bound::Lifetime(l) = b {
                            longer.push((s.name.clone(), l.clone()));
                        }
                    }
                }
            }
        }
    }
    for (_, t) in &sig.params {
        crate::regions::qself_outlives(t, &mut longer, 0);
    }
    while let Some((a, _)) = longer.iter().find(|(a, b)| ls.contains(b) && !ls.contains(a)) {
        ls.push(a.clone());
    }
}

/// SigSyn::ret_from: the elision rules (`&self` methods return borrows of self, else the only argument with
/// lifetimes) and named lifetimes (`fn f<'a>(x: &'a T, y: &U) -> &'a T` borrows from x only)
fn ret_from(sig: &PSig) -> u64 {
    const KNOWN: u64 = 1 << 63;
    let Some(ret) = &sig.ret else { return KNOWN };
    let (mut rl, mut relided, mut opaque) = (vec![], false, false);
    lifetimes(ret, &mut rl, &mut relided, &mut opaque);
    if opaque {
        return opaque_from(sig);
    }
    // (a lifetime declared to outlive one of the result's may be the result's too: `fn f<'a, 'b: 'a>(x: &'b T) -> &'a T`)
    // (and so may one a parameter's projection implies to: `<&'a &'b () as Tr>::Type`)
    if !rl.is_empty() {
        outlived_by(sig, &mut rl);
    }
    let mut inputs: Vec<&PTy> = vec![];
    if let Some(r) = &sig.recv_ty {
        inputs.push(r);
    }
    inputs.extend(sig.params.iter().map(|p| &p.1));
    if inputs.len() > 62 {
        return 0;
    }
    let mut mask = 0u64;
    for (i, t) in inputs.iter().enumerate() {
        if let PTy::Ref(Some(l), _, _) = t {
            if rl.contains(l) && l != "'static" {
                mask |= 1 << i;
            }
        }
    }
    if relided {
        let recv_ref = matches!(sig.recv_ty, Some(PTy::Ref(..)));
        if recv_ref {
            mask |= 1;
        } else {
            let with: Vec<usize> = (0..inputs.len())
                .filter(|&i| {
                    let (mut l, mut e, mut o) = (vec![], false, false);
                    lifetimes(inputs[i], &mut l, &mut e, &mut o);
                    e || !l.is_empty()
                })
                .collect();
            match with.as_slice() {
                [i] if matches!(inputs[*i], PTy::Ref(..)) => mask |= 1 << i,
                [_] => {}
                _ => return 0,
            }
        }
    }
    KNOWN | mask
}

/// SigSyn::ret_from of a result `impl Trait` (OPAQUE_FROM): the arguments it may borrow from. Edition 2024 captures
/// every lifetime in scope (the arguments' elided ones too) unless `use<..>` lists them; 2021 only those its bounds
/// name (`impl Display + 'a`). 0 (not known) for another opaque result, or more than 31 arguments
fn opaque_from(sig: &PSig) -> u64 {
    let Some(PTy::Impl(bs)) = &sig.ret else { return 0 };
    let mut inputs: Vec<&PTy> = vec![];
    if let Some(r) = &sig.recv_ty {
        inputs.push(r);
    }
    inputs.extend(sig.params.iter().map(|p| &p.1));
    if inputs.len() > 31 {
        return 0;
    }
    // (`use<'a, T>`: parsed as a bound named `use`)
    let used: Option<Vec<String>> = bs.iter().find_map(|b| match b {
        Bound::Trait(_, p) if p.segs.len() == 1 && p.segs[0].name == "use" => {
            Some(p.segs[0].args.iter().filter_map(|a| if let crate::parse::GArg::Lifetime(l) = a { Some(l.clone()) } else { None }).collect())
        }
        _ => None,
    });
    let mut named: Vec<String> = vec![];
    for b in bs {
        match b {
            Bound::Lifetime(l) => named.push(l.clone()),
            Bound::Trait(_, p) if !(p.segs.len() == 1 && p.segs[0].name == "use") => {
                let (mut e, mut o) = (false, false);
                lifetimes(&PTy::Path(None, p.clone()), &mut named, &mut e, &mut o);
            }
            _ => {}
        }
    }
    let mask = |pick: &dyn Fn(&[String], bool) -> bool| {
        let mut m = 0u64;
        for (i, t) in inputs.iter().enumerate() {
            let (mut l, mut e, mut o) = (vec![], false, false);
            lifetimes(t, &mut l, &mut e, &mut o);
            l.retain(|x| x != "'static");
            if pick(&l, e) {
                m |= 1 << i;
            }
        }
        m
    };
    let (m24, m21) = match &used {
        Some(u) => {
            let m = mask(&|l: &[String], _: bool| l.iter().any(|x| u.contains(x)));
            (m, m)
        }
        None => (mask(&|l: &[String], e: bool| e || !l.is_empty()), mask(&|l: &[String], _: bool| l.iter().any(|x| named.contains(x)))),
    };
    crate::types::OPAQUE_FROM | m24 | m21 << 31
}

/// A bound's reading, kept by its text for the thread: the readers below parse the bound as a file of its own each
/// time, and the same bounds are read again and again (each instance of a generic fn reads its parameters' bounds,
/// `F: Fn(u32) -> u32` once per call). Each reading is a function of the text alone
fn memo<T: Clone + 'static>(cache: &'static std::thread::LocalKey<std::cell::RefCell<crate::fxhash::FxMap<String, T>>>, s: &str, read: impl FnOnce(&str) -> T) -> T {
    if let Some(r) = cache.with(|c| c.borrow().get(s).cloned()) {
        return r;
    }
    let r = read(s);
    cache.with(|c| c.borrow_mut().insert(s.to_string(), r.clone()));
    r
}

thread_local! {
    static FN_BOUNDS: std::cell::RefCell<crate::fxhash::FxMap<String, Option<(String, Vec<TySyn>, TySyn)>>> = Default::default();
    static BOUND_ARGS: std::cell::RefCell<crate::fxhash::FxMap<String, Option<(String, Vec<TySyn>)>>> = Default::default();
    static ASSOC_BINDINGS: std::cell::RefCell<crate::fxhash::FxMap<String, Vec<(String, TySyn)>>> = Default::default();
}

/// (kind, arguments, return value) if it is a bound of the form `Fn(A, B) -> R`
pub fn parse_fn_bound(s: &str) -> Option<(String, Vec<TySyn>, TySyn)> {
    // (one that is no Fn bound is told by its first word, before the cache)
    if !["FnOnce", "FnMut", "Fn"].into_iter().any(|k| s.starts_with(k) && s[k.len()..].trim_start().starts_with('(')) {
        return None;
    }
    memo(&FN_BOUNDS, s, read_fn_bound)
}

fn read_fn_bound(s: &str) -> Option<(String, Vec<TySyn>, TySyn)> {
    let kind = ["FnOnce", "FnMut", "Fn"].into_iter().find(|k| s.starts_with(k) && s[k.len()..].trim_start().starts_with('('))?;
    let f = parse_file(&format!("type __B = dyn {s};")).ok()?;
    let ItemKind::Type(_, _, _, Some(PTy::Dyn(bs))) = &f.items.first()?.kind else { return None };
    let Some(Bound::Trait(_, p)) = bs.first() else { return None };
    let (ps, r) = p.segs.last()?.fn_args.clone()?;
    let mut args = Vec::new();
    for t in &ps {
        args.push(tysyn(t, None).ok()?);
    }
    let ret = match r {
        None => TySyn::Prim(Ty::Unit),
        Some(t) => tysyn(&t, None).ok()?,
    };
    Some((kind.to_string(), args, ret))
}

/// A bound `a::Tr<A, B>` (or `Tr<A, Item = X>`) as its trait's name and its type arguments, without its
/// associated-type bindings: None when a type argument cannot be read (a lifetime is left out)
pub fn bound_args(s: &str) -> Option<(String, Vec<TySyn>)> {
    let Some(lt) = s.find('<') else {
        let n = s.trim();
        return Some((n.rsplit("::").next().unwrap_or(n).to_string(), vec![]));
    };
    memo(&BOUND_ARGS, s, |s| read_bound_args(s, lt))
}

fn read_bound_args(s: &str, lt: usize) -> Option<(String, Vec<TySyn>)> {
    let f = parse_file(&format!("type __B = dyn {s};")).ok()?;
    let ItemKind::Type(_, _, _, Some(PTy::Dyn(bs))) = &f.items.first()?.kind else { return None };
    let Some(Bound::Trait(_, p)) = bs.first() else { return None };
    let last = p.segs.last()?;
    if last.fn_args.is_some() {
        return None;
    }
    let mut args = Vec::new();
    for a in &last.args {
        match a {
            crate::parse::GArg::Ty(t) => args.push(tysyn(t, None).ok()?),
            // (a const argument as impl headers keep it (`Tr<2>`): not left out, or `Tr<1>`'s impl would answer)
            crate::parse::GArg::Const(e) => args.push(crate::ast_types::const_arg_syn(e).ok()?),
            _ => {}
        }
    }
    let n = s[..lt].trim();
    Some((n.rsplit("::").next().unwrap_or(n).to_string(), args))
}

/// A type written as text (`<T as Tr>::Assoc`, `W<u8>`, `()`), as a TySyn; self_ty: what `Self` is
pub fn ty_of_text(s: &str, self_ty: Option<&TySyn>) -> Option<TySyn> {
    let f = parse_file(&format!("type __W = {s};")).ok()?;
    let ItemKind::Type(_, _, _, Some(t)) = &f.items.first()?.kind else { return None };
    tysyn(t, self_ty).ok()
}

/// The arguments a call through a closure bound (`Fn(&T) -> &T`) returns a borrow of, by its lifetimes: those whose
/// types name a lifetime its result names (`for<'a> Fn(&'a T, &u8) -> &'a T`: the first), or the only one with a
/// reference when the result's is elided (`Fn(&T) -> &T`). Not a `'static` result's. Empty when it returns none
pub fn fn_bound_ret_from(s: &str) -> Vec<usize> {
    fn refs(t: &PTy, named: &mut Vec<String>, elided: &mut bool) {
        match t {
            PTy::Ref(l, _, i) => {
                match l.as_deref() {
                    Some("'_") | None => *elided = true,
                    Some(l) => named.push(l.to_string()),
                }
                refs(i, named, elided);
            }
            PTy::Path(None, p) => {
                for sg in &p.segs {
                    for a in &sg.args {
                        match a {
                            crate::parse::GArg::Lifetime(l) if l == "'_" => *elided = true,
                            crate::parse::GArg::Lifetime(l) => named.push(l.clone()),
                            crate::parse::GArg::Ty(t) => refs(t, named, elided),
                            _ => {}
                        }
                    }
                }
            }
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => refs(i, named, elided),
            PTy::Tuple(ts) => ts.iter().for_each(|t| refs(t, named, elided)),
            _ => {}
        }
    }
    let Some(kind) = ["FnOnce", "FnMut", "Fn"].into_iter().find(|k| s.starts_with(k) && s[k.len()..].trim_start().starts_with('(')) else { return vec![] };
    let _ = kind;
    let Ok(f) = parse_file(&format!("type __B = dyn {s};")) else { return vec![] };
    let Some(ItemKind::Type(_, _, _, Some(PTy::Dyn(bs)))) = f.items.first().map(|i| &i.kind) else { return vec![] };
    let Some(Bound::Trait(_, p)) = bs.first() else { return vec![] };
    let Some((ps, Some(r))) = p.segs.last().and_then(|s| s.fn_args.clone()) else { return vec![] };
    let (mut rn, mut re) = (vec![], false);
    refs(&r, &mut rn, &mut re);
    rn.retain(|l| l != "'static");
    let params: Vec<(Vec<String>, bool)> = ps
        .iter()
        .map(|t| {
            let (mut n, mut e) = (vec![], false);
            refs(t, &mut n, &mut e);
            (n, e)
        })
        .collect();
    let with_refs: Vec<usize> = (0..params.len()).filter(|&i| params[i].1 || !params[i].0.is_empty()).collect();
    (0..params.len())
        .filter(|&i| params[i].0.iter().any(|l| rn.contains(l)) || (re && with_refs == [i]))
        .collect()
}

/// The associated type bindings (name, type) of a bound `Iterator<Item = T>`
pub fn assoc_bindings(s: &str) -> Vec<(String, TySyn)> {
    memo(&ASSOC_BINDINGS, s, read_assoc_bindings)
}

fn read_assoc_bindings(s: &str) -> Vec<(String, TySyn)> {
    let mut out = Vec::new();
    let Ok(f) = parse_file(&format!("type __B = dyn {s};")) else { return out };
    let Some(it) = f.items.first() else { return out };
    let ItemKind::Type(_, _, _, Some(PTy::Dyn(bs))) = &it.kind else { return out };
    let Some(Bound::Trait(_, p)) = bs.first() else { return out };
    let Some(last) = p.segs.last() else { return out };
    for a in &last.args {
        if let crate::parse::GArg::Bind(n, t) = a {
            if let Ok(ts) = tysyn(t, None) {
                out.push((n.clone(), ts));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- Closure lifting

/// All the patterns within patterns (collects the bound names)
fn pats_in_block(b: &Block, out: &mut Vec<String>) {
    for s in &b.stmts {
        match s {
            Stmt::Let(p, _, init, els, ..) => {
                ast_closure::pat_idents(p, out);
                if let Some(e) = init {
                    pats_in_expr(e, out);
                }
                if let Some(b) = els {
                    pats_in_block(b, out);
                }
            }
            Stmt::Expr(e, _) => pats_in_expr(e, out),
            Stmt::Item(_) => {}
        }
    }
}

fn pats_in_expr(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Let(p, x) => {
            ast_closure::pat_idents(p, out);
            pats_in_expr(x, out);
        }
        Expr::Match(x, arms) => {
            pats_in_expr(x, out);
            for a in arms {
                ast_closure::pat_idents(&a.pat, out);
                if let Some(g) = &a.guard {
                    pats_in_expr(g, out);
                }
                pats_in_expr(&a.body, out);
            }
        }
        Expr::For(_, p, x, b) => {
            ast_closure::pat_idents(p, out);
            pats_in_expr(x, out);
            pats_in_block(b, out);
        }
        Expr::Closure(_, ps, _, body, _) => {
            for (p, _) in ps {
                ast_closure::pat_idents(p, out);
            }
            pats_in_expr(body, out);
        }
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => pats_in_block(b, out),
        Expr::If(c, t, els) => {
            pats_in_expr(c, out);
            pats_in_block(t, out);
            if let Some(x) = els {
                pats_in_expr(x, out);
            }
        }
        Expr::While(_, c, b) => {
            pats_in_expr(c, out);
            pats_in_block(b, out);
        }
        Expr::Macro(..) => {}
        _ => {
            let mut kids: Vec<Expr> = Vec::new();
            ast_closure::each_child_pub(e, &mut |c| kids.push(c.clone()));
            for k in &kids {
                pats_in_expr(k, out);
            }
        }
    }
}

/// Names bound anywhere in a fn (including nested closures)
fn bound_in_fn(sig: &PSig, body: &Block) -> Vec<String> {
    let mut v = Vec::new();
    if sig.recv_ty.is_some() {
        v.push("self".to_string());
    }
    for (p, _) in &sig.params {
        ast_closure::pat_idents(p, &mut v);
    }
    pats_in_block(body, &mut v);
    v
}

/// From `let n = ..` in the body, the type of a variable captured by the returned closure (an annotation or the literal of the initializer)
fn local_ty_syn(b: &Block, n: &str, int_hint: Option<Ty>) -> Option<TySyn> {
    let mut found = None;
    for st in &b.stmts {
        let Stmt::Let(pat, ann, init, _, ..) = st else { continue };
        let Pat::Ident(_, _, name, None) = pat else { continue };
        if name != n {
            continue;
        }
        if let Some(t) = ann {
            found = tysyn(t, None).ok();
            continue;
        }
        found = match init {
            Some(Expr::Lit(t, _)) => lit_ty(t, int_hint),
            Some(Expr::Call(f, _)) => match &**f {
                Expr::Path(None, p) if p.segs.len() == 2 && p.segs[0].name == "String" => Some(TySyn::Named("String".into())),
                _ => None,
            },
            Some(Expr::MethodCall(_, m, _, _, _)) if m == "to_string" => Some(TySyn::Named("String".into())),
            _ => None,
        };
    }
    found
}

/// A type from literal text (integers: suffix → context → i32)
fn lit_ty(t: &str, int_hint: Option<Ty>) -> Option<TySyn> {
    if t == "true" || t == "false" {
        return Some(TySyn::Prim(Ty::Bool));
    }
    if t.starts_with('\'') {
        return Some(TySyn::Prim(Ty::Char));
    }
    if t.starts_with('"') || t.starts_with("r\"") || t.starts_with("r#") {
        return Some(TySyn::Ref(false, Box::new(TySyn::Str)));
    }
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let hex = t.starts_with("0x") || t.starts_with("0b") || t.starts_with("0o");
    // Suffix: the letters after the digits / `_` / (a-f if hex)
    let body = if hex { &t[2..] } else { t };
    let suf_at = body
        .char_indices()
        .find(|&(_, c)| !(c.is_ascii_digit() || c == '_' || c == '.' || (hex && c.is_ascii_hexdigit()) || (!hex && (c == 'e' || c == 'E'))))
        .map(|(i, _)| i)
        .unwrap_or(body.len());
    let suffix = &body[suf_at..];
    let is_float = !hex && (body[..suf_at].contains('.') || body[..suf_at].contains('e') || body[..suf_at].contains('E') || suffix == "f32" || suffix == "f64");
    if is_float {
        return Some(TySyn::Prim(if suffix == "f32" { Ty::F32 } else { Ty::F64 }));
    }
    Some(TySyn::Prim(Ty::from_suffix(suffix).or(int_hint).unwrap_or(Ty::I32)))
}

/// `__implN` that no type parameter has yet (`fn f<__impl1>(x: __impl1, y: Option<impl Copy>)`)
fn fresh_impl_name(generics: &[(String, Vec<String>)]) -> String {
    let mut k = generics.len();
    while generics.iter().any(|(n, _)| *n == format!("__impl{k}")) {
        k += 1;
    }
    format!("__impl{k}")
}

/// t with each `impl Trait` in it replaced by a new type parameter `__implN` (added to generics with its bounds; its
/// lifetime bounds to lts, `impl Debug + 'static`: (`__implN`, `'static`))
fn lift_impls(t: &PTy, generics: &mut Vec<(String, Vec<String>)>, lts: &mut Vec<(String, String)>) -> R<PTy> {
    use crate::parse::{GArg, Seg};
    Ok(match t {
        PTy::Impl(bs) => {
            let g = fresh_impl_name(generics);
            lts.extend(bs.iter().filter_map(|b| if let Bound::Lifetime(l) = b { Some((g.clone(), l.clone())) } else { None }));
            generics.push((g.clone(), bound_names(bs)?));
            PTy::Path(None, Path { global: false, segs: vec![Seg { name: g.clone(), args: vec![], fn_args: None, text: g }], pos: Default::default() })
        }
        PTy::Path(q, p) => {
            let mut p = p.clone();
            for s in &mut p.segs {
                for a in &mut s.args {
                    if let GArg::Ty(x) | GArg::Bind(_, x) = a {
                        *x = lift_impls(x, generics, lts)?;
                    }
                }
            }
            PTy::Path(q.clone(), p)
        }
        PTy::Ref(l, m, i) => PTy::Ref(l.clone(), *m, Box::new(lift_impls(i, generics, lts)?)),
        PTy::Ptr(m, i) => PTy::Ptr(*m, Box::new(lift_impls(i, generics, lts)?)),
        PTy::Slice(i) => PTy::Slice(Box::new(lift_impls(i, generics, lts)?)),
        PTy::Array(i, n) => PTy::Array(Box::new(lift_impls(i, generics, lts)?), n.clone()),
        PTy::Tuple(v) => PTy::Tuple(v.iter().map(|x| lift_impls(x, generics, lts)).collect::<R<Vec<_>>>()?),
        t => t.clone(),
    })
}

/// whether an `impl Trait` appears inside t
fn nested_impl(t: &PTy) -> bool {
    match t {
        PTy::Impl(_) => true,
        PTy::Path(q, p) => {
            q.as_deref().is_some_and(nested_impl)
                || p.segs.iter().any(|s| {
                    s.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) if nested_impl(t)))
                        || s.fn_args.as_ref().is_some_and(|(v, r)| v.iter().any(nested_impl) || r.as_deref().is_some_and(nested_impl))
                })
        }
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => nested_impl(i),
        PTy::Tuple(v) => v.iter().any(nested_impl),
        PTy::Fn(v, r) => v.iter().any(nested_impl) || nested_impl(r),
        _ => false,
    }
}

/// whether a `_` appears in t (not inside `impl Trait`, whose bounds are not types)
pub fn has_infer(t: &PTy) -> bool {
    match t {
        PTy::Infer => true,
        PTy::Path(q, p) => {
            q.as_deref().is_some_and(has_infer)
                || p.segs.iter().any(|s| {
                    s.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) if has_infer(t)))
                        || s.fn_args.as_ref().is_some_and(|(v, r)| v.iter().any(has_infer) || r.as_deref().is_some_and(has_infer))
                })
        }
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => has_infer(i),
        PTy::Tuple(v) => v.iter().any(has_infer),
        PTy::Fn(v, r) => v.iter().any(has_infer) || has_infer(r),
        _ => false,
    }
}

/// t with each `impl Trait` in it as `_`
fn impl_as_infer(t: &PTy) -> PTy {
    use crate::parse::GArg;
    let b = |i: &PTy| Box::new(impl_as_infer(i));
    match t {
        PTy::Impl(_) => PTy::Infer,
        PTy::Path(q, p) => {
            let mut p = p.clone();
            for s in &mut p.segs {
                for a in &mut s.args {
                    match a {
                        GArg::Ty(t) | GArg::Bind(_, t) => *t = impl_as_infer(t),
                        _ => {}
                    }
                }
                if let Some((v, r)) = &mut s.fn_args {
                    for t in v.iter_mut() {
                        *t = impl_as_infer(t);
                    }
                    if let Some(r) = r {
                        **r = impl_as_infer(r);
                    }
                }
            }
            PTy::Path(q.as_deref().map(b), p)
        }
        PTy::Ref(l, m, i) => PTy::Ref(l.clone(), *m, b(i)),
        PTy::Ptr(m, i) => PTy::Ptr(*m, b(i)),
        PTy::Slice(i) => PTy::Slice(b(i)),
        PTy::Array(i, n) => PTy::Array(b(i), n.clone()),
        PTy::Tuple(v) => PTy::Tuple(v.iter().map(impl_as_infer).collect()),
        PTy::Fn(v, r) => PTy::Fn(v.iter().map(impl_as_infer).collect(), b(r)),
        t => t.clone(),
    }
}

/// A parameter's name (if it is an identifier pattern)
fn pat_name(p: &Pat) -> Option<&str> {
    match p {
        Pat::Ident(_, _, n, None) => Some(n),
        _ => None,
    }
}

fn lift_closures(sig_ast: &PSig, body: &Block, fname: &str, sig: &mut SigSyn, self_ty: Option<&TySyn>, sm: &mut Summary) -> R<()> {
    let cs = ast_closure::closures_of(body);
    if cs.is_empty() {
        if sig.ret == TySyn::Named(IMPL_RET.into()) {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
        }
        return Ok(());
    }
    let bound = bound_in_fn(sig_ast, body);
    let mut out: Vec<ClosureSyn> = Vec::new();
    for (k, c) in cs.iter().enumerate() {
        let Expr::Closure(mv, cps, cret, _, _) = c else { continue };
        let name = format!("{fname}::{{closure#{k}}}");
        let mut generics: Vec<String> = sig.generics.iter().map(|g| g.0.clone()).collect();
        let n_parent = generics.len() as u64;
        let caps: Vec<String> = ast_closure::free_vars(c).into_iter().filter(|n| bound.contains(n)).collect();
        let mut fields = Vec::new();
        for (i, n) in caps.iter().enumerate() {
            let g = format!("__C{i}");
            fields.push((n.clone(), TySyn::Named(g.clone())));
            generics.push(g);
        }
        let mut params = Vec::new();
        for (i, (_, t)) in cps.iter().enumerate() {
            match t {
                Some(t) => params.push(tysyn(t, self_ty)?),
                None => {
                    let g = format!("__P{i}");
                    params.push(TySyn::Named(g.clone()));
                    generics.push(g);
                }
            }
        }
        generics.push("__R".into());
        let ret_ann = match cret {
            None => None,
            Some(t) => Some(tysyn(t, self_ty)?),
        };
        let env = TySyn::App(name.clone(), generics.iter().map(|g| TySyn::Named(g.clone())).collect());
        params.insert(0, TySyn::Ptr(true, Box::new(env)));
        let mut gs: Vec<(String, Vec<String>)> = sig.generics.clone();
        for g in &generics[n_parent as usize..] {
            gs.push((g.clone(), Vec::new()));
        }
        let csig = SigSyn {
            params: params,
            ret: TySyn::Named("__R".into()),
            symbol: format!("{}.c{k}", sig.symbol),
            impl_of: sig.impl_of.clone(),
            generics: gs,
            has_body: true,
            self_ty: sig.self_ty.clone(),
            ..Default::default()
        };
        let assigns = ast_closure::assigns_capture(c, &caps);
        let mut moved: Vec<bool> = Vec::new();
        let mut deref: Vec<bool> = Vec::new();
        let mutated: Vec<bool> = caps.iter().map(|n| ast_closure::mutates_capture(c, n)).collect();
        for n in &caps {
            moved.push(*mv || (ast_closure::moves_capture(c, &[n.clone()]) && !assigns));
            // (a parameter that is a shared reference (`h: &T`, `&self`), used only through a deref of it: edition 2021
            // captures `*h` by reference, the same as a copy of h, which the closure may then hold past the fn. Not
            // where the fn's body binds the name again)
            // (`&self` / `&'a self`, not `&mut self`)
            let shared_ref = (n == "self" && sig_ast.recv.as_deref().is_some_and(|r| r.starts_with('&') && !r.contains("mut ") && r.ends_with("self")))
                || sig_ast.params.iter().any(|(p, t)| pat_name(p) == Some(n.as_str()) && matches!(t, PTy::Ref(_, false, _)));
            deref.push(!*mv && shared_ref && ast_closure::deref_uses_only(c, n) && !body.stmts.iter().any(|s| match s {
                Stmt::Let(p, _, init, _, ..) => ast_closure::pat_binds_name_pub(p, n) || init.as_ref().is_some_and(|x| ast_closure::binds_name(x, n, 0)),
                Stmt::Expr(x, _) => ast_closure::binds_name(x, n, 0),
                _ => false,
            }));
        }
        let by_move = *mv || (!moved.is_empty() && moved.iter().all(|&m| m));
        out.push(ClosureSyn {
            name: name,
            generics: generics,
            fields: fields,
            sig: csig,
            by_move: by_move,
            moved: moved,
            ret_ann: ret_ann,
            n_parent: n_parent,
            fake_reads: ast_closure::fake_reads(c).into_iter().filter(|n| bound.contains(n)).collect(),
            deref,
            mutated,
        });
    }
    // `-> impl Fn(A) -> B { move |x| .. }`: the type is the struct of the trailing closure
    if sig.ret == TySyn::Named(IMPL_RET.into()) {
        let mut tail = match body.stmts.last() {
            Some(Stmt::Expr(e, false)) => Some(e),
            _ => None,
        };
        while let Some(Expr::Paren(p)) = tail {
            tail = Some(&**p);
        }
        let Some(tc @ Expr::Closure(_, tps, _, _, _)) = tail else {
            sig.ret = TySyn::Named(OPAQUE_RET.into());
            sm.closures = out;
            return Ok(());
        };
        let key = format!("{tc:?}");
        let k = cs.iter().position(|c| format!("{c:?}") == key).ok_or("internal: tail closure")?;
        let Some(PTy::Impl(ibs)) = &sig_ast.ret else { return err("internal: impl return") };
        let bs = bound_names(ibs)?;
        let Some((_, bargs, bret)) = bs.iter().find_map(|b| parse_fn_bound(b)) else {
            return err("`impl Trait` return types must be `impl Fn(..) -> ..`");
        };
        let cl = &out[k];
        let mut args: Vec<TySyn> = sig.generics.iter().map(|g| TySyn::Named(g.0.clone())).collect();
        let skip = sig.has_self as usize;
        for (n, _) in &cl.fields {
            if cl.by_move {
                let ty = sig_ast.params.iter().zip(sig.params.iter().skip(skip)).find_map(|((p, _), t)| {
                    if pat_name(p) == Some(n.as_str()) { Some(t.clone()) } else { None }
                });
                let int_hint = match &bret {
                    TySyn::Prim(t) if t.is_int() => Some(*t),
                    _ => None,
                };
                match ty.or_else(|| local_ty_syn(body, n, int_hint)) {
                    Some(t) => args.push(t),
                    None => {
                        return Err(format!(
                            "a returned closure can only capture parameters and locals with an obvious type (`{n}`; annotate it: `let {n}: T = ..`)"
                        ));
                    }
                }
            } else {
                return err("a returned closure that captures must be `move`");
            }
        }
        for (i, (_, t)) in tps.iter().enumerate() {
            if t.is_none() {
                match bargs.get(i) {
                    Some(t) => args.push(t.clone()),
                    None => return err("closure has more parameters than the bound"),
                }
            }
        }
        args.push(cl.ret_ann.clone().unwrap_or(bret));
        sig.ret = TySyn::App(cl.name.clone(), args);
    }
    sm.closures = out;
    Ok(())
}

// ---------------------------------------------------------------- Summary

/// E0106 in a fn pointer type or an `Fn(..) -> R` bound: its return type elides a lifetime, and its parameters do not
/// have exactly one lifetime to take it from
/// The input lifetimes of a fn pointer type / `Fn(..)` bound for elision: each elided one counts by itself, a named
/// one once however often it is written. With the count, whether every input was understood: a path type written
/// without lifetime arguments may have lifetime parameters of its own (`fn(View) -> &u8` with `struct View<'a>`)
fn input_lifetimes(ps: &[PTy]) -> (usize, bool) {
    const NO_LTS: &[&str] = &[
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str",
        "String", "Vec", "Option", "Result", "Box", "Rc", "Arc", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque", "Self",
    ];
    fn go(t: &PTy, named: &mut Vec<String>, elided: &mut usize) -> bool {
        match t {
            PTy::Ref(l, _, i) => {
                match l.as_deref() {
                    None | Some("'_") => *elided += 1,
                    Some(l) => {
                        if !named.iter().any(|x| x == l) {
                            named.push(l.to_string());
                        }
                    }
                }
                go(i, named, elided)
            }
            PTy::Path(None, p) => {
                let mut lts = 0;
                // (every argument is counted, though one is not known: what follows it still has lifetimes)
                let mut args_ok = true;
                for sg in &p.segs {
                    for a in &sg.args {
                        match a {
                            crate::parse::GArg::Lifetime(l) if l == "'_" => {
                                *elided += 1;
                                lts += 1;
                            }
                            crate::parse::GArg::Lifetime(l) => {
                                lts += 1;
                                if !named.iter().any(|x| x == l) {
                                    named.push(l.clone());
                                }
                            }
                            crate::parse::GArg::Ty(t) => {
                                args_ok &= go(t, named, elided);
                            }
                            _ => {}
                        }
                    }
                }
                // (a type parameter (one letter) or a type known to have no lifetime parameter)
                let last = p.segs.last().map(|s| s.name.as_str()).unwrap_or("");
                args_ok && (lts > 0 || last.len() == 1 || NO_LTS.contains(&last))
            }
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => go(i, named, elided),
            PTy::Tuple(ts) => ts.iter().fold(true, |ok, t| go(t, named, elided) && ok),
            PTy::Never => true,
            // (a nested fn pointer or a trait object binds or hides its own)
            _ => false,
        }
    }
    let (mut named, mut elided, mut complete) = (vec![], 0, true);
    for p in ps {
        complete &= go(p, &mut named, &mut elided);
    }
    (named.len() + elided, complete)
}

/// The elided output of a fn type with these inputs has no lifetime to take: more than one is known (an input not
/// understood can only add more), or all are understood and there is not exactly one
fn elision_fails(ps: &[PTy]) -> bool {
    let (n, complete) = input_lifetimes(ps);
    n > 1 || (complete && n != 1)
}

fn fn_elision_err(t: &PTy) -> bool {
    let sig = |ps: &[PTy], r: Option<&PTy>| r.is_some_and(|r| lifetime_positions(r).0 > 0) && elision_fails(ps);
    match t {
        PTy::Fn(ps, r) => sig(ps, Some(r)) || ps.iter().any(fn_elision_err) || fn_elision_err(r),
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => fn_elision_err(i),
        PTy::Tuple(ts) => ts.iter().any(fn_elision_err),
        PTy::Path(q, p) => q.as_deref().is_some_and(fn_elision_err) || path_elision_err(p),
        PTy::Dyn(bs) | PTy::Impl(bs) => bounds_elision_err(bs),
        _ => false,
    }
}

fn path_elision_err(p: &Path) -> bool {
    p.segs.iter().any(|sg| {
        sg.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) if fn_elision_err(t)))
            || sg.fn_args.as_ref().is_some_and(|(ps, r)| {
                (r.as_deref().is_some_and(|r| lifetime_positions(r).0 > 0) && elision_fails(ps))
                    || ps.iter().any(fn_elision_err)
                    || r.as_deref().is_some_and(fn_elision_err)
            })
    })
}

fn bounds_elision_err(bs: &[Bound]) -> bool {
    bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if path_elision_err(p)))
}

fn generics_elision_err(g: &Generics) -> bool {
    g.params.iter().any(|p| matches!(p, GParam::Ty(_, bs, d) if bounds_elision_err(bs) || d.as_ref().is_some_and(fn_elision_err)))
        || g.preds.iter().any(|(t, bs)| fn_elision_err(t) || bounds_elision_err(bs))
}

fn item_elision_err(item: &Item) -> bool {
    let fields = |f: &Fields| match f {
        Fields::Unit => false,
        Fields::Tuple(v) => v.iter().any(fn_elision_err),
        Fields::Named(v) => v.iter().any(|x| fn_elision_err(&x.1)),
    };
    match &item.kind {
        ItemKind::Fn(fs, _) => {
            generics_elision_err(&fs.generics) || fs.params.iter().any(|p| fn_elision_err(&p.1)) || fs.ret.as_ref().is_some_and(fn_elision_err)
        }
        ItemKind::Struct(_, g, f) | ItemKind::Union(_, g, f) => generics_elision_err(g) || fields(f),
        ItemKind::Enum(_, g, vs) => generics_elision_err(g) || vs.iter().any(|v| fields(&v.1)),
        ItemKind::Type(_, g, bs, t) => generics_elision_err(g) || bounds_elision_err(bs) || t.as_ref().is_some_and(fn_elision_err),
        ItemKind::Const(_, Some(t), _) | ItemKind::Static(_, _, t, _) => fn_elision_err(t),
        _ => false,
    }
}

/// E0637: `'_` as a bound (`T: '_`, `'b: '_`), or a reference with no lifetime in a bound's arguments
/// (`where T: WithType<&u32>`; `Fn(&u32)` elides): a message (`\u{3}line\u{3}msg` at the reference's line)
fn anon_bound_err(item: &Item) -> Option<String> {
    let g = match &item.kind {
        ItemKind::Fn(fs, _) => &fs.generics,
        ItemKind::Struct(_, g, _) | ItemKind::Enum(_, g, _) | ItemKind::Union(_, g, _) | ItemKind::Impl(g, ..) | ItemKind::Trait(_, g, ..) | ItemKind::Type(_, g, _, _) => g,
        _ => return None,
    };
    if g.params.is_empty() && g.preds.is_empty() {
        return None;
    }
    let bs_err = |bs: &[Bound]| -> Option<String> {
        for b in bs {
            match b {
                Bound::Lifetime(l) if l == "'_" => return Some("`'_` cannot be used here (E0637)".to_string()),
                Bound::Trait(_, p) => {
                    for a in p.segs.iter().flat_map(|s| s.args.iter()) {
                        let (crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t)) = a else { continue };
                        if let PTy::Ref(None, _, i) = t {
                            let m = "`&` without an explicit lifetime name cannot be used here (E0637)";
                            return Some(match crate::wf::pos_of(i) {
                                Some(p) => format!("\u{3}{}\u{3}{m}", p.line),
                                None => m.to_string(),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        None
    };
    for p in &g.params {
        if let GParam::Ty(_, bs, _) = p {
            if let Some(e) = bs_err(bs) {
                return Some(e);
            }
        }
    }
    g.preds.iter().find_map(|(_, bs)| bs_err(bs))
}

/// E0597: the initializer of a const or a static, a block that ends in a borrow of one of its own `let` locals
/// (`const Z: &isize = { let p = 3; &p };`, also of a field or an element of it): the value lives for 'static, the
/// local only to the end of the block (`\u{3}line\u{3}msg`, at the local's line)
fn const_local_borrow_err(item: &Item) -> Option<String> {
    let (e, what, ty) = match &item.kind {
        ItemKind::Const(_, t, Some(e)) => (e, "constant", t.as_ref()),
        ItemKind::Static(_, _, t, e) => (e, "static", Some(t)),
        _ => return None,
    };
    // (the initializer's own block, or one in it whose value it uses: `mem::transmute({ .. { let s = [0; 10]; &s } })`)
    let (line, name, inited) = block_borrowing_own_local(e, 0)?;
    // (a local with no value (`let x; &x`): the borrow of what is not initialized, E0381 there)
    if !inited {
        return Some(format!("\u{3}{line}\u{3}used binding `{name}` isn't initialized (E0381)"));
    }
    // (of a raw pointer's type (`const FOO: *const u32 = { let x = 42; &x };`): no borrow check error, as a pointer
    // has no lifetime, but rustc's evaluation of the constant finds the pointer dangling, at the item)
    if matches!(ty, Some(PTy::Ptr(..))) {
        return Some(format!("\u{3}1\u{3}encountered dangling pointer in final value of {what}"));
    }
    Some(format!("\u{3}{line}\u{3}`{name}` does not live long enough (borrowed value must outlive the {what}) (E0597)"))
}

/// A block in e (not in a closure's body) whose tail borrows one of its own locals: that local's line and name, and
/// whether its `let` gives it a value
fn block_borrowing_own_local(e: &Expr, d: u32) -> Option<(u32, String, bool)> {
    if d > 32 {
        return None;
    }
    match e {
        Expr::Closure(..) => None,
        Expr::Block(_, _, b) | Expr::Const(b) => tail_borrows_own_local(b).or_else(|| {
            b.stmts.iter().find_map(|st| match st {
                Stmt::Expr(x, _) => block_borrowing_own_local(x, d + 1),
                // (a `let` of a raw pointer's type coerces its block's tail before the block ends: `let _: *const
                // u32 = { let x = 42; &x };` borrows x no longer than the block. Its statements are looked at)
                Stmt::Let(_, Some(PTy::Ptr(..)), Some(x), ..) => {
                    let mut x = x;
                    while let Expr::Paren(i) = x {
                        x = i;
                    }
                    match x {
                        Expr::Block(_, _, ib) | Expr::Const(ib) => {
                            let n = ib.stmts.len() - matches!(ib.stmts.last(), Some(Stmt::Expr(_, false))) as usize;
                            ib.stmts[..n].iter().find_map(|st| match st {
                                Stmt::Expr(y, _) => block_borrowing_own_local(y, d + 2),
                                Stmt::Let(_, _, Some(y), ..) => block_borrowing_own_local(y, d + 2),
                                _ => None,
                            })
                        }
                        x => block_borrowing_own_local(x, d + 1),
                    }
                }
                Stmt::Let(_, _, Some(x), ..) => block_borrowing_own_local(x, d + 1),
                _ => None,
            })
        }),
        _ => {
            let mut found = None;
            ast_closure::each_child_pub(e, &mut |c| {
                if found.is_none() {
                    found = block_borrowing_own_local(c, d + 1);
                }
            });
            found
        }
    }
}

/// The block's tail borrows one of its own locals (`{ let p = 3; &p }`, `&p.0` of a local tuple): its line and name,
/// and whether its `let` gives it a value
fn tail_borrows_own_local(b: &Block) -> Option<(u32, String, bool)> {
    let Some(Stmt::Expr(tail, false)) = b.stmts.last() else { return None };
    let mut x = tail;
    while let Expr::Paren(i) = x {
        x = i;
    }
    let Expr::Ref(_, inner) = x else { return None };
    let mut r = &**inner;
    let mut through = false;
    loop {
        match r {
            Expr::Field(b, _, _) | Expr::Index(b, _, _) => {
                through = true;
                r = b;
            }
            Expr::Paren(b) => r = b,
            _ => break,
        }
    }
    let Expr::Path(None, p) = r else { return None };
    let [seg] = p.segs.as_slice() else { return None };
    // (the local's `let`, the last one before the tail)
    let Some((ann, init)) = b.stmts.iter().rev().find_map(|st| match st {
        Stmt::Let(Pat::Ident(_, _, n, _), ann, init, ..) if *n == seg.name => Some((ann, init)),
        _ => None,
    }) else {
        return None;
    };
    // (a field or an element of the local is its own storage only where the local holds the aggregate, not a
    // reference the access derefs (`let p = &DATA; &p.0` borrows DATA): a written tuple or array type, or a value
    // built there (a literal, a tuple, an array, a struct). A written path may name an alias of a reference
    // (`type Alias = &'static (i32,);`), which the summary does not know: its value decides)
    if through {
        let mut i = init.as_ref();
        while let Some(Expr::Paren(x)) = i {
            i = Some(x);
        }
        let built = matches!(i, Some(Expr::Lit(..) | Expr::Tuple(_) | Expr::Array(_) | Expr::Repeat(..) | Expr::Struct(..)));
        let owned = match ann {
            Some(PTy::Tuple(_) | PTy::Array(..)) => true,
            Some(PTy::Ref(..) | PTy::Ptr(..)) => false,
            _ => built,
        };
        if !owned {
            return None;
        }
    }
    if p.pos.line == 0 {
        return None;
    }
    Some((p.pos.line, seg.name.clone(), init.is_some()))
}

/// The summary of one item (front.rs's summarize)
pub fn summarize(item: &Item, owner: Option<&Owner>) -> R<Summary> {
    if item_elision_err(item) {
        return err("missing lifetime specifier (E0106)");
    }
    let mut sm = Summary::default();
    // (`type Assoc<T = u32>;` in a trait or an impl: an associated type's parameters take no defaults)
    if let ItemKind::Type(_, g, ..) = &item.kind {
        if owner.is_some() && g.params.iter().any(|p| matches!(p, GParam::Ty(_, _, Some(_)))) {
            return err("defaults for generic parameters are not allowed here");
        }
    }
    // (the program's items: the prelude writes no lifetimes in its bounds, `Iterator<Item = &T>`; the driver
    // reports it)
    sm.adt_err = anon_bound_err(item).or_else(|| const_local_borrow_err(item));
    match &item.kind {
        ItemKind::Type(n, g, _, Some(t)) if owner.is_some() => summarize_assoc_type(owner, &mut sm, n, g, t)?,
        ItemKind::Fn(fs, Some(body)) if owner.is_some() => summarize_assoc_fn(owner, &mut sm, fs, body)?,
        // Type alias (`type Key = u128;`)
        ItemKind::Type(n, g, _, Some(t)) => summarize_type_alias(&mut sm, n, g, t)?,
        ItemKind::Trait(n, g, supers, _) => summarize_trait(&mut sm, n, g, supers)?,
        ItemKind::Impl(g, tr, self_ty, _) => summarize_impl(&mut sm, g, tr, self_ty)?,
        ItemKind::Fn(fs, Some(body)) => summarize_fn(item, &mut sm, fs, body)?,
        ItemKind::Use(_) => {}
        ItemKind::Const(n, Some(t), Some(_)) => summarize_const(item, owner, &mut sm, n, t)?,
        ItemKind::Static(m, n, t, _) => summarize_static(item, &mut sm, m, n, t)?,
        ItemKind::Extern(items) => summarize_extern(&mut sm, items)?,
        ItemKind::Enum(sn, g, vars) => summarize_enum(item, &mut sm, sn, g, vars)?,
        ItemKind::Struct(sn, g, fields) | ItemKind::Union(sn, g, fields) => summarize_struct(item, &mut sm, sn, g, fields)?,
        _ => return err("unsupported item"),
    }
    Ok(sm)
}

/// [`summarize`] of an associated type with a value in a trait or an impl (`type Item = u8;`): the owner's
/// generics, then a generic associated type's own
fn summarize_assoc_type(owner: Option<&Owner>, sm: &mut Summary, n: &String, g: &Generics, t: &PTy) -> R<()> {
    let ow = owner.unwrap();
    // (`type Pointer<T> = Rc<T>;`: a generic associated type's own parameters follow the impl's, after the
    // mark `#gat`)
    let gat: Vec<String> = g.params.iter().filter_map(|p| match p {
        GParam::Ty(n, ..) => Some(n.clone()),
        _ => None,
    }).collect();
    if let Some(r) = &ow.regions {
        let mut used = vec![];
        named_lifetimes(t, &mut used);
        if let Some(l) = used.iter().find(|l| r.free.contains(l)) {
            return Err(format!("\u{2}the lifetime parameter `{l}` is not constrained by the impl trait, self type, or predicates (E0207)"));
        }
        // (the value is well formed, as the header implies: `type Out = &'a T;` needs `T: 'a`)
        if !ow.is_trait {
            if let Some(e) = crate::wf::Known::of_owner(ow).check(t, false, 0) {
                return Err(crate::wf::at(e));
            }
        }
    }
    // (an associated type's value elides no lifetime: `type Output = &i32;` (E0106), `&'_ i32` (E0637); the
    // program's, not the prelude's, which writes no lifetimes: the driver reports it of the program's items)
    if !ow.is_trait && lifetime_positions(t).0 > 0 {
        sm.adt_err = Some(if format!("{t:?}").contains("\"'_\"") { "`'_` cannot be used here (E0637)" } else { "missing lifetime in associated type (E0106)" }.to_string());
    }
    let mut generics: Vec<String> = ow.generics.iter().map(|g| g.0.clone()).collect();
    if !gat.is_empty() {
        generics.push("#gat".to_string());
        generics.extend(gat);
    }
    // (a value that names a lifetime, the impl's or 'static: its codes against the impl's lifetimes)
    // (an impl that names no lifetime (`impl Foo for () { type Out = &'static u32; }`): none to instantiate)
    // (or an impl whose header requires `'d: 'a` of its lifetimes: a projection that selects it requires it of
    // the lifetimes its self type and trait write (nll/normalization-bounds-error.rs), whatever its value)
    let hdr_outlives = ow.regions.as_deref().is_some_and(|r| !r.outlives.is_empty());
    if !ow.is_trait && (crate::regions::names_lt(t) || hdr_outlives) {
        let (lts, self_slots) = ow.regions.as_deref().map(|r| (r.lts.clone(), r.self_slots.as_slice())).unwrap_or_default();
        {
            let tps: Vec<&str> = ow.generics.iter().map(|g| g.0.as_str()).collect();
            let mut s = crate::regions::Slots { lts, tparams: tps, self_slots, self_paths: &[], any: false, in_fn: 0, derived: None };
            let mut v = vec![];
            s.slots(t, false, &mut v);
            // (a value the codes do not follow (`<() as Base>::Out`) keeps the header's outlives all the same:
            // its codes have the mark R_UNKNOWN, so that no tree is made of them)
            if (s.any && !v.contains(&crate::types::R_UNKNOWN)) || hdr_outlives {
                sm.assoc_regions.push((sm.assocs.len() as u64, v));
            }
        }
    }
    sm.assocs.push(AssocSyn {
        name: n.clone(),
        self_ty: ow.self_ty.clone(),
        generics,
        value: tysyn(t, Some(&ow.self_ty))?,
    });
    Ok(())
}

/// [`summarize`] of a fn with a body in a trait or an impl: its signature with the owner's generics, and the closures in it
fn summarize_assoc_fn(owner: Option<&Owner>, sm: &mut Summary, fs: &PSig, body: &Block) -> R<()> {
    let ow = owner.unwrap();
    // (a method of `impl<T> Tr for &T` / `&mut T`: noted for Program, which asks the impl to define each default
    // method of the trait. Its methods are not called: where T is bounded by the trait itself, a call on `&X`
    // reaches X's impl through the deref, which is what a method that forwards to T's does. One that does
    // more is noted too: the driver reports it where the program calls a method of that name. (With another
    // bound, thiserror's `impl<T: Display> AsDisplay for &T`, X has no impl of its own to reach)
    if let (false, Some(trn), TySyn::Ref(_, inner)) = (ow.is_trait, ow.impl_trait.as_deref(), &ow.self_ty) {
        let trn = trn.split('<').next().unwrap_or(trn);
        if let TySyn::Named(param) = &**inner {
            if ow.generics.iter().any(|g| g.0 == *param) {
                sm.ref_impl_fns.push(fs.name.clone());
                let bounded = ow.generics.iter().any(|g| g.0 == *param && g.1.iter().any(|b| b.split('<').next().unwrap_or(b).trim() == trn));
                let mut_ref = matches!(&ow.self_ty, TySyn::Ref(true, _));
                if bounded && fs.recv.is_some() {
                    if !forwards_to_referent(fs, body, param, trn, mut_ref) {
                        sm.ref_impl_bad.push(fs.name.clone());
                    } else {
                        // (a `_` parameter it gives the path of its type: a forward if that names a unit struct)
                        for (p, t) in &fs.params {
                            if let (Pat::Wild, PTy::Path(None, tp)) = (p, t) {
                                if let Some(l) = tp.segs.last() {
                                    sm.ref_impl_unit_args.push((fs.name.clone(), l.name.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut sig = parse_sig(fs, false, false, Some(ow))?;
    // (E0106: a method with no receiver elides as a free fn; the program's, not the prelude's, which writes no
    // lifetimes: the driver reports it of the program's items)
    if fs.recv_ty.is_none() && !fs.name.starts_with("__") {
        if let Some(r) = fs.ret.as_ref().filter(|r| lifetime_positions(r).0 > 0) {
            // (the distinct input lifetimes: `x: &'a u8, y: &'a u8` has one; a parameter whose type may hold
            // lifetimes not written leaves none known to be missing)
            let ps: Vec<PTy> = fs.params.iter().map(|(_, t)| t.clone()).collect();
            let (n, complete) = input_lifetimes(&ps);
            if n > 1 || n == 0 && complete {
                sm.adt_err = Some(match crate::wf::pos_of(r) {
                    Some(p) => format!("\u{3}{}\u{3}missing lifetime specifier (E0106)", p.line),
                    None => "missing lifetime specifier (E0106)".to_string(),
                });
            }
        }
    }
    let name = match ow.impl_trait.as_deref() {
        Some(t) if t == "Display" || t == "Debug" => format!("{}::<{t}>::{}", ow.base, fs.name),
        Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => format!("{}::<{t}>::{}", ow.base, fs.name),
        _ if matches!(ow.self_ty, TySyn::Ref(..)) => {
            let (mut pre, mut t) = (String::new(), &ow.self_ty);
            while let TySyn::Ref(_, i) = t {
                pre.push_str("__ref_");
                t = i;
            }
            format!("{}::{pre}{}", ow.base, fs.name)
        }
        _ => format!("{}::{}", ow.base, fs.name),
    };
    lift_closures(fs, body, &name, &mut sig, Some(&ow.self_ty), sm)?;
    sm.fns.push((name, sig));
    Ok(())
}

/// [`summarize`] of a type alias (`type Key = u128;`)
fn summarize_type_alias(sm: &mut Summary, n: &String, g: &Generics, t: &PTy) -> R<()> {
    let mut gs = Vec::new();
    collect_generics(g, &mut gs)?;
    let consts: Vec<&str> = g.params.iter().filter_map(|p| match p {
        GParam::Const(n, ..) => Some(n.as_str()),
        _ => None,
    }).collect();
    if !consts.is_empty() && const_param_in_op(t, &consts) {
        return err("generic parameters may not be used in const operations");
    }
    let names: Vec<String> = gs.into_iter().map(|x| x.0).collect();
    // (the defaults of its parameters, `type Result<T, E = Error>`, in adt_defaults, which an alias's summary does not
    // use otherwise: a use may leave them out, `Result<()>`)
    sm.adt_defaults = adt_defaults(g, &names)?;
    sm.aliases.push((n.clone(), names, tysyn(t, None)?));
    let own: Vec<&str> = g.params.iter().filter_map(|p| if let GParam::Lifetime(l) = p { Some(l.as_str()) } else { None }).collect();
    let mut named = vec![];
    crate::regions::lifetimes_in(t, &mut named);
    if named.iter().any(|l| l != "'_" && !own.contains(&l.as_str())) {
        sm.alias_fixed_lts.push(n.clone());
    }
    Ok(())
}

/// [`summarize`] of a trait: its generics, supertraits, associated types and consts, and its methods' signatures
fn summarize_trait(sm: &mut Summary, n: &String, g: &Generics, supers: &Vec<Bound>) -> R<()> {
    // (E0261: the lifetimes its bounds and where clause name are its own; `'_`, `'static` and a binder's
    // (`for<'a> T: 'a`, parsed as 'static) excepted)
    if !g.preds.is_empty() || g.params.iter().any(|p| matches!(p, GParam::Ty(_, bs, _) if !bs.is_empty())) {
        let declared: Vec<&str> = g.params.iter().filter_map(|p| if let GParam::Lifetime(l) = p { Some(l.as_str()) } else { None }).collect();
        let mut used = vec![];
        bound_lifetimes(g, &mut used);
        for b in supers {
            if let Bound::Lifetime(l) = b {
                used.push(l.clone());
            }
        }
        if let Some(l) = undeclared_lifetime(&used, &declared) {
            let m = format!("use of undeclared lifetime name `{l}` (E0261)");
            // (at the where clause's predicate that names it)
            let names = |t: &PTy| {
                let mut v = vec![];
                named_lifetimes_deep(t, &mut v, 0);
                v.contains(&l)
            };
            let at = g.preds.iter().find(|(t, bs)| bs.iter().any(|b| matches!(b, Bound::Lifetime(x) if *x == l)) || names(t)).and_then(|(t, _)| crate::wf::pos_of(t));
            return Err(match at {
                Some(p) => format!("{}:{}: {m}", p.line, p.col + 1),
                None => m,
            });
        }
    }
    sm.hdr_regions = crate::regions::hdr_regions(g, None);
    // (`trait Wedding<'t>: 't`: what Self must outlive)
    let mut self_outlives = vec![];
    for b in supers {
        if let Bound::Lifetime(l) = b {
            if l == "'static" {
                self_outlives.push("static".to_string());
            } else if let Some(i) = g.params.iter().filter_map(|p| if let GParam::Lifetime(l) = p { Some(l) } else { None }).position(|x| x == l) {
                self_outlives.push(i.to_string());
            }
        }
    }
    let mut sup = vec![];
    // (`trait Num: Copy`: the marker supertraits a bound of the trait gives, Copy and Clone; `where Self: Copy`
    // is the same)
    let mut msup = vec![];
    for b in supers.iter().chain(g.preds.iter().filter(|(t, _)| matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "Self")).flat_map(|(_, bs)| bs)) {
        if let Bound::Trait(false, p) = b {
            let from_where = !supers.iter().any(|x| std::ptr::eq(x, b));
            if matches!(p.segs.last().map(|s| s.name.as_str()), Some("Copy" | "Clone")) && from_where {
                msup.push(p.segs.last().unwrap().name.clone());
                continue;
            }
            // (a supertrait with an equation, `trait View<U>: Deref<Target = U>` or `where Self: Deref<Target =
            // U>`: a mark, so that a parameter bounded by the trait is not taken as free of equations (acg
            // placeholder_mismatch))
            if p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, crate::parse::GArg::Bind(..)))) && !sup.iter().any(|x| x == "#Eq") {
                sup.push("#Eq".to_string());
            }
            // (the where clause's other supertraits are left as they were)
            if from_where {
                continue;
            }
            if let Some(s) = p.segs.last() {
                if !matches!(s.name.as_str(), "Sized" | "Send" | "Sync" | "Copy" | "Clone" | "Unpin" | "Any" | "Debug" | "Display") {
                    sup.push(s.name.clone());
                }
                if matches!(s.name.as_str(), "Copy" | "Clone") {
                    msup.push(s.name.clone());
                }
                // (`trait Dup: Clone`: a marker no trait has the name of, so that a `T: Dup` is not taken
                // for a type that is surely not Clone (Program::surely_not_clone))
                if matches!(s.name.as_str(), "Copy" | "Clone") && !sup.iter().any(|x| x == "#Clone") {
                    sup.push("#Clone".to_string());
                }
                // (`trait T: PartialEq`: its `Rhs` is Self by default, so T is not dyn compatible; a marker
                // no trait has the name of)
                if matches!(s.name.as_str(), "PartialEq" | "PartialOrd") && s.args.is_empty() {
                    sup.push("#SelfRhs".to_string());
                }
            }
        }
    }
    // (`trait Foo<X = Box<dyn Foo>>`: the default needs the trait itself)
    for p in &g.params {
        if let GParam::Ty(_, _, Some(d)) = p {
            let mut names = vec![];
            ty_names(d, &mut names);
            if names.iter().any(|x| x == n) {
                return Err(format!("cycle detected when computing the default of a type parameter of `{n}`"));
            }
        }
    }
    collect_generics(g, &mut sm.trait_generics)?;
    if !sup.is_empty() {
        sm.trait_generics.push(("#supers".to_string(), sup));
    }
    if !msup.is_empty() {
        sm.trait_generics.push(("#marker_supers".to_string(), msup));
    }
    if !self_outlives.is_empty() {
        sm.trait_generics.push((crate::types::SELF_OUTLIVES.to_string(), self_outlives));
    }
    sm.trait_decl = Some(n.clone());
    Ok(())
}

/// [`summarize`] of an impl: inherent or of a trait, with the checks its header needs (E0207, coherence's inputs)
fn summarize_impl(sm: &mut Summary, g: &Generics, tr: &Option<(bool, Path)>, self_ty: &PTy) -> R<()> {
    sm.hdr_regions = crate::regions::hdr_regions(g, Some(self_ty));
    // (the lifetimes the trait path writes as its arguments, as codes of the header's)
    if let (Some(r), Some((_, p))) = (sm.hdr_regions.as_mut(), tr.as_ref()) {
        let code = |l: &str| if l == "'static" { crate::types::R_STATIC } else { r.lts.iter().position(|x| x == l).map_or(crate::types::R_UNKNOWN, |i| i as u8) };
        let lts: Vec<u8> = p.segs.last().map(|s| s.args.iter().filter_map(|a| if let crate::parse::GArg::Lifetime(l) = a { Some(code(l)) } else { None }).collect()).unwrap_or_default();
        r.trait_lts = lts;
    }
    sm.impl_trait_lts = tr.as_ref().is_some_and(|(_, p)| crate::regions::path_names_lt(p));
    // (E0261: the header names only the lifetimes it declares; E0207: a lifetime parameter an associated type
    // names must be constrained by the self type or the trait)
    if crate::regions::generics_name_lt(g) || crate::regions::names_lt(self_ty) || tr.as_ref().is_some_and(|(_, p)| crate::regions::path_names_lt(p)) {
        let declared: Vec<&str> = g.params.iter().filter_map(|p| if let GParam::Lifetime(l) = p { Some(l.as_str()) } else { None }).collect();
        let mut used = vec![];
        named_lifetimes(self_ty, &mut used);
        if let Some((_, tp)) = tr {
            named_lifetimes(&PTy::Path(None, tp.clone()), &mut used);
        }
        // (constrained: written anywhere in the self type or the trait (`dyn Tr + 'a` too), or in the value an
        // associated type is bound to in a bound (`T: Tr<Ty = &'x ()>`, inline or in a where clause). Only
        // restricting it (`S: 'a`) does not constrain it)
        let constrained: Vec<String> = {
            let mut binds = vec![];
            let mut of_bounds = |bs: &[Bound]| {
                for b in bs {
                    if let Bound::Trait(_, p) = b {
                        for sg in &p.segs {
                            for a in &sg.args {
                                if let crate::parse::GArg::Bind(_, t) = a {
                                    binds.push(t.clone());
                                }
                            }
                        }
                    }
                }
            };
            for p in &g.params {
                if let GParam::Ty(_, bs, _) = p {
                    of_bounds(bs);
                }
            }
            for (_, bs) in &g.preds {
                of_bounds(bs);
            }
            let text = format!("{self_ty:?} {tr:?} {binds:?}");
            declared.iter().filter(|l| text.contains(&format!("\"{l}\""))).map(|l| l.to_string()).collect()
        };
        bound_lifetimes(g, &mut used);
        if let Some(l) = undeclared_lifetime(&used, &declared) {
            return Err(format!("use of undeclared lifetime name `{l}` (E0261)"));
        }
        if let Some(r) = &mut sm.hdr_regions {
            r.free = declared.iter().filter(|l| !constrained.iter().any(|x| x == *l)).map(|l| l.to_string()).collect();
            // (what the header's types imply, for its members' well-formedness; its declared bounds are
            // the owner's own)
            let mut k = crate::wf::Known::default();
            k.tparams = g.params.iter().filter_map(|p| if let GParam::Ty(n, _, _) = p { Some(n.clone()) } else { None }).collect();
            k.implied(self_ty, 0);
            if let Some((_, tp)) = tr {
                for s in &tp.segs {
                    for a in &s.args {
                        if let crate::parse::GArg::Ty(t) = a {
                            k.implied(t, 0);
                        }
                    }
                }
            }
            r.wf_pairs = k.pairs;
            r.wf_vague = k.vague;
        }
    }
    if let Some((neg, tp)) = tr {
        if *neg {
            return err("negative impls are not supported");
        }
        let last = tp.segs.last().ok_or("unsupported trait path")?;
        sm.impl_trait = Some(last.name.clone());
        // (the trait's arguments `impl From<u8> for S`: Program tells two impls of the trait apart with them)
        let args: Option<Vec<TySyn>> = last
            .args
            .iter()
            .filter(|a| matches!(a, crate::parse::GArg::Ty(_) | crate::parse::GArg::Const(_)))
            .map(|a| match a {
                crate::parse::GArg::Ty(t) => tysyn(t, None).ok(),
                crate::parse::GArg::Const(e) => crate::ast_types::const_arg_syn(e).ok(),
                _ => None,
            })
            .collect();
        sm.impl_trait_args = args.unwrap_or_default();
        // `impl PartialEq<[U; N]> for Vec<T>`: a comparison whose right-hand type differs from the self type keeps the type arguments in the trait name
        if (last.name == "PartialEq" || last.name == "PartialOrd") && !last.args.is_empty() {
            let rhs = compact(&last.text[last.name.len()..]);
            let selfs = ty_compact(self_ty);
            if rhs != format!("<{selfs}>") && rhs != "<Self>" {
                sm.impl_trait = Some(format!("{}{rhs}", last.name));
            }
        }
    }
    collect_generics(g, &mut sm.impl_generics)?;
    // (E0207: a type parameter must appear in the self type or the trait; a where clause counts too)
    {
        let mut names = vec![];
        // (a projection `T::Assoc` in the self type does not constrain T)
        let params: Vec<&str> = g.params.iter().filter_map(|p| if let GParam::Ty(n, _, _) = p { Some(n.as_str()) } else { None }).collect();
        let mut st = self_ty.clone();
        strip_projections(&mut st, &params);
        ty_names(&st, &mut names);
        if let Some((_, tp)) = tr {
            ty_names(&PTy::Path(None, tp.clone()), &mut names);
        }
        for (t, bs) in &g.preds {
            ty_names(t, &mut names);
            for b in bs {
                if let Bound::Trait(_, p) = b {
                    ty_names(&PTy::Path(None, p.clone()), &mut names);
                }
            }
        }
        for p in &g.params {
            if let GParam::Ty(_, bs, _) = p {
                for b in bs {
                    if let Bound::Trait(_, p) = b {
                        ty_names(&PTy::Path(None, p.clone()), &mut names);
                    }
                }
            }
        }
        for p in &g.params {
            if let GParam::Ty(n, _, _) = p {
                if !names.iter().any(|x| x == n) {
                    return Err(format!("the type parameter `{n}` is not constrained by the impl trait, self type, or predicates"));
                }
            }
        }
    }
    // `impl Add for &M`: held as M's method `__ref_add` (self is &M)
    // (`impl Tr for &&str`: every reference off, the methods `str::__ref___ref_m`, one `__ref_` a level)
    let mut self_path = self_ty;
    while let (PTy::Ref(_, _, i), true) = (self_path, tr.is_some()) {
        self_path = &**i;
    }
    if let (PTy::Tuple(v), true) = (self_path, tr.is_some()) {
        sm.impl_self = Some(format!("#tuple{}", v.len()));
        sm.impl_self_ty = Some(tysyn(self_ty, None)?);
        return Ok(());
    }
    // (`impl Tr for fn(A) -> B`: the methods of `__fnptr`)
    if matches!(self_path, PTy::Fn(..) | PTy::Ptr(..) | PTy::Never) && tr.is_some() {
        sm.impl_self = Some(match self_path {
            PTy::Fn(..) => "__fnptr",
            PTy::Ptr(..) => "__ptr",
            _ => "__never",
        }.to_string());
        sm.impl_self_ty = tysyn(self_ty, None).ok();
        return Ok(());
    }
    if matches!(self_path, PTy::Array(..) | PTy::Slice(_)) && tr.is_some() {
        sm.impl_self = Some(if matches!(self_path, PTy::Array(..)) { "__array" } else { "__slice" }.to_string());
        sm.impl_self_ty = Some(tysyn(self_ty, None)?);
        return Ok(());
    }
    // (`impl dyn Tr + 'a { .. }` / `impl X for dyn Tr`: the methods of `__dyn`)
    if let PTy::Dyn(bs) = self_path {
        let principal = bs.iter().find_map(|b| match b {
            Bound::Trait(_, p) => p.segs.last().map(|s| s.name.clone()),
            _ => None,
        });
        let mut names = vec![];
        ty_names(self_path, &mut names);
        if names.iter().any(|n| n == "Self") {
            return err("`Self` is not valid in the self type of an impl block");
        }
        if let (Some(t), Some(p)) = (sm.impl_trait.as_deref(), principal.as_deref()) {
            if t == p {
                return Err(format!("the object type `dyn {p}` automatically implements the trait `{t}` (E0371)"));
            }
            if t == "Drop" {
                return err("the `Drop` trait may only be implemented for local structs, enums, and unions (E0120)");
            }
            if matches!(t, "Send" | "Sync" | "Unpin") {
                return Err(format!("cross-crate traits with a default impl, like `{t}`, can only be implemented for a struct/enum type, not `dyn {p}` (E0321)"));
            }
        }
        sm.impl_self = Some(format!("__dyn_{}", principal.unwrap_or_default()));
        sm.impl_self_ty = Some(tysyn(self_ty, None)?);
        return Ok(());
    }
    let PTy::Path(None, tp) = self_path else {
        return err("unsupported impl self type");
    };
    if tp.segs.len() != 1 {
        // (`impl foo::Point` / `impl Tr for std::result::Result<O, E>`: the type the path names, as in a
        // signature)
        let (TySyn::Named(n) | TySyn::App(n, _)) = tysyn(self_path, None)? else {
            return err("unsupported impl self type");
        };
        sm.impl_self = Some(n);
        sm.impl_self_ty = Some(tysyn(self_ty, None)?);
        sm.impl_self_path = true;
        return Ok(());
    }
    sm.impl_self = Some(tp.segs[0].name.clone());
    sm.impl_self_ty = Some(tysyn(self_ty, None)?);
    Ok(())
}

/// [`summarize`] of a free fn with a body: its signature, and the closures in it
fn summarize_fn(item: &Item, sm: &mut Summary, fs: &PSig, body: &Block) -> R<()> {
    let name = fs.name.clone();
    if !item.init_text.is_empty() && fs.generics.params.is_empty() {
        let ps: Option<Vec<String>> = fs.params.iter().map(|(p, _)| match p {
            Pat::Ident(false, _, n, None) => Some(n.clone()),
            _ => None,
        }).collect();
        if let Some(ps) = ps {
            // (`u8`, an alias, or a primitive by its path: `core::primitive::u8`)
            let tn = |t: &crate::parse::Ty| match t {
                crate::parse::Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => p.segs[0].name.clone(),
                crate::parse::Ty::Path(None, p)
                    if p.segs.len() == 3
                        && matches!(p.segs[0].name.as_str(), "core" | "std")
                        && p.segs[1].name == "primitive"
                        && p.segs.iter().all(|s| s.args.is_empty()) =>
                {
                    p.segs[2].name.clone()
                }
                _ => String::new(),
            };
            let pts = fs.params.iter().map(|(_, t)| tn(t)).collect();
            sm.const_fn_tys.push((name.clone(), pts, fs.ret.as_ref().map_or(String::new(), tn)));
            sm.const_fns.push((name.clone(), ps, item.init_text.clone()));
        }
    }
    let export = has_attr(&item.attrs, "no_mangle");
    // (`zustc --test`'s entry point, crate::test_harness: a `main` too. The program keeps it as the one, and
    //  the crate's own `main` as an ordinary function: sema::program)
    let entry = has_attr(&item.attrs, "zust_test_entry");
    // (a `main` that cannot be the program's (generic, with parameters, or another result) is summed up as an
    //  ordinary function: under `--test` the crate's `main` is one. sema::program reports it when it would be
    //  the entry. The summary does not depend on the rest of the crate, as it is cached by the item's text)
    let named_main = name == "main" && !export && !entry;
    let generic = named_main && !fs.generics.params.is_empty();
    let mut sig = parse_sig(fs, false, (named_main && !generic) || entry, None)?;
    let result_ret = matches!(&sig.ret, TySyn::App(n, _) if n == "Result");
    let bad_main = named_main && (generic || !sig.params.is_empty() || (sig.ret != TySyn::Prim(Ty::Unit) && !result_ret));
    // (an `async fn main` cannot be the entry either (E0752): its symbol tells it from the others)
    let async_main = named_main && fs.quals.iter().any(|q| q == "async");
    if bad_main || async_main {
        sig.is_main = false;
        sig.symbol = if async_main { "_xmain.async".to_string() } else { format!("_x{name}") };
    }
    if entry && (!sig.params.is_empty() || sig.ret != TySyn::Prim(Ty::Unit)) {
        return err("`main` must be `fn main()`");
    }
    if export {
        sig.export = true;
        sig.symbol = name.clone();
    }
    if has_attr(&item.attrs, "panic_handler") {
        sig.panic_handler = true;
        if sig.ret != TySyn::Prim(Ty::Never) {
            return err("`#[panic_handler]` function must return `!`");
        }
    }
    lift_closures(fs, body, &name, &mut sig, None, sm)?;
    sm.fns.push((name, sig));
    Ok(())
}

/// [`summarize`] of a const with a type and a value
fn summarize_const(item: &Item, owner: Option<&Owner>, sm: &mut Summary, n: &String, t: &PTy) -> R<()> {
    if let Some(u) = unsized_value(t) {
        return Err(format!("the size for values of type `{u}` cannot be known at compilation time (E0277)"));
    }
    let name = match owner {
        Some(ow) => format!("{}::{}", ow.base, n),
        None => n.clone(),
    };
    // (an impl's whose type names the impl's lifetimes: its codes against them)
    if let Some(r) = owner.and_then(|o| o.regions.as_deref()).filter(|r| !r.lts.is_empty() && crate::regions::names_lt(t)) {
        let tps: Vec<&str> = owner.map(|o| o.generics.iter().map(|g| g.0.as_str()).collect()).unwrap_or_default();
        let mut s = crate::regions::Slots { lts: r.lts.clone(), tparams: tps, self_slots: &r.self_slots, self_paths: &[], any: false, in_fn: 0, derived: None };
        let mut v = vec![];
        s.slots(t, false, &mut v);
        if s.any && !v.contains(&crate::types::R_UNKNOWN) && v.iter().any(|&c| (c as usize) < r.lts.len()) {
            sm.const_regions.push((name.clone(), v));
        }
    }
    sm.consts.push((name, tysyn(t, owner.map(|o| &o.self_ty))?, item.init_text.clone()));
    Ok(())
}

/// [`summarize`] of a static
fn summarize_static(item: &Item, sm: &mut Summary, m: &bool, n: &String, t: &PTy) -> R<()> {
    let mut used = vec![];
    named_lifetimes(t, &mut used);
    if let Some(l) = undeclared_lifetime(&used, &[]) {
        return Err(format!("use of undeclared lifetime name `{l}` (E0261)"));
    }
    if let Some(u) = unsized_value(t) {
        return Err(format!("the size for values of type `{u}` cannot be known at compilation time (E0277)"));
    }
    sm.statics.push((n.clone(), tysyn(t, None)?, item.init_text.clone(), *m));
    Ok(())
}

/// [`summarize`] of an `extern` block's fns and statics
fn summarize_extern(sm: &mut Summary, items: &Vec<Item>) -> R<()> {
    // (E0428: two foreign items of one name in the value namespace, at the second's start, its `pub` included)
    let mut names: Vec<&str> = vec![];
    for fi in items {
        let n = match &fi.kind {
            ItemKind::Fn(fs, _) => fs.name.as_str(),
            ItemKind::Static(_, n, ..) => n.as_str(),
            _ => continue,
        };
        if names.contains(&n) {
            return Err(format!("{}:1: the name `{n}` is defined multiple times (E0428)", fi.line));
        }
        names.push(n);
    }
    for fi in items {
        // (`static X: T;`: the symbol X (or its link_name) of what the program links with)
        if let ItemKind::Static(m, n, t, _) = &fi.kind {
            let sym = link_name(&fi.attrs).unwrap_or_else(|| n.clone());
            let at = |m: &str| format!("{}:1: {m}", fi.line);
            if sym.starts_with("__enzyme_") {
                return Err(at("linking to Enzyme intrinsics is experimental"));
            }
            if fi.attrs.iter().any(|a| compact(a).starts_with("#[linkage")) {
                return Err(at("the `linkage` attribute is experimental and not portable"));
            }
            if matches!(t, crate::parse::Ty::Infer) {
                return Err(at("the placeholder `_` is not allowed within types on item signatures for static items (E0121)"));
            }
            if let Some(u) = unsized_value(t) {
                return Err(at(&format!("the size for values of type `{u}` cannot be known at compilation time (E0277)")));
            }
            if matches!(t, crate::parse::Ty::Never) {
                return Err(at("static of uninhabited type"));
            }
            sm.statics.push((n.clone(), tysyn(t, None)?, format!("{}{} {sym}", crate::parse::EXTERN_STATIC_SYM, fi.line), *m));
            continue;
        }
        let ItemKind::Fn(fs, None) = &fi.kind else {
            // (a fn with a body: what a foreign fn may not have)
            if matches!(fi.kind, ItemKind::Fn(_, Some(_))) {
                return Err(format!("{}:1: incorrect function inside `extern` block", fi.line));
            }
            return err("only fns and statics are supported in extern blocks");
        };
        if link_name(&fi.attrs).unwrap_or_else(|| fs.name.clone()).starts_with("__enzyme_") {
            return Err(format!("{}:1: linking to Enzyme intrinsics is experimental", fi.line));
        }
        if fi.attrs.iter().any(|a| compact(a).starts_with("#[linkage")) {
            return Err(format!("{}:1: the `linkage` attribute is experimental and not portable", fi.line));
        }
        // (an error at the fn's line in the block)
        let mut sig = parse_sig(fs, true, false, None).map_err(|e| if e.starts_with(|c: char| c.is_ascii_digit()) { e } else { format!("{}:1: {e}", fi.line) })?;
        if let Some(l) = link_name(&fi.attrs) {
            sig.symbol = l;
        }
        sm.fns.push((fs.name.clone(), sig));
    }
    Ok(())
}

/// [`summarize`] of an enum: its variants, layout attributes, generics and derives
fn summarize_enum(item: &Item, sm: &mut Summary, sn: &String, g: &Generics, vars: &Vec<(String, Fields, Option<Expr>)>) -> R<()> {
    {
        let mut tys: Vec<&PTy> = vec![];
        for (_, f, _) in vars {
            match f {
                Fields::Named(v) => tys.extend(v.iter().map(|x| &x.1)),
                Fields::Tuple(v) => tys.extend(v.iter()),
                Fields::Unit => {}
            }
        }
        sm.adt_err = adt_checks(&item.attrs, g, &tys, false, vars.len()).err().or_else(|| static_fields(g, &tys));
        if let Some(e) = sm.adt_err.clone().filter(|e| e.ends_with("(E0106)")) {
            sm.adt_err = Some(format!("\u{1}&\u{1}{e}"));
        }
        // (an undeclared lifetime: at the line it is first written on)
        if let Some(e) = sm.adt_err.as_ref().filter(|e| e.ends_with("(E0261)")) {
            if let Some(l) = e.split('`').nth(1) {
                sm.adt_err = Some(format!("\u{1}{l}\u{1}{e}"));
            }
        }
        if let Some(r) = int_repr(&item.attrs) {
            sm.reprs.push((sn.clone(), r));
        }
        for r in layout_reprs(&item.attrs)? {
            sm.reprs.push((sn.clone(), r));
        }
    }
    sm.adt_generics = adt_params(g)?;
    sm.adt_defaults = adt_defaults(g, &sm.adt_generics)?;
    sm.adt_bounds = adt_bounds(g);
    sm.adt_unsized = adt_unsized(g);
    {
        let tys: Vec<&PTy> = vars.iter().flat_map(|(_, f, _)| match f {
            Fields::Unit => vec![],
            Fields::Tuple(v) => v.iter().collect(),
            Fields::Named(v) => v.iter().map(|x| &x.1).collect(),
        }).collect();
        sm.adt_regions = crate::regions::adt_regions(g, &tys);
    }
    let selft = self_of(sn, &sm.adt_generics);
    let mut vs = Vec::new();
    for (vn, fields, d) in vars {
        let (kind, fs) = match fields {
            Fields::Unit => (VariantKind::Unit, Vec::new()),
            Fields::Tuple(v) => {
                let mut fs = Vec::new();
                for (i, t) in v.iter().enumerate() {
                    fs.push((i.to_string(), tysyn(t, Some(&selft))?));
                }
                (VariantKind::Tuple, fs)
            }
            Fields::Named(v) => {
                let mut fs = Vec::new();
                for (n, t) in v {
                    fs.push((n.clone(), tysyn(t, Some(&selft))?));
                }
                (VariantKind::Struct, fs)
            }
        };
        let mut discr_src = String::new();
        let discr = match d {
            None => None,
            // (a literal here; an expression with an operator or a cast is Program's typed evaluator's:
            // `A = (300u32 as u8) as u16` is 44, which an untyped fold would read as 300)
            Some(e) => match if lit_discr(e) { const_int(e) } else { err("not a literal") } {
                Ok(v) => Some(v),
                // (`Bar = QUUX` / `Y = Foo::X as isize - 3`: Program evaluates it with the constants)
                Err(er) => {
                    // (a block's tail, `unsafe` or not: `A = unsafe { 1 + 2 }`)
                    // (through parentheses too: `A = (unsafe { 1 + 2 })`)
                    let mut x = e;
                    loop {
                        x = match x {
                            Expr::Paren(i) => i,
                            Expr::Block(None, _, b) => match &b.stmts[..] {
                                [crate::parse::Stmt::Expr(t, false)] => t,
                                _ => break,
                            },
                            _ => break,
                        };
                    }
                    discr_src = crate::ast_types::const_expr_text(x).ok_or(er)?;
                    Some(0)
                }
            },
        };
        vs.push(VariantSyn { name: vn.clone(), kind: kind, fields: fs, discr: discr, discr_src });
    }
    let d = derive_names(&item.attrs);
    if !d.is_empty() {
        sm.derives.push((sn.clone(), d));
    }
    sm.enums.push((sn.clone(), vs));
    Ok(())
}

/// [`summarize`] of a struct or a union: its fields, layout attributes, generics and derives
fn summarize_struct(item: &Item, sm: &mut Summary, sn: &String, g: &Generics, fields: &Fields) -> R<()> {
    let union = matches!(item.kind, ItemKind::Union(..));
    {
        let tys: Vec<&PTy> = match fields {
            Fields::Named(v) => v.iter().map(|x| &x.1).collect(),
            Fields::Tuple(v) => v.iter().collect(),
            Fields::Unit => vec![],
        };
        if union && matches!(fields, Fields::Tuple(_)) {
            return err("tuple unions are not allowed");
        }
        // (PhantomData itself is the one type whose parameter needs no use)
        sm.adt_err = adt_checks(&item.attrs, g, &tys, union, tys.len()).err().or_else(|| static_fields(g, &tys));
        // (an unsized element: at its field's line, which the driver finds by the field's name)
        // (an undeclared lifetime: at the line it is first written on)
        if let Some(e) = sm.adt_err.as_ref().filter(|e| e.ends_with("(E0261)")) {
            if let Some(l) = e.split('`').nth(1) {
                sm.adt_err = Some(format!("\u{1}{l}\u{1}{e}"));
            }
        }
        // (a missing lifetime: at the field it is missing in (named), or at the first `&`)
        if let Some(e) = sm.adt_err.clone().filter(|e| e.ends_with("(E0106)")) {
            let at = match fields {
                Fields::Named(v) => v.iter().find(|x| lifetime_positions(&x.1).0 > 0 || anon_dyn(&x.1)).map(|x| x.0.clone()),
                _ => None,
            };
            sm.adt_err = Some(format!("\u{1}{}\u{1}{e}", at.unwrap_or_else(|| "&".to_string())));
        }
        if let (Some(e), Fields::Named(v)) = (&sm.adt_err, fields) {
            if e.ends_with("(E0277)") {
                if let Some((n, _)) = v.iter().find(|x| unsized_elem(&x.1).is_some()) {
                    sm.adt_err = Some(format!("\u{1}{n}\u{1}{e}"));
                }
            }
        }
    }
    if union {
        sm.unions.push(sn.clone());
    }
    for r in layout_reprs(&item.attrs)? {
        sm.reprs.push((sn.clone(), r));
    }
    sm.adt_generics = adt_params(g)?;
    sm.adt_defaults = adt_defaults(g, &sm.adt_generics)?;
    sm.adt_bounds = adt_bounds(g);
    sm.adt_unsized = adt_unsized(g);
    {
        let tys: Vec<&PTy> = match fields {
            Fields::Named(v) => v.iter().map(|x| &x.1).collect(),
            Fields::Tuple(v) => v.iter().collect(),
            Fields::Unit => vec![],
        };
        sm.adt_regions = crate::regions::adt_regions(g, &tys);
    }
    let selft = self_of(sn, &sm.adt_generics);
    let mut fs = Vec::new();
    match fields {
        Fields::Named(v) => {
            for (n, t) in v {
                fs.push((n.clone(), tysyn(t, Some(&selft))?));
            }
        }
        Fields::Tuple(v) => {
            for (i, t) in v.iter().enumerate() {
                fs.push((i.to_string(), tysyn(t, Some(&selft))?));
            }
        }
        Fields::Unit => {}
    }
    let d = derive_names(&item.attrs);
    if !d.is_empty() {
        sm.derives.push((sn.clone(), d));
    }
    sm.structs.push((sn.clone(), fs));
    Ok(())
}

/// Parses and summarizes one lexer item (front.rs's parse_and_summarize)
pub fn parse_and_summarize(src: &str, it: &SrcItem, owner: Option<&Owner>) -> R<Summary> {
    let text = &src[it.start..it.end];
    if let LexKind::InnerAttr = it.kind {
        return Ok(Summary::default());
    }
    let text = match it.kind {
        LexKind::ImplHeader | LexKind::TraitHeader => format!("{text}}}"),
        _ => text.to_string(),
    };
    let f = parse_file(&text)?;
    // (`for<`, also with spaces or a line break before the `<`)
    HRTB_TEXT.with(|c| c.set(text.match_indices("for").any(|(i, _)| text[i + 3..].trim_start().starts_with('<'))));
    let Some(item) = f.items.first() else { return Ok(Summary::default()) };
    // (E0496 / E0403: a generic associated type's own parameters named as its trait's or impl's: `type Bar<'a>` in
    // `trait Shadow<'a>`)
    if let (ItemKind::Type(_, g, ..), Some(ow)) = (&item.kind, owner) {
        let lts: &[String] = ow.regions.as_ref().map(|r| r.lts.as_slice()).unwrap_or(&[]);
        for p in &g.params {
            match p {
                GParam::Lifetime(l) if lts.iter().any(|x| x == l) => {
                    return Err(format!("lifetime name `{l}` shadows a lifetime name that is already in scope (E0496)"));
                }
                GParam::Ty(n, ..) if n != "Self" && ow.generics.iter().any(|x| x.0 == *n) => {
                    return Err(format!("the name `{n}` is already used for a generic parameter in this item's generic parameters (E0403)"));
                }
                _ => {}
            }
        }
    }
    if let LexKind::TraitMember(_) = it.kind {
        let ow = owner.ok_or("trait member without trait")?;
        match &item.kind {
            // `type Item: Clone;` (an associated type declaration): its bounds (a binding `dyn Tr<Item = T>` must meet them)
            // (one without bounds too: a `dyn Tr` must bind every associated type the trait leaves without a
            // default (E0191))
            ItemKind::Type(n, _, bs, None) => {
                let mut names = if bs.is_empty() { vec![] } else { bound_names(bs)? };
                // (and its lifetime bounds (`type Output: 'a;`): `'static`, or the trait's lifetime parameter by its
                // place, `'0` for the first (a placeholder's associated type outlives it, regionck))
                for b in bs {
                    if let Bound::Lifetime(l) = b {
                        let lts = ow.regions.as_deref().map(|r| r.lts.as_slice()).unwrap_or(&[]);
                        match lts.iter().position(|x| x == l) {
                            _ if l == "'static" => names.push(l.clone()),
                            Some(i) => names.push(format!("'{i}")),
                            None => {}
                        }
                    }
                }
                let mut sm = Summary::default();
                sm.assoc_bounds.push((ow.base.clone(), n.clone(), names));
                return Ok(sm);
            }
            ItemKind::Type(..) => return Ok(Summary::default()),
            // `const K: i32 = 3;` (with a default, a const named `Trait::K`)
            ItemKind::Const(n, t, e) => {
                let mut sm = Summary::default();
                if let (Some(t), Some(_)) = (t, e) {
                    sm.consts.push((format!("{}::{}", ow.base, n), tysyn(t, None)?, item.init_text.clone()));
                }
                return Ok(sm);
            }
            ItemKind::Fn(fs, body) => {
                let mut sig = parse_sig(fs, false, false, Some(ow))?;
                sig.has_body = body.is_some();
                // (`fn f(&self) -> impl Tr;` in a trait: each impl's method has its own type from its body; the
                // declaration's type is a placeholder (calls through a type parameter do not know it))
                if sig.ret == TySyn::Named(IMPL_RET.into()) && body.is_none() {
                    sig.ret = TySyn::Prim(Ty::Unit);
                }
                // (a default method `fn bar() -> impl Sized { .. }`: the type is its body's, as for a free fn)
                if sig.ret == TySyn::Named(IMPL_RET.into()) {
                    sig.ret = TySyn::Named(OPAQUE_RET.into());
                }
                let mut sm = Summary::default();
                sm.fns.push((format!("{}::{}", ow.base, fs.name), sig));
                where_types(item, Some(ow), &mut sm.where_types);
                return Ok(sm);
            }
            _ => return err("unsupported trait member"),
        }
    }
    let mut sm = summarize(item, owner)?;
    where_types(item, owner, &mut sm.where_types);
    Ok(sm)
}

/// Makes the members' owner from the summary of an impl / trait header (same as driver.rs)
pub fn owner_of(it: &SrcItem, sums: &[Summary]) -> Option<Owner> {
    match it.kind {
        LexKind::ImplMember(h) => {
            let hs = &sums[h as usize];
            hs.impl_self.clone().map(|n| Owner {
                self_ty: hs.impl_self_ty.clone().unwrap_or(TySyn::Named(n.clone())),
                base: n,
                impl_trait: hs.impl_trait.clone(),
                generics: hs.impl_generics.clone(),
                is_trait: false,
                regions: hs.hdr_regions.clone(),
            })
        }
        LexKind::TraitMember(h) => sums[h as usize].trait_decl.clone().map(|n| Owner {
            self_ty: TySyn::Named("Self".into()),
            generics: std::iter::once(("Self".to_string(), vec![n.clone()])).chain(sums[h as usize].trait_generics.iter().filter(|g| !g.0.starts_with('#')).cloned()).collect(),
            base: n,
            impl_trait: None,
            is_trait: true,
            regions: sums[h as usize].hdr_regions.clone(),
        }),
        _ => None,
    }
}

/// An array length in t is an operation on a const parameter (`[u8; N - 1]`; `[u8; N]` alone is fine)
fn const_param_in_op(t: &PTy, consts: &[&str]) -> bool {
    match t {
        PTy::Array(e, n) => {
            let bare = matches!(&**n, Expr::Path(None, p) if p.segs.len() == 1)
                || matches!(&**n, Expr::Block(None, false, b) if matches!(b.stmts.as_slice(), [Stmt::Expr(Expr::Path(None, p), false)] if p.segs.len() == 1));
            if !bare {
                let d = format!("{n:?}");
                if consts.iter().any(|c| d.contains(&format!("\"{c}\""))) {
                    return true;
                }
            }
            const_param_in_op(e, consts)
        }
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) => const_param_in_op(i, consts),
        PTy::Tuple(v) => v.iter().any(|x| const_param_in_op(x, consts)),
        _ => false,
    }
}

/// A method of `impl<T: Tr> Tr for &T` / `&mut T` that only forwards to T's: `(**self).m(a, b)` / `(*self).m(a, b)`, or
/// `T::m(&**self, a, b)` / `Tr::m(&mut **self, a, b)` / `Tr::m(*self, a, b)`, passing its own parameters in order (and
/// a turbofish only of its own generic parameters, in order: `(**self).m::<N>()`). A call on the reference reaches T's
/// method through the deref, which is what such a body does. Not `(*self).m()` for a `&self` method of an impl for
/// `&mut T`: `*self` is the `&mut T`, whose method lookup takes the impl's own `m` (through `&&mut T`) before T's, so
/// it calls itself (mut_ref: the impl is for `&mut T`)
fn forwards_to_referent(fs: &PSig, body: &Block, param: &str, tr: &str, mut_ref: bool) -> bool {
    // (the receiver: `&self` / `&mut self` (with a lifetime, `&'b self`, too), or a pinned one, `self: Pin<&mut Self>`
    // (futures' and tokio's poll methods), which forwards through `Pin::new(&mut **self)`)
    #[derive(PartialEq)]
    enum Recv {
        Ref(bool),
        PinMut,
    }
    let self_ty = |t: &PTy| matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "Self" && p.segs[0].args.is_empty());
    let recv = match fs.recv_ty.as_ref() {
        Some(PTy::Ref(_, m, i)) if self_ty(i) => Recv::Ref(*m),
        Some(PTy::Path(None, p))
            if p.segs.last().is_some_and(|s| s.name == "Pin")
                && matches!(p.segs.last().unwrap().args.as_slice(), [crate::parse::GArg::Ty(PTy::Ref(_, true, i))] if self_ty(i)) =>
        {
            Recv::PinMut
        }
        _ => return false,
    };
    fn strip(mut e: &Expr) -> &Expr {
        loop {
            match e {
                Expr::Paren(i) => e = i,
                // (`unsafe { (**self).m(..) }`, allocator-api2's: a block of the one expression)
                Expr::Block(None, _, b) if b.stmts.len() == 1 => match &b.stmts[0] {
                    Stmt::Expr(i, false) => e = i,
                    _ => return e,
                },
                _ => return e,
            }
        }
    }
    let [st] = body.stmts.as_slice() else { return false };
    let e = match st {
        Stmt::Expr(e, semi) if !*semi || fs.ret.is_none() => e,
        _ => return false,
    };
    let one = |e: &Expr, n: &str| matches!(strip(e), Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == n && p.segs[0].args.is_empty());
    let deref1 = |e: &Expr| matches!(strip(e), Expr::Unary(o, j, _) if o == "*" && one(j, "self"));
    let deref2 = |e: &Expr| matches!(strip(e), Expr::Unary(o, i, _) if o == "*" && deref1(i));
    // (a parameter `_` the impl cannot pass on: in its place the path of its type itself, as a value, is the unit
    // struct's one value (tokio's `_: sealed::Internal` passed as `(**self).to_socket_addrs(sealed::Internal)`); no
    // other value is taken (a const, or `Mode::Fast` of an enum, may differ from the caller's))
    let Some(names) = fs
        .params
        .iter()
        .map(|(p, t)| match (p, t) {
            (Pat::Ident(_, false, n, None), _) => Some(Ok(n.as_str())),
            (Pat::Wild, PTy::Path(None, tp)) if tp.segs.iter().all(|s| s.args.is_empty()) => Some(Err(tp)),
            _ => None,
        })
        .collect::<Option<Vec<Result<&str, &crate::parse::Path>>>>()
    else {
        return false;
    };
    let same_path = |e: &Expr, tp: &crate::parse::Path| {
        matches!(strip(e), Expr::Path(None, p) if p.segs.len() == tp.segs.len() && p.segs.iter().zip(&tp.segs).all(|(a, b)| a.name == b.name && a.args.is_empty()))
    };
    let args_ok = |args: &[Expr]| {
        args.len() == names.len()
            && args.iter().zip(&names).all(|(a, n)| match n {
                Ok(n) => one(a, n),
                Err(tp) => same_path(a, tp),
            })
    };
    // (a turbofish passes the method's own type and const parameters, in their order, or there is none)
    let own: Vec<&str> = fs.generics.params.iter().filter_map(|g| match g {
        GParam::Ty(n, ..) | GParam::Const(n, ..) => Some(n.as_str()),
        GParam::Lifetime(_) => None,
    }).collect();
    let targs_ok = |targs: &[crate::parse::GArg]| {
        targs.is_empty()
            || (targs.len() == own.len()
                && targs.iter().zip(&own).all(|(a, n)| match a {
                    crate::parse::GArg::Ty(PTy::Path(None, p)) => p.segs.len() == 1 && p.segs[0].name == *n && p.segs[0].args.is_empty(),
                    crate::parse::GArg::Const(e) => one(e, n),
                    _ => false,
                }))
    };
    // (`Pin::new(&mut **self)` (or of `self.get_mut()`): the pinned referent, T's own receiver)
    let pinned = |e: &Expr| {
        let Expr::Call(f, a) = strip(e) else { return false };
        let pin_new = matches!(strip(f), Expr::Path(None, p) if p.segs.len() == 2 && p.segs[0].name == "Pin" && p.segs[1].name == "new");
        let inner = |e: &Expr| {
            deref2(e)
                || matches!(strip(e), Expr::Unary(o, i, _) if o == "*"
                    && matches!(strip(i), Expr::Unary(o2, j, _) if o2 == "*"
                        && matches!(strip(j), Expr::MethodCall(r, m, t, a, _) if m == "get_mut" && t.is_empty() && a.is_empty() && one(r, "self"))))
        };
        pin_new && matches!(a.as_slice(), [x] if matches!(strip(x), Expr::Ref(true, i) if inner(i)))
    };
    // (the head of a path call: `T::m`, `Tr::m`, or `<T as Tr>::m` (futures-core's and quote's))
    let path_head = |f: &Expr| match strip(f) {
        Expr::Path(None, p) => p.segs.len() == 2 && (p.segs[0].name == param || p.segs[0].name == tr) && p.segs[1].name == fs.name && targs_ok(&p.segs[1].args),
        Expr::Path(Some(q), p) => {
            matches!(&**q, PTy::Path(None, qp) if qp.segs.len() == 1 && qp.segs[0].name == param && qp.segs[0].args.is_empty())
                && p.segs.last().is_some_and(|s| s.name == fs.name && targs_ok(&s.args))
                && p.segs.iter().rev().nth(1).is_none_or(|s| s.name == tr)
        }
        _ => false,
    };
    if recv == Recv::PinMut {
        return match strip(e) {
            Expr::MethodCall(r, m, targs, args, _) => *m == fs.name && pinned(r) && targs_ok(targs) && args_ok(args),
            // (`S::poll_next(Pin::new(&mut **self), cx)`, futures-core's)
            Expr::Call(f, args) => path_head(f) && args.first().is_some_and(pinned) && args_ok(&args[1..]),
            _ => false,
        };
    }
    // (`Pin::new(&**self).m(..)` of a `&self` method, http-body's: the referent behind a shared pin, whose `&self` method
    // is T's through Pin's deref)
    let pinned_shared = |e: &Expr| {
        let Expr::Call(f, a) = strip(e) else { return false };
        matches!(strip(f), Expr::Path(None, p) if p.segs.len() == 2 && p.segs[0].name == "Pin" && p.segs[1].name == "new")
            && matches!(a.as_slice(), [x] if matches!(strip(x), Expr::Ref(false, i) if deref2(i)))
    };
    // (`(*self).m(..)`: `*self` is the `&T`, whose own method is T's through the deref too; but not the `&mut T` of a
    // `&self` method, see above)
    let deref1_recv = !(mut_ref && recv == Recv::Ref(false));
    // (the first argument of a path call: `&**self` / `&mut **self`, `*self`, or, where the head names T (`T::m`, `<T
    // as Tr>::m`), `self` itself, which coerces to T's receiver through the deref (`T::read(self, buf)`, embedded-io's;
    // `Sp::spawn_obj(self, ..)`, futures-task's). Not after the trait's name: `Tr::m(self)` takes Self = `&T`, this impl
    // again)
    let names_param = |f: &Expr| match strip(f) {
        Expr::Path(None, p) => p.segs.first().is_some_and(|s| s.name == param),
        Expr::Path(Some(_), _) => true,
        _ => false,
    };
    let first_ok = |f: &Expr, a: &Expr| matches!(strip(a), Expr::Ref(_, i) if deref2(i)) || deref1(a) || (names_param(f) && one(a, "self"));
    match strip(e) {
        Expr::MethodCall(r, m, targs, args, _) => {
            *m == fs.name
                && (deref2(r) || (deref1_recv && deref1(r)) || (recv == Recv::Ref(false) && pinned_shared(r)))
                && targs_ok(targs)
                && args_ok(args)
        }
        Expr::Call(f, args) => path_head(f) && args.first().is_some_and(|a| first_ok(f, a)) && args_ok(&args[1..]),
        _ => false,
    }
}
