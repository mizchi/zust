//! 型の構文 (自前の構文木の `parse::Ty`) を `TySyn` にする: front.rs の `parse_tysyn` (syn 版) の移植。
//! tests/ast_types.rs が全ファイルの型で syn 版と同じ結果になることを確かめる。
//! 外部クレートに依存しない (src/parse.rs と src/types.rs だけ) ので zustc 自身でもコンパイルできる。

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
    // (`_` を除き、接頭辞で基数を決めて、数字の続く分だけ読む: 接尾辞 `usize` などは捨てる)
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

/// const ジェネリクスの引数 (`Buf<4>` / `Buf<N>` / `Buf<{ 4 }>`): 長さを持つ配列型 `[(); n]`
pub fn const_arg_syn(e: &Expr) -> R<TySyn> {
    match e {
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
        // (整数の定数式 `{ u128::MAX_STR_LEN - 16 }`: 式のテキストを const の名前の代わりに持つ。Program が評価する)
        e => match const_expr_text(e) {
            Some(t) => Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Const(t))),
            None => Err("const arguments must be a literal or a name".to_string()),
        },
    }
}

/// 整数の定数式のテキスト (リテラル・パス・二項演算・単項の `-`・括弧・引数のないメソッド)。それ以外は None
pub(crate) fn const_expr_text(e: &Expr) -> Option<String> {
    Some(match e {
        Expr::Lit(t, _) => t.clone(),
        Expr::Path(None, p) if !p.global && p.segs.iter().all(|s| s.args.is_empty() && s.fn_args.is_none()) => {
            p.segs.iter().map(|s| s.name.as_str()).collect::<Vec<&str>>().join("::")
        }
        Expr::Binary(op, l, r, _) => format!("({} {op} {})", const_expr_text(l)?, const_expr_text(r)?),
        Expr::Unary(op, x, _) if op == "-" => format!("(-{})", const_expr_text(x)?),
        Expr::Paren(x) => const_expr_text(x)?,
        Expr::MethodCall(r, m, g, a, _) if g.is_empty() && a.is_empty() => format!("{}.{m}()", const_expr_text(r)?),
        Expr::Unary(op, x, _) if op == "!" => format!("(!{})", const_expr_text(x)?),
        // (the type of a cast does not change an integer constant's value here)
        Expr::Cast(x, _) => format!("({} as usize)", const_expr_text(x)?),
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

/// 1 つの識別子だけのパス
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
fn auto_dyn(t: &PTy) -> Option<String> {
    let PTy::Dyn(bs) = t else { return None };
    let mut auto = None;
    for b in bs {
        match b {
            Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name == "Send" || s.name == "Sync") => {
                auto.get_or_insert_with(|| p.segs.last().unwrap().name.clone());
            }
            Bound::Lifetime(_) => {}
            _ => return None,
        }
    }
    auto
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
                    // (`dyn Error + Send + Sync`: 自動 trait は読み飛ばす)
                    Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name == "Send" || s.name == "Sync") => {
                        auto.get_or_insert_with(|| p.segs.last().map(|s| s.name.clone()).unwrap_or_default());
                    }
                    Bound::Trait(_, p) if tr.is_none() => tr = Some(p.segs.last().map(|s| s.text.clone()).unwrap_or_default()),
                    Bound::Lifetime(_) => {}
                    _ => return Err("only one trait is supported in `dyn`".to_string()),
                }
            }
            match tr {
                Some(s) => TySyn::Dyn(s),
                // (an auto trait alone is read right behind a pointer only, see auto_dyn)
                None => {
                    let _ = auto;
                    return Err("`dyn` needs a trait".to_string());
                }
            }
        }
        // ライフタイムはコード生成に影響しないので消去する
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
                // 関連定数 `[u8; Buf::CAP]` / `[u8; Self::CAP]`
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
            TySyn::Assoc(Box::new(tysyn(q, self_ty)?), p.segs.last().unwrap().name.clone())
        }
        PTy::Path(None, p) => path_ty(p, self_ty)?,
        _ => return Err("unsupported type".to_string()),
    })
}

fn no_args(s: &crate::parse::Seg) -> bool {
    s.args.is_empty() && s.fn_args.is_none()
}

fn path_ty(p: &Path, self_ty: Option<&TySyn>) -> R<TySyn> {
    let n = p.segs.len();
    // `Self::Item` / `I::Item` (大文字で始まる 2 セグメントのパス)
    // (`Self::F<'a>`: a generic associated type with lifetime arguments only is the same type for every lifetime)
    let lifetimes_only = |s: &crate::parse::Seg| s.fn_args.is_none() && s.args.iter().all(|a| matches!(a, GArg::Lifetime(_)));
    if !p.global && n == 2 && no_args(&p.segs[0]) && lifetimes_only(&p.segs[1]) && p.segs[0].name.starts_with(|c: char| c.is_ascii_uppercase()) {
        let b = p.segs[0].name.clone();
        let base = if b == "Self" { self_ty.cloned().unwrap_or(TySyn::Named("Self".into())) } else { TySyn::Named(b) };
        return Ok(TySyn::Assoc(Box::new(base), p.segs[1].name.clone()));
    }
    let last = p.segs.last().ok_or("empty path")?;
    // `io::Error` / `io::Result<T>` (prelude の IoError)
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
    // (`Result<T>` with one argument: `std::io::Result` imported by name (`use std::io::{self, Result};`), which is
    // the only way a one-argument `Result` names a type)
    if n == 1 && last.name == "Result" && last.args.len() == 1 {
        if let Some(GArg::Ty(a)) = last.args.first() {
            return Ok(TySyn::App("Result".into(), vec![tysyn(a, self_ty)?, TySyn::Named("IoError".into())]));
        }
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
    let s = last.name.clone();
    let mut generic_args = Vec::new();
    for a in &last.args {
        match a {
            // (`Box<dyn Send>`: an auto trait alone, right in a Box)
            GArg::Ty(t) if s == "Box" && auto_dyn(t).is_some() => generic_args.push(TySyn::Dyn(auto_dyn(t).unwrap())),
            GArg::Ty(t) => {
                // (`Buf<N>`: 型の位置の 1 つの名前は型として読まれる (syn と同じ))
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
