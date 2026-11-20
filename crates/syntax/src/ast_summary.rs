//! アイテムの要約 (front.rs の `summarize` / `parse_sig` / `lift_closures` / `parse_and_summarize`) を
//! 自前の構文木 (src/parse.rs) の上で行う。tests/ast_summary.rs が全ファイルで syn 版と同じ要約になることを確かめる。
//! 外部クレートに依存しない (parse / types / ast_types / ast_closure / lexer だけ) ので zustc 自身でもコンパイルできる。

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

/// 空白を除いた属性のテキスト (`#[derive(Debug,Clone)]`)
fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `#[name]` か `#[unsafe(name)]` が付いているか
fn has_attr(attrs: &[String], name: &str) -> bool {
    attrs.iter().any(|a| {
        let c = compact(a);
        c == format!("#[{name}]") || c.starts_with(&format!("#[{name}(")) || c.starts_with(&format!("#[{name}=")) || c == format!("#[unsafe({name})]")
    })
}

/// `#[derive(A, B)]` の名前 (1 つの識別子のものだけ)
fn derive_names(attrs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for a in attrs {
        let c = compact(a);
        let Some(inner) = c.strip_prefix("#[derive(").and_then(|x| x.strip_suffix(")]")) else { continue };
        for part in inner.split(',') {
            if !part.is_empty() && part.chars().all(|ch| ch == '_' || ch.is_alphanumeric()) {
                out.push(part.to_string());
            }
        }
    }
    out
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

/// `#[link_name = "malloc"]` の名前
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

/// 境界の名前 (front.rs の bound_names と同じ規則)
pub fn bound_names(bs: &[Bound]) -> R<Vec<String>> {
    let mut v = Vec::new();
    for b in bs {
        match b {
            // `?Sized` は制約を緩めるだけなので数えない
            Bound::Trait(true, _) => {}
            Bound::Trait(false, p) => {
                let last = p.segs.last().ok_or("empty bound")?;
                // (`iter::FromIterator<char>` はモジュールの部分を飛ばす)
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
            GParam::Ty(n, _, _) | GParam::Const(n, _) => {
                after_ty = true;
                // (the type of a const parameter: integers, bool, char)
                if let GParam::Const(_, t) = p {
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
            GParam::Const(n, _) => out.push((n.clone(), vec![CONST_GENERIC.to_string()])),
        }
    }
    for (t, bs) in &g.preds {
        // (型パラメータ以外への境界は読み飛ばす)
        let PTy::Path(None, p) = t else { continue };
        if p.global || p.segs.len() != 1 || !p.segs[0].args.is_empty() || p.segs[0].fn_args.is_some() {
            continue;
        }
        // (ライフタイムの述語 `'a: 'b`)
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
        let param = g.params.iter().any(|q| matches!(q, GParam::Ty(x, ..) | GParam::Const(x, _) if x == n));
        if param || owner.is_some_and(|o| o.generics.iter().any(|x| x.0 == n)) || out.iter().any(|x| x == n) {
            continue;
        }
        out.push(n.to_string());
    }
}

/// The plain trait bounds (one-segment paths without arguments, not `?Sized`) on a struct / enum's parameters
fn adt_bounds(g: &Generics) -> Vec<(String, Vec<String>)> {
    let mut v = Vec::new();
    for p in &g.params {
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

/// 型の空白なしのテキスト (syn のトークン列から空白を除いたものと同じ: `PartialEq<[U;N]>` の比較用)
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

/// 関数のシグネチャ (front.rs の parse_sig)
/// `-> impl Trait + use<..>`: the precise capture list names the function's own parameters
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
            GParam::Ty(n, _, _) | GParam::Const(n, _) => tys.push(n),
        }
    }
    for b in bs {
        let Bound::Trait(_, p) = b else { continue };
        let Some(s) = p.segs.last().filter(|s| s.name == "use" && p.segs.len() == 1) else { continue };
        for a in &s.args {
            match a {
                crate::parse::GArg::Lifetime(l) if l == "'_" => {
                    // (fine when an argument has an elided lifetime for it to name)
                    let elided = sig.recv_ty.is_some()
                        || sig.params.iter().any(|p| {
                            let (mut v, mut e, mut o) = (vec![], false, false);
                            lifetimes(&p.1, &mut v, &mut e, &mut o);
                            e
                        });
                    if !elided {
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
    let mut generics = owner.map(|o| o.generics.clone()).unwrap_or_default();
    collect_generics(&sig.generics, &mut generics)?;
    if is_extern && !generics.is_empty() {
        return err("extern fns cannot be generic");
    }
    if is_main && !generics.is_empty() {
        return err("`main` cannot be generic");
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
    if owner.is_none() {
        let declared: Vec<&str> = sig.generics.params.iter().filter_map(|g| match g {
            GParam::Lifetime(l) => Some(l.as_str()),
            _ => None,
        }).collect();
        let mut used = vec![];
        for (_, t) in &sig.params {
            named_lifetimes(t, &mut used);
        }
        if let Some(r) = &sig.ret {
            named_lifetimes(r, &mut used);
        }
        if let Some(l) = undeclared_lifetime(&used, &declared) {
            return Err(format!("use of undeclared lifetime name `{l}` (E0261)"));
        }
        // (E0106: an elided lifetime in the return type needs exactly one lifetime among the parameters)
        // (not the prelude's own `__exr_` helpers, written against this compiler rather than rustc)
        if sig.recv_ty.is_none() && !is_extern && !sig.name.starts_with("__") {
            if let Some(r) = &sig.ret {
                if lifetime_positions(r).0 > 0 {
                    let inputs: usize = sig.params.iter().map(|(_, t)| lifetime_positions(t).1).sum();
                    if inputs != 1 {
                        return err("missing lifetime specifier (E0106)");
                    }
                }
            }
        }
    }
    for (_, t) in &sig.params {
        // (E0277: a parameter's value needs a size; a trait's declaration without a body does not)
        if !owner.is_some_and(|o| o.is_trait) && !is_extern {
            if let Some(u) = unsized_value(t) {
                return Err(format!("the size for values of type `{u}` cannot be known at compilation time (E0277)"));
            }
        }
        // 引数位置の `impl Trait` は名前のない型パラメータ
        let (inner, rf) = match t {
            PTy::Ref(_, m, i) => (&**i, Some(*m)),
            t => (t, None),
        };
        if let PTy::Impl(bs) = inner {
            let g = format!("__impl{}", generics.len());
            generics.push((g.clone(), bound_names(bs)?));
            let n = TySyn::Named(g);
            params.push(match rf {
                Some(m) => TySyn::Ref(m, Box::new(n)),
                None => n,
            });
        } else {
            params.push(tysyn(t, self_ty)?);
        }
    }
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
    let symbol = match owner {
        _ if is_extern || is_main => name.clone(),
        Some(o) => match o.impl_trait.as_deref() {
            Some(t) if t == "Display" || t == "Debug" => format!("_x{}.{t}.{name}", o.base),
            Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => {
                let t: String = t.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
                format!("_x{}.{t}.{name}", o.base)
            }
            _ => format!("_x{}.{name}", o.base),
        },
        None => format!("_x{name}"),
    };
    let is_trait = owner.is_some_and(|o| o.is_trait);
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
        is_unsafe: sig.quals.iter().any(|q| q == "unsafe") || (is_extern && !sig.quals.iter().any(|q| q == "safe")),
    })
}

/// Checks on a struct / enum / union definition: unused parameters (E0392), `Self` in defaults (E0735), union fields
/// a slice or array whose element is unsized (`[[T]]`, `[str; 2]`, `[dyn Tr]`), anywhere in t: its text
/// The lifetimes a type names, outside fn pointer, `impl` and `dyn` types (which may bind their own with `for<'a>`)
fn named_lifetimes(t: &PTy, out: &mut Vec<String>) {
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

fn adt_checks(attrs: &[String], g: &Generics, tys: &[&PTy], union: bool, nfields: usize) -> R<()> {
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
        if let Some(l) = undeclared_lifetime(&used, &declared) {
            return Err(format!("use of undeclared lifetime name `{l}` (E0261)"));
        }
        // (E0106: a field's reference names its lifetime)
        if tys.iter().any(|t| lifetime_positions(t).0 > 0) {
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
            return Err(format!("cannot find derive macro `{d}` in this scope"));
        }
    }
    if union {
        if nfields == 0 {
            return err("unions cannot have zero fields");
        }
        if let Some(d) = derives.iter().find(|d| !matches!(d.as_str(), "Clone" | "Copy")) {
            return Err(format!("this trait cannot be derived for unions: `{d}`"));
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
                return err("field must implement `Copy` or be wrapped in `ManuallyDrop<...>` to be used in a union");
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

/// SigSyn::ret_from: the elision rules (`&self` methods return borrows of self, else the only argument with
/// lifetimes) and named lifetimes (`fn f<'a>(x: &'a T, y: &U) -> &'a T` borrows from x only)
fn ret_from(sig: &PSig) -> u64 {
    const KNOWN: u64 = 1 << 63;
    let Some(ret) = &sig.ret else { return KNOWN };
    let (mut rl, mut relided, mut opaque) = (vec![], false, false);
    lifetimes(ret, &mut rl, &mut relided, &mut opaque);
    if opaque {
        return 0;
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

/// `Fn(A, B) -> R` 形式の境界なら (種類, 引数, 戻り値)
pub fn parse_fn_bound(s: &str) -> Option<(String, Vec<TySyn>, TySyn)> {
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

/// 境界 `Iterator<Item = T>` の関連型の束縛 (名前, 型)
pub fn assoc_bindings(s: &str) -> Vec<(String, TySyn)> {
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

// ---------------------------------------------------------------- クロージャの持ち上げ

/// パターンの中のすべてのパターン (束縛の名前を集める)
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

/// 関数 (入れ子のクロージャを含む) のどこかで束縛される名前
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

/// 本体の `let n = ..` から、返すクロージャがキャプチャした変数の型 (注釈か初期化式のリテラル)
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

/// リテラルのテキストから型 (整数は接尾辞 → 文脈 → i32)
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
    // 接尾辞: 数字・`_`・(16 進なら a-f) の後ろの英字
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
fn has_infer(t: &PTy) -> bool {
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

/// 引数の名前 (識別子のパターンなら)
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
        for n in &caps {
            moved.push(*mv || (ast_closure::moves_capture(c, &[n.clone()]) && !assigns));
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
        });
    }
    // `-> impl Fn(A) -> B { move |x| .. }`: 型は末尾のクロージャの struct
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

// ---------------------------------------------------------------- 要約

/// 1 つのアイテムの要約 (front.rs の summarize)
pub fn summarize(item: &Item, owner: Option<&Owner>) -> R<Summary> {
    let mut sm = Summary::default();
    match &item.kind {
        ItemKind::Type(n, _, _, Some(t)) if owner.is_some() => {
            let ow = owner.unwrap();
            sm.assocs.push(AssocSyn {
                name: n.clone(),
                self_ty: ow.self_ty.clone(),
                generics: ow.generics.iter().map(|g| g.0.clone()).collect(),
                value: tysyn(t, Some(&ow.self_ty))?,
            });
        }
        ItemKind::Fn(fs, Some(body)) if owner.is_some() => {
            let ow = owner.unwrap();
            let mut sig = parse_sig(fs, false, false, Some(ow))?;
            let name = match ow.impl_trait.as_deref() {
                Some(t) if t == "Display" || t == "Debug" => format!("{}::<{t}>::{}", ow.base, fs.name),
                Some(t) if t.starts_with("PartialEq<") || t.starts_with("PartialOrd<") => format!("{}::<{t}>::{}", ow.base, fs.name),
                _ if matches!(ow.self_ty, TySyn::Ref(..)) => format!("{}::__ref_{}", ow.base, fs.name),
                _ => format!("{}::{}", ow.base, fs.name),
            };
            lift_closures(fs, body, &name, &mut sig, Some(&ow.self_ty), &mut sm)?;
            sm.fns.push((name, sig));
        }
        // 型の別名 (`type Key = u128;`)
        ItemKind::Type(n, g, _, Some(t)) => {
            let mut gs = Vec::new();
            collect_generics(g, &mut gs)?;
            let consts: Vec<&str> = g.params.iter().filter_map(|p| match p {
                GParam::Const(n, _) => Some(n.as_str()),
                _ => None,
            }).collect();
            if !consts.is_empty() && const_param_in_op(t, &consts) {
                return err("generic parameters may not be used in const operations");
            }
            sm.aliases.push((n.clone(), gs.into_iter().map(|x| x.0).collect(), tysyn(t, None)?));
        }
        ItemKind::Trait(n, g, supers, _) => {
            let mut sup = vec![];
            for b in supers {
                if let Bound::Trait(false, p) = b {
                    if let Some(s) = p.segs.last() {
                        if !matches!(s.name.as_str(), "Sized" | "Send" | "Sync" | "Copy" | "Clone" | "Unpin" | "Any" | "Debug" | "Display") {
                            sup.push(s.name.clone());
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
            sm.trait_decl = Some(n.clone());
        }
        ItemKind::Impl(g, tr, self_ty, _) => {
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
                // `impl PartialEq<[U; N]> for Vec<T>`: 右辺の型が self 型と違う比較は trait 名に型引数を残す
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
            // `impl Add for &M`: M のメソッド `__ref_add` (self は &M) として持つ
            let self_path = match self_ty {
                PTy::Ref(_, _, i) if tr.is_some() => &**i,
                t => t,
            };
            if let (PTy::Tuple(v), true) = (self_path, tr.is_some()) {
                sm.impl_self = Some(format!("#tuple{}", v.len()));
                sm.impl_self_ty = Some(tysyn(self_ty, None)?);
                return Ok(sm);
            }
            // (`impl Tr for fn(A) -> B`: the methods of `__fnptr`)
            if matches!(self_path, PTy::Fn(..) | PTy::Ptr(..) | PTy::Never) && tr.is_some() {
                sm.impl_self = Some(match self_path {
                    PTy::Fn(..) => "__fnptr",
                    PTy::Ptr(..) => "__ptr",
                    _ => "__never",
                }.to_string());
                sm.impl_self_ty = tysyn(self_ty, None).ok();
                return Ok(sm);
            }
            if matches!(self_path, PTy::Array(..) | PTy::Slice(_)) && tr.is_some() {
                sm.impl_self = Some(if matches!(self_path, PTy::Array(..)) { "__array" } else { "__slice" }.to_string());
                sm.impl_self_ty = Some(tysyn(self_ty, None)?);
                return Ok(sm);
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
                return Ok(sm);
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
                return Ok(sm);
            }
            sm.impl_self = Some(tp.segs[0].name.clone());
            sm.impl_self_ty = Some(tysyn(self_ty, None)?);
        }
        ItemKind::Fn(fs, Some(body)) => {
            let name = fs.name.clone();
            if !item.init_text.is_empty() && fs.generics.params.is_empty() {
                let ps: Option<Vec<String>> = fs.params.iter().map(|(p, _)| match p {
                    Pat::Ident(false, _, n, None) => Some(n.clone()),
                    _ => None,
                }).collect();
                if let Some(ps) = ps {
                    sm.const_fns.push((name.clone(), ps, item.init_text.clone()));
                }
            }
            let export = has_attr(&item.attrs, "no_mangle");
            let is_main = name == "main" && !export;
            let mut sig = parse_sig(fs, false, is_main, None)?;
            let result_ret = matches!(&sig.ret, TySyn::App(n, _) if n == "Result");
            if is_main && (!sig.params.is_empty() || (sig.ret != TySyn::Prim(Ty::Unit) && !result_ret)) {
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
            lift_closures(fs, body, &name, &mut sig, None, &mut sm)?;
            sm.fns.push((name, sig));
        }
        ItemKind::Use(_) => {}
        ItemKind::Const(n, Some(t), Some(_)) => {
            if let Some(u) = unsized_value(t) {
                return Err(format!("the size for values of type `{u}` cannot be known at compilation time (E0277)"));
            }
            let name = match owner {
                Some(ow) => format!("{}::{}", ow.base, n),
                None => n.clone(),
            };
            sm.consts.push((name, tysyn(t, owner.map(|o| &o.self_ty))?, item.init_text.clone()));
        }
        ItemKind::Static(m, n, t, _) => {
            let mut used = vec![];
            named_lifetimes(t, &mut used);
            if let Some(l) = undeclared_lifetime(&used, &[]) {
                return Err(format!("use of undeclared lifetime name `{l}` (E0261)"));
            }
            if let Some(u) = unsized_value(t) {
                return Err(format!("the size for values of type `{u}` cannot be known at compilation time (E0277)"));
            }
            sm.statics.push((n.clone(), tysyn(t, None)?, item.init_text.clone(), *m));
        }
        ItemKind::Extern(items) => {
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
                    return err("only fns and statics are supported in extern blocks");
                };
                if link_name(&fi.attrs).unwrap_or_else(|| fs.name.clone()).starts_with("__enzyme_") {
                    return Err(format!("{}:1: linking to Enzyme intrinsics is experimental", fi.line));
                }
                if fi.attrs.iter().any(|a| compact(a).starts_with("#[linkage")) {
                    return Err(format!("{}:1: the `linkage` attribute is experimental and not portable", fi.line));
                }
                let mut sig = parse_sig(fs, true, false, None)?;
                if let Some(l) = link_name(&fi.attrs) {
                    sig.symbol = l;
                }
                sm.fns.push((fs.name.clone(), sig));
            }
        }
        ItemKind::Enum(sn, g, vars) => {
            {
                let mut tys: Vec<&PTy> = vec![];
                for (_, f, _) in vars {
                    match f {
                        Fields::Named(v) => tys.extend(v.iter().map(|x| &x.1)),
                        Fields::Tuple(v) => tys.extend(v.iter()),
                        Fields::Unit => {}
                    }
                }
                sm.adt_err = adt_checks(&item.attrs, g, &tys, false, vars.len()).err();
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
            }
            sm.adt_generics = adt_params(g)?;
            sm.adt_bounds = adt_bounds(g);
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
                    Some(e) => match const_int(e) {
                        Ok(v) => Some(v),
                        // (`Bar = QUUX` / `Y = Foo::X as isize - 3`: Program evaluates it with the constants)
                        Err(er) => {
                            discr_src = crate::ast_types::const_expr_text(e).ok_or(er)?;
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
        }
        ItemKind::Struct(sn, g, fields) | ItemKind::Union(sn, g, fields) => {
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
                sm.adt_err = adt_checks(&item.attrs, g, &tys, union, tys.len()).err();
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
                        Fields::Named(v) => v.iter().find(|x| lifetime_positions(&x.1).0 > 0).map(|x| x.0.clone()),
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
            sm.adt_generics = adt_params(g)?;
            sm.adt_bounds = adt_bounds(g);
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
        }
        _ => return err("unsupported item"),
    }
    Ok(sm)
}

/// 字句解析のアイテム (lexer) 1 つを構文解析して要約する (front.rs の parse_and_summarize)
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
    let Some(item) = f.items.first() else { return Ok(Summary::default()) };
    if let LexKind::TraitMember(_) = it.kind {
        let ow = owner.ok_or("trait member without trait")?;
        match &item.kind {
            // `type Item: Clone;` (関連型の宣言): its bounds (a binding `dyn Tr<Item = T>` must meet them)
            // (one without bounds too: a `dyn Tr` must bind every associated type the trait leaves without a
            // default (E0191))
            ItemKind::Type(n, _, bs, None) => {
                let names = if bs.is_empty() { vec![] } else { bound_names(bs)? };
                let mut sm = Summary::default();
                sm.assoc_bounds.push((ow.base.clone(), n.clone(), names));
                return Ok(sm);
            }
            ItemKind::Type(..) => return Ok(Summary::default()),
            // `const K: i32 = 3;` (既定値があれば `Trait::K` という const)
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

/// impl / trait のヘッダの要約からメンバの所有者を作る (driver.rs と同じ)
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
            })
        }
        LexKind::TraitMember(h) => sums[h as usize].trait_decl.clone().map(|n| Owner {
            self_ty: TySyn::Named("Self".into()),
            generics: std::iter::once(("Self".to_string(), vec![n.clone()])).chain(sums[h as usize].trait_generics.iter().filter(|g| g.0 != "#supers").cloned()).collect(),
            base: n,
            impl_trait: None,
            is_trait: true,
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
