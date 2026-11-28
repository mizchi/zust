//! Type syntax (our own tree's `parse::Ty`) to `TySyn`: a port of `parse_tysyn` in front.rs (the syn version).
//! tests/ast_types.rs checks that it gives the syn version's results for the types in every file.
//! It depends on no external crate (only src/parse.rs and src/types.rs), so zustc itself can compile it.

use crate::parse::{Bound, Expr, GArg, Path, Stmt, Ty as PTy};
use crate::types::{ArrLen, Ty, TySyn};

pub type R<T> = Result<T, String>;

pub fn ffi_alias(s: &str) -> Option<Ty> {
    Some(match s {
        "c_char" | "c_schar" => Ty::I8,
        "c_uchar" => Ty::U8,
        "c_short" => Ty::I16,
        "c_ushort" => Ty::U16,
        "c_int" => Ty::I32,
        "c_uint" => Ty::U32,
        "c_long" | "c_longlong" => Ty::I64,
        "c_ulong" | "c_ulonglong" => Ty::U64,
        "c_float" => Ty::F32,
        "c_double" => Ty::F64,
        "c_size_t" => Ty::Usize,
        "c_ssize_t" | "c_ptrdiff_t" => Ty::Isize,
        _ => return None,
    })
}

fn lit_u64(t: &str) -> Option<u64> {
    // (drop `_`, take the radix from the prefix, read while digits follow: a suffix such as `usize` is dropped)
    let t: String = t.chars().filter(|&c| c != '_').collect();
    let (radix, body) = if let Some(h) = t.strip_prefix("0x") {
        (16, h)
    } else if let Some(b) = t.strip_prefix("0b") {
        (2, b)
    } else if let Some(o) = t.strip_prefix("0o") {
        (8, o)
    } else {
        (10, t.as_str())
    };
    let digits: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
    u64::from_str_radix(&digits, radix).ok()
}

/// A const generic argument (`Buf<4>` / `Buf<N>` / `Buf<{ 4 }>`): an array type `[(); n]` carrying the length
/// (`Flag<true>`: `true` / `false` in a type's arguments, which the parser reads as a type path, is a const argument:
/// 1 / 0, as an integer const argument is)
pub fn bool_arg(t: &PTy) -> Option<TySyn> {
    let PTy::Path(None, p) = t else { return None };
    let n = match single(p)? {
        "true" => 1,
        "false" => 0,
        _ => return None,
    };
    Some(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Lit(n)))
}

pub fn const_arg_syn(e: &Expr) -> R<TySyn> {
    match e {
        Expr::Lit(t, _) if t == "true" || t == "false" => Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Lit((t == "true") as u64))),
        Expr::Lit(t, _) if t.starts_with(|c: char| c.is_ascii_digit()) => match lit_u64(t) {
            Some(n) => Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Lit(n))),
            None => Err("bad const argument".to_string()),
        },
        Expr::Block(None, false, b) if b.stmts.len() == 1 => match &b.stmts[0] {
            Stmt::Expr(e, false) => const_arg_syn(e),
            _ => Err("unsupported const argument".to_string()),
        },
        Expr::Path(None, p) if single(p).is_some() => {
            Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Const(single(p).unwrap().to_string())))
        }
        // (integer const expression `{ u128::MAX_STR_LEN - 16 }`: its text replaces a const name; Program evaluates it)
        e => match const_expr_text(e) {
            Some(t) => Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Const(t))),
            None => Err("const arguments must be a literal or a name".to_string()),
        },
    }
}

/// Text of an integer const expression (literal, path, binary op, unary `-`, parens, no-argument method) or None
pub(crate) fn const_expr_text(e: &Expr) -> Option<String> {
    Some(match e {
        Expr::Lit(t, _) => t.clone(),
        Expr::Path(None, p) if !p.global && p.segs.iter().all(|s| s.args.is_empty() && s.fn_args.is_none()) => {
            p.segs.iter().map(|s| s.name.as_str()).collect::<Vec<&str>>().join("::")
        }
        Expr::Binary(op, l, r, _) => format!("({} {op} {})", const_expr_text(l)?, const_expr_text(r)?),
        Expr::Unary(op, x, _) if op == "-" => format!("(-{})", const_expr_text(x)?),
        Expr::Paren(x) => const_expr_text(x)?,
        // (`size_of::<c_long>()` / `mem::align_of::<T>()`: Program::eval_const knows primitives, pointers and aliases
        // of them; libc 0.2's `[u8; 128 - 2 * size_of::<c_int>()]`)
        Expr::Call(f, args)
            if args.is_empty()
                && matches!(&**f, Expr::Path(None, p) if p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "size_of" | "align_of")
                    && matches!(s.args.as_slice(), [crate::parse::GArg::Ty(_)]))) =>
        {
            let Expr::Path(None, p) = &**f else { return None };
            let last = p.segs.last()?;
            let prefix_ok = p.segs[..p.segs.len() - 1].iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
            let prefix_ok = matches!(prefix_ok.as_slice(), [] | ["mem"] | ["core" | "std", "mem"] | ["crate", "mem"]);
            if !prefix_ok || !matches!(last.name.as_str(), "size_of" | "align_of") || last.args.len() != 1 {
                return None;
            }
            let crate::parse::GArg::Ty(t) = &last.args[0] else { return None };
            let ty = match t {
                // (a pointer to a slice / str / trait object stays a fat one: two words)
                crate::parse::Ty::Ptr(_, i) | crate::parse::Ty::Ref(_, _, i)
                    if matches!(&**i, crate::parse::Ty::Slice(_) | crate::parse::Ty::Dyn(_))
                        || matches!(&**i, crate::parse::Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "str") =>
                {
                    "*const [()]".to_string()
                }
                crate::parse::Ty::Ptr(..) | crate::parse::Ty::Ref(..) => "*const ()".to_string(),
                crate::parse::Ty::Path(None, tp) if tp.segs.iter().all(|s| s.args.is_empty()) => tp.segs.last()?.name.clone(),
                crate::parse::Ty::Tuple(v) if v.is_empty() => "()".to_string(),
                _ => return None,
            };
            // (a path to core's keeps it, so a program's own fn of the name does not take the call: `mem::size_of`
            //  is core's even beside `fn size_of`)
            if p.segs.len() > 1 { format!("core::mem::{}::<{ty}>()", last.name) } else { format!("{}::<{ty}>()", last.name) }
        }
        Expr::MethodCall(r, m, g, a, _) if g.is_empty() && a.is_empty() => format!("{}.{m}()", const_expr_text(r)?),
        Expr::Unary(op, x, _) if op == "!" => format!("(!{})", const_expr_text(x)?),
        // (a cast keeps its type's name, which Program::eval_const wraps to when it is an integer type or an alias
        // of one: `-1i8 as i32` is -1, `4294967298u64 as i32` is 2; another type does not change the value)
        Expr::Cast(x, t) => {
            let inner = const_expr_text(x)?;
            match &**t {
                crate::parse::Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => format!("({inner} as {})", p.segs[0].name),
                // (a primitive by its path: `as core::primitive::u16`)
                crate::parse::Ty::Path(None, p)
                    if p.segs.len() == 3 && matches!(p.segs[0].name.as_str(), "core" | "std") && p.segs[1].name == "primitive" && p.segs.iter().all(|s| s.args.is_empty()) =>
                {
                    format!("({inner} as {})", p.segs[2].name)
                }
                _ => inner,
            }
        }
        // (`unsafe { 1 + 2 }` inside another expression keeps its `unsafe`: `1 + (unsafe { 1 + 2 })`)
        Expr::Block(None, true, b) if b.stmts.len() == 1 => match &b.stmts[0] {
            Stmt::Expr(x, false) => format!("(unsafe {{ {} }})", const_expr_text(x)?),
            _ => return None,
        },
        Expr::Block(None, false, b) | Expr::Const(b) if b.stmts.len() == 1 => match &b.stmts[0] {
            Stmt::Expr(x, false) => const_expr_text(x)?,
            _ => return None,
        },
        // (`{ let x = 2; 5 }`: the `let`s and the value. Items and closures in it do not change the value)
        Expr::Block(None, false, b) | Expr::Const(b) if !b.stmts.is_empty() => {
            let mut out = String::from("{ ");
            let n = b.stmts.len();
            for (k, s) in b.stmts.iter().enumerate() {
                match s {
                    Stmt::Let(crate::parse::Pat::Ident(false, _, v, None), _, Some(init), None, ..) => {
                        out.push_str(&format!("let {v} = {}; ", const_expr_text(init)?));
                    }
                    Stmt::Item(_) | Stmt::Expr(Expr::Closure(..) | Expr::Lit(..), true) => {}
                    // (`x = 3;` / `x += 1;` to a local of the block)
                    Stmt::Expr(Expr::Assign(l, r), _) if matches!(&**l, Expr::Path(None, p) if single(p).is_some()) => {
                        let Expr::Path(None, p) = &**l else { unreachable!() };
                        out.push_str(&format!("{} = {}; ", single(p)?, const_expr_text(r)?));
                    }
                    Stmt::Expr(Expr::AssignOp(op, l, r, _), _) if matches!(&**l, Expr::Path(None, p) if single(p).is_some()) => {
                        let Expr::Path(None, p) = &**l else { unreachable!() };
                        let v = single(p)?;
                        out.push_str(&format!("{v} = ({v} {op} {}); ", const_expr_text(r)?));
                    }
                    Stmt::Expr(x, false) if k + 1 == n => out.push_str(&const_expr_text(x)?),
                    _ => return None,
                }
            }
            out.push_str(" }");
            out
        }
        Expr::Ref(false, x) => const_expr_text(x)?,
        Expr::Unary(op, x, _) if op == "*" => const_expr_text(x)?,
        Expr::Index(b, i, _) => format!("{}[{}]", const_expr_text(b)?, const_expr_text(i)?),
        // (a `const fn` call: `[i32; add(1, 2)]`)
        Expr::Call(f, args) if matches!(&**f, Expr::Path(None, p) if single(p).is_some()) => {
            let Expr::Path(None, p) = &**f else { unreachable!() };
            let a: Option<Vec<String>> = args.iter().map(const_expr_text).collect();
            format!("{}({})", single(p)?, a?.join(", "))
        }
        Expr::Field(b, f, _) => format!("{}.{f}", const_expr_text(b)?),
        Expr::If(c, t, Some(e)) => {
            let t = match t.stmts.as_slice() {
                [Stmt::Expr(x, false)] => const_expr_text(x)?,
                _ => return None,
            };
            format!("(if {} {{ {t} }} else {{ {} }})", const_expr_text(c)?, const_expr_text(e)?)
        }
        _ => return None,
    })
}

/// A path that is a single identifier
fn single(p: &Path) -> Option<&str> {
    if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() {
        Some(&p.segs[0].name)
    } else {
        None
    }
}

/// `dyn Send` / `dyn Sync + 'a`: a trait object of an auto trait alone, its trait. Read only right behind a pointer
/// (`Box<dyn Send>`, `&dyn Sync`): other places (`Rc<dyn Send>`, `Wrapper<dyn Send>`) need unsizing this compiler
/// does not do
/// std's auto traits: no methods, so a `dyn` type has them beside its one trait (`dyn Error + Send + Sync +
/// UnwindSafe`)
pub fn is_auto_trait(name: &str) -> bool {
    matches!(name, "Send" | "Sync" | "Unpin" | "UnwindSafe" | "RefUnwindSafe")
}

// (`dyn Send + Sync` / `dyn Sync + Send` is `Send + Sync`, the text the other objects' auto traits are written in
// (types::dyn_autos): the first is the object's trait, the second a kept auto trait)
fn auto_dyn(t: &PTy) -> Option<String> {
    let PTy::Dyn(bs) = t else { return None };
    let mut autos: Vec<String> = vec![];
    for b in bs {
        match b {
            Bound::Trait(_, p) if p.segs.last().is_some_and(|s| is_auto_trait(&s.name)) => {
                let n = p.segs.last().unwrap().name.clone();
                if !autos.contains(&n) {
                    autos.push(n);
                }
            }
            Bound::Lifetime(_) => {}
            _ => return None,
        }
    }
    let first = autos.first()?.clone();
    if autos.iter().any(|a| a == "Send") && autos.iter().any(|a| a == "Sync") {
        return Some("Send + Sync".to_string());
    }
    Some(first)
}

pub fn tysyn(t: &PTy, self_ty: Option<&TySyn>) -> R<TySyn> {
    // (the parser's marker of `dyn Future<Output = T>` behind Box / Pin: futures run at once, so it is T)
    if let PTy::Path(None, p) = t {
        if p.segs.len() == 1 && p.segs[0].name == "__exr_fut" {
            if let [crate::parse::GArg::Ty(o)] = &p.segs[0].args[..] {
                return tysyn(o, self_ty);
            }
        }
    }
    Ok(match t {
        PTy::Never => TySyn::Prim(Ty::Never),
        PTy::Tuple(v) if v.is_empty() => TySyn::Prim(Ty::Unit),
        PTy::Tuple(v) => {
            let mut out = Vec::new();
            for e in v {
                out.push(tysyn(e, self_ty)?);
            }
            TySyn::Tuple(out)
        }
        PTy::Fn(ps, r) => {
            let mut v = Vec::new();
            for a in ps {
                v.push(tysyn(a, self_ty)?);
            }
            v.push(tysyn(r, self_ty)?);
            TySyn::FnPtr(v)
        }
        PTy::Dyn(bs) => {
            let mut tr: Option<String> = None;
            // (`dyn Send`: an auto trait alone is the object's trait)
            let mut auto: Option<String> = None;
            for b in bs {
                match b {
                    // (`dyn Error + Send + Sync`: auto traits are skipped)
                    Bound::Trait(_, p) if p.segs.last().is_some_and(|s| is_auto_trait(&s.name)) => {
                        auto.get_or_insert_with(|| p.segs.last().map(|s| s.name.clone()).unwrap_or_default());
                    }
                    Bound::Trait(_, p) if tr.is_none() => tr = Some(p.segs.last().map(|s| s.text.clone()).unwrap_or_default()),
                    Bound::Lifetime(_) => {}
                    _ => return Err("only one trait is supported in `dyn`".to_string()),
                }
            }
            match tr {
                // (`+ Send` / `+ Sync` are kept, in that order: they decide the object's own auto traits)
                Some(s) => {
                    let has = |n: &str| bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name == n)));
                    let mut s = s;
                    for a in ["Send", "Sync"] {
                        if has(a) {
                            s.push_str(" + ");
                            s.push_str(a);
                        }
                    }
                    TySyn::Dyn(s)
                }
                // (an auto trait alone is read right behind a pointer only, see auto_dyn)
                None => {
                    let _ = auto;
                    return Err("`dyn` needs a trait".to_string());
                }
            }
        }
        // Lifetimes do not affect codegen, so they are erased
        PTy::Ref(_, m, i) => TySyn::Ref(*m, Box::new(auto_dyn(i).map(TySyn::Dyn).map(Ok).unwrap_or_else(|| tysyn(i, self_ty))?)),
        PTy::Ptr(m, i) => TySyn::Ptr(*m, Box::new(auto_dyn(i).map(TySyn::Dyn).map(Ok).unwrap_or_else(|| tysyn(i, self_ty))?)),
        PTy::Slice(i) => TySyn::Slice(Box::new(tysyn(i, self_ty)?)),
        PTy::Array(e, n) => {
            let len = match &**n {
                Expr::Lit(t, _) if t.starts_with(|c: char| c.is_ascii_digit()) => match lit_u64(t) {
                    Some(v) => ArrLen::Lit(v),
                    None => return Err("bad array length".to_string()),
                },
                Expr::Path(None, p) if single(p).is_some() => ArrLen::Const(single(p).unwrap().to_string()),
                // Associated consts `[u8; Buf::CAP]` / `[u8; Self::CAP]`
                Expr::Path(None, p) if !p.global && p.segs.len() == 2 => ArrLen::Const(format!("{}::{}", self_name(&p.segs[0].name, self_ty), p.segs[1].name)),
                // (`[u8; <Foo as Bar>::SIZE]`: the impl's associated const, `Foo as Bar::SIZE` when an inherent
                // `Foo::SIZE` took the plain name, else `Foo::SIZE`)
                Expr::Path(Some(q), p) if p.segs.len() == 2 && matches!(&**q, PTy::Path(None, qp) if qp.segs.len() == 1 && qp.segs[0].args.is_empty()) => {
                    let PTy::Path(None, qp) = &**q else { unreachable!() };
                    ArrLen::Const(format!("{} as {}::{}", self_name(&qp.segs[0].name, self_ty), p.segs[0].name, p.segs[1].name))
                }
                // (a constant expression of literals: `[u8; 1 + 1]`, `[T; 4 as usize]`)
                x => match crate::ast_summary::const_int(x) {
                    Ok(v) if (0..=u32::MAX as i64).contains(&v) => ArrLen::Lit(v as u64),
                    // (an expression over other consts, `[u8; N * 2]` / `[T; ARR[0]]`: its text, for Program to evaluate)
                    Err(_) => match const_expr_text(x) {
                        Some(t) => ArrLen::Const(t),
                        None => return Err("array length must be a literal or a const".to_string()),
                    },
                    _ => return Err("array length must be a literal or a const".to_string()),
                },
            };
            TySyn::Array(Box::new(tysyn(e, self_ty)?), len)
        }
        PTy::Infer => TySyn::Infer,
        // `<T as Iterator>::Item`
        PTy::Path(Some(q), p) if !p.segs.is_empty() => {
            // (`<T as Tr>::Out<u8>`, a generic associated type in a qualified path, is not taken apart yet: only
            // `T::Out<u8>` is)
            let name = p.segs.last().unwrap().name.clone();
            // (`<i32 as Local>::Output` of a trait other than std's operators / Deref: an impl must give it, not std's
            // impls for the numbers, which Program answers `<i32 as Add>::Output` with)
            let tr = p.segs.len().checked_sub(2).map(|i| p.segs[i].name.as_str());
            // (`<dyn Trait<i32>>::Ty`: an associated type of a trait object named without its trait, E0223 as in
            // rustc; a type parameter's (`<T>::Item`) is its bound's)
            if tr.is_none() && matches!(&**q, PTy::Dyn(_)) {
                return Err(format!("ambiguous associated type `{name}`: use fully-qualified syntax, `<dyn .. as Trait>::{name}` (E0223)"));
            }
            let std_tr = matches!(
                tr,
                Some("Add" | "Sub" | "Mul" | "Div" | "Rem" | "Neg" | "Not" | "BitAnd" | "BitOr" | "BitXor" | "Shl" | "Shr" | "Deref" | "DerefMut")
            );
            let name = if tr.is_some() && !std_tr && matches!(name.as_str(), "Output" | "Target") { format!("{NO_STD_ASSOC}{name}") } else { name };
            // (`<S as Trait<usize>>::Assoc`: the trait's type arguments choose the impl, as the trait solver's matching
            // of heads does. The associated type `#targs:Trait#Assoc` of the tuple (S, usize), as for a generic
            // associated type; Program::resolve takes it apart)
            // (const arguments too, as impl headers keep them: `<S as Tr<2>>::Item`)
            let targs: Vec<&crate::parse::GArg> = p.segs.len().checked_sub(2).map(|i| &p.segs[i]).map_or(vec![], |sg| {
                sg.args.iter().filter(|a| matches!(a, crate::parse::GArg::Ty(_) | crate::parse::GArg::Const(_))).collect()
            });
            // (a const argument that cannot be lowered (`{ match .. }`): a hole, the trait kept; Program::resolve takes
            // the impl only when one value fits whatever the hole is)
            let lowered: Option<Vec<TySyn>> = targs
                .iter()
                .map(|a| match a {
                    crate::parse::GArg::Ty(t) => tysyn(t, self_ty).ok(),
                    crate::parse::GArg::Const(e) => Some(const_arg_syn(e).unwrap_or(TySyn::Infer)),
                    _ => None,
                })
                .collect();
            if let (Some(tr), false, false, Some(lowered)) = (tr, targs.is_empty(), std_tr, lowered) {
                let mut parts = vec![tysyn(q, self_ty)?];
                parts.extend(lowered);
                return Ok(TySyn::Assoc(Box::new(TySyn::Tuple(parts)), format!("{TARGS_PREFIX}{tr}#{name}")));
            }
            // (the trait too, when its arguments did not take the path above: `<&J as BitAnd<&J>>::Output` beside `impl Not
            // for &J`, that trait's impl gives it)
            let name = match tr {
                Some(t) if !t.is_empty() && !t.contains(':') => format!("{TRAIT_ASSOC}{t}:{name}"),
                _ => name,
            };
            TySyn::Assoc(Box::new(tysyn(q, self_ty)?), name)
        }
        PTy::Path(None, p) => path_ty(p, self_ty)?,
        _ => return Err("unsupported type".to_string()),
    })
}

/// The name of a qualified path's associated type whose trait has type arguments: `#targs:Trait#Assoc` of the tuple of
/// the self type and the arguments (`<S as Trait<usize>>::Assoc` is `#targs:Trait#Assoc` of `(S, usize)`)
pub const TARGS_PREFIX: &str = "#targs:";

/// The name a generic associated type's application has: `P::Pointer<String>` is the associated type
/// `#gat:Pointer` of the tuple `(P, String)` (Program::assoc_of takes it apart)
pub const GAT_PREFIX: &str = "#gat:";

/// The mark of an associated type a qualified path names of a trait other than std's operators (`<i32 as
/// Local>::Output`): std's impls for the numbers do not give it
pub const NO_STD_ASSOC: &str = "#impl:";

/// The mark of an associated type a qualified path names with its trait, `#tr:BitAnd:Output` of `<&J as
/// BitAnd<&J>>::Output`: the impl of that trait gives it, before another trait's of the same name for the type
pub const TRAIT_ASSOC: &str = "#tr:";

/// An associated type's name without the trait a qualified path gave it, and that trait
pub fn split_trait_assoc(n: &str) -> (&str, Option<&str>) {
    match n.strip_prefix(TRAIT_ASSOC).and_then(|r| r.split_once(':')) {
        Some((t, rest)) => (rest, Some(t)),
        None => (n, None),
    }
}

fn gat_app(base: TySyn, seg: &crate::parse::Seg, self_ty: Option<&TySyn>) -> R<TySyn> {
    let mut parts = vec![base];
    for a in &seg.args {
        match a {
            // (`M::Box<dyn Send>`: an auto trait alone, right in a Box, as for the last segment of a path)
            GArg::Ty(t) if seg.name == "Box" && auto_dyn(t).is_some() => parts.push(TySyn::Dyn(auto_dyn(t).unwrap())),
            GArg::Ty(t) => parts.push(match bool_arg(t) {
                Some(b) => b,
                None => tysyn(t, self_ty)?,
            }),
            GArg::Lifetime(_) => {}
            // (`M::S<3>`: a module-qualified type with a const argument also comes here; `Program` resolves it as `S<3>`)
            GArg::Const(e) => parts.push(const_arg_syn(e)?),
            GArg::Bind(..) => return Err("unsupported argument of a generic associated type".to_string()),
        }
    }
    Ok(TySyn::Assoc(Box::new(TySyn::Tuple(parts)), format!("{GAT_PREFIX}{}", seg.name)))
}

fn no_args(s: &crate::parse::Seg) -> bool {
    s.args.is_empty() && s.fn_args.is_none()
}

fn path_ty(p: &Path, self_ty: Option<&TySyn>) -> R<TySyn> {
    let n = p.segs.len();
    // `Self::Item` / `I::Item` (a two-segment path starting with an uppercase letter)
    // (`Self::F<'a>`: a generic associated type with lifetime arguments only is the same type for every lifetime)
    let lifetimes_only = |s: &crate::parse::Seg| s.fn_args.is_none() && s.args.iter().all(|a| matches!(a, GArg::Lifetime(_)));
    if !p.global && n == 2 && no_args(&p.segs[0]) && lifetimes_only(&p.segs[1]) && p.segs[0].name.starts_with(|c: char| c.is_ascii_uppercase()) {
        let b = p.segs[0].name.clone();
        let base = if b == "Self" { self_ty.cloned().unwrap_or(TySyn::Named("Self".into())) } else { TySyn::Named(b) };
        return Ok(TySyn::Assoc(Box::new(base), p.segs[1].name.clone()));
    }
    // (`P::Pointer<String>` / `Self::Output<O>`: a generic associated type applied to type arguments)
    if !p.global && n == 2 && no_args(&p.segs[0]) && p.segs[1].fn_args.is_none() && p.segs[0].name.starts_with(|c: char| c.is_ascii_uppercase()) {
        let b = p.segs[0].name.clone();
        let base = if b == "Self" { self_ty.cloned().unwrap_or(TySyn::Named("Self".into())) } else { TySyn::Named(b) };
        return gat_app(base, &p.segs[1], self_ty);
    }
    let last = p.segs.last().ok_or("empty path")?;
    // `io::Error` / `io::Result<T>` (the prelude's IoError)
    if n >= 2 && p.segs[n - 2].name == "io" && (last.name == "Result" || last.name == "Error") {
        let io_err = TySyn::Named("IoError".into());
        if last.name == "Error" {
            return Ok(io_err);
        }
        let Some(GArg::Ty(a)) = last.args.first() else {
            return Err("`io::Result` needs a type argument".to_string());
        };
        return Ok(TySyn::App("Result".into(), vec![tysyn(a, self_ty)?, io_err]));
    }
    // `thread::Result<T>` (a join's result: Err is the panic's payload, the prelude's __ThreadPanic)
    if n >= 2 && p.segs[n - 2].name == "thread" && last.name == "Result" {
        let Some(GArg::Ty(a)) = last.args.first() else {
            return Err("`thread::Result` needs a type argument".to_string());
        };
        return Ok(TySyn::App("Result".into(), vec![tysyn(a, self_ty)?, TySyn::Named("__ThreadPanic".into())]));
    }
    // (`Result<T>` with one argument: `std::io::Result` imported by name (`use std::io::{self, Result};`), which is
    // the only way a one-argument `Result` names a type)
    if n == 1 && last.name == "Result" && last.args.len() == 1 {
        if let Some(GArg::Ty(a)) = last.args.first() {
            return Ok(TySyn::App("Result".into(), vec![tysyn(a, self_ty)?, TySyn::Named("IoError".into())]));
        }
    }
    // `path::Display<'a>` (what `Path::display` returns): a Path is a str here, and its display the &str
    if n >= 2 && p.segs[n - 2].name == "path" && last.name == "Display" {
        return Ok(TySyn::Ref(false, Box::new(TySyn::Str)));
    }
    // `fmt::Result` / `fmt::Error`
    if n >= 2 && p.segs[n - 2].name == "fmt" && (last.name == "Result" || last.name == "Error") {
        let err = TySyn::Named("__FmtError".into());
        if last.name == "Error" {
            return Ok(err);
        }
        return Ok(TySyn::App("Result".into(), vec![TySyn::Prim(Ty::Unit), err]));
    }
    if last.fn_args.is_some() {
        return Err("unsupported type path".to_string());
    }
    if !last.args.is_empty() && (n == 1 || p.segs[n - 2].name == "primitive") && (Ty::prim(&last.name).is_some() || last.name == "str") {
        return Err(builtin_args_err(last));
    }
    let s = last.name.clone();
    let mut generic_args = Vec::new();
    for a in &last.args {
        match a {
            // (`Box<dyn Send>`: an auto trait alone, right in a Box)
            GArg::Ty(t) if s == "Box" && auto_dyn(t).is_some() => generic_args.push(TySyn::Dyn(auto_dyn(t).unwrap())),
            GArg::Ty(t) if bool_arg(t).is_some() => generic_args.push(bool_arg(t).unwrap()),
            GArg::Ty(t) => {
                // (`Buf<N>`: a lone name in type position is read as a type (as syn does))
                generic_args.push(tysyn(t, self_ty)?)
            }
            GArg::Lifetime(_) => {}
            GArg::Const(e) => generic_args.push(const_arg_syn(e)?),
            GArg::Bind(..) => return Err("unsupported generic argument".to_string()),
        }
    }
    if !generic_args.is_empty() {
        return Ok(TySyn::App(s, generic_args));
    }
    Ok(match Ty::prim(&s) {
        Some(t) => TySyn::Prim(t),
        None if s == "Self" => match self_ty {
            Some(t) => t.clone(),
            None => return Err("`Self` is only available in impls".to_string()),
        },
        None if s == "str" => TySyn::Str,
        None => match ffi_alias(&s) {
            Some(t) => TySyn::Prim(t),
            None => TySyn::Named(s),
        },
    })
}

/// The type name an associated const's path starts with: `Self` in an impl is its self type (`impl Sealed for u8`:
/// `Self::MAX_STR_LEN` is `u8::MAX_STR_LEN`)
fn self_name(a: &str, self_ty: Option<&TySyn>) -> String {
    match (a, self_ty) {
        ("Self", Some(TySyn::Named(n))) => n.clone(),
        ("Self", Some(TySyn::Prim(t))) => {
            let names = ["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "bool", "char", "f32", "f64"];
            match names.iter().find(|n| Ty::prim(n) == Some(*t)) {
                Some(n) => n.to_string(),
                None => a.to_string(),
            }
        }
        _ => a.to_string(),
    }
}

/// E0109: `u32<i32>` / `str<'static>`, generic arguments on a builtin type
#[cold]
fn builtin_args_err(last: &crate::parse::Seg) -> String {
    let kind = if last.args.iter().any(|a| matches!(a, GArg::Ty(_) | GArg::Const(_))) { "type" } else { "lifetime" };
    format!("{kind} arguments are not allowed on builtin type `{}` (E0109)", last.name)
}
