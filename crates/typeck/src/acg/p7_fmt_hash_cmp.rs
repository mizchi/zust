use super::*;
use crate::parse::{Expr, Path};
use crate::types::{Ty, intern};
use lit::Lit;

// ---------------------------------------------------------------------------
// Formatting (format! / print! / println! / eprint! / eprintln! / write! / writeln!)
// ---------------------------------------------------------------------------
//
// The format string is split up at compile time, and "string pieces" and "values" are written in order to the prelude's Formatter.
// Values are written with `__exr_display::<T>` / `__exr_debug::<T>` (codegen builds their bodies per type, like drop glue).
// Writing out numbers / strings, padding, and Debug pretty-printing are Rust fns in the prelude.

/// One format spec
#[derive(Clone, Default, PartialEq)]
pub(crate) struct FmtSpec {
    width: usize,
    /// `{:w$}` / `{:1$}`: take the width from an argument (the argument's number)
    width_arg: Option<usize>,
    /// `{:.p$}` / `{:.1$}` / `{:.*}`: take the precision from an argument
    prec_arg: Option<usize>,
    fill: char,
    align: u8,
    flags: u8,
    prec: Option<usize>,
    radix: u8,
}

pub(crate) enum FmtPiece {
    Lit(String),
    Arg {
        /// The argument's number (determined from the position or the name)
        arg: usize,
        debug: bool,
        spec: FmtSpec,
    },
}

/// Splits up a format string. A named `{x}` is one of named (the named arguments), or if there is none, the variable of that name is added to the arguments
pub(crate) fn parse_fmt(
    s: &str,
    n_pos: usize,
    named: &mut Vec<String>,
    span: Sp,
) -> R<Vec<FmtPiece>> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut lit = String::new();
    let mut next = 0;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '{' && cs.get(i + 1) == Some(&'{') {
            lit.push('{');
            i += 2;
            continue;
        }
        if c == '}' && cs.get(i + 1) == Some(&'}') {
            lit.push('}');
            i += 2;
            continue;
        }
        // (an error is `\u{6}index\u{6}msg`: at that char of the string, which the caller places in the literal)
        if c == '}' {
            return err(span, format!("\u{6}{i}\u{6}invalid format string: unmatched `}}` found"));
        }
        if c != '{' {
            lit.push(c);
            i += 1;
            continue;
        }
        // (`{ x}`: no white space before the argument, as rustc reads it; `{ }` and `{x }` are fine)
        let k = (i + 1..cs.len()).find(|&k| !cs[k].is_whitespace()).unwrap_or(cs.len());
        if k > i + 1 && k < cs.len() && !matches!(cs[k], '}' | ':') {
            return err(span, format!("\u{6}{k}\u{6}invalid format string: expected `}}`, found `{}`", cs[k]));
        }
        let Some(end) = cs[i..].iter().position(|&x| x == '}').map(|p| p + i) else {
            return err(span, format!("\u{6}{}\u{6}invalid format string: expected `}}` but string was terminated", cs.len()));
        };
        let inner: String = cs[i + 1..end].iter().collect();
        i = end + 1;
        if !lit.is_empty() {
            out.push(FmtPiece::Lit(std::mem::take(&mut lit)));
        }
        let (a, sp) = match inner.split_once(':') {
            Some((a, s)) => (a.trim().to_string(), s.to_string()),
            None => (inner.trim().to_string(), String::new()),
        };
        let implicit = a.is_empty();
        let mut arg = if a.is_empty() {
            next += 1;
            next - 1
        } else if let Ok(k) = a.parse::<usize>() {
            k
        } else {
            match named.iter().position(|n| *n == a) {
                Some(k) => n_pos + k,
                None => {
                    named.push(a);
                    n_pos + named.len() - 1
                }
            }
        };
        // [[fill]align][+][#][0][width][.prec][type]
        let sc: Vec<char> = sp.chars().collect();
        let mut j = 0;
        let mut spec = FmtSpec {
            fill: ' ',
            ..Default::default()
        };
        let al = |c: char| match c {
            '<' => Some(1),
            '>' => Some(2),
            '^' => Some(3),
            _ => None,
        };
        if sc.len() >= 2 && al(sc[1]).is_some() {
            spec.fill = sc[0];
            spec.align = al(sc[1]).unwrap();
            j = 2;
        } else if !sc.is_empty() && al(sc[0]).is_some() {
            spec.align = al(sc[0]).unwrap();
            j = 1;
        }
        if sc.get(j) == Some(&'+') {
            spec.flags |= 2;
            j += 1;
        }
        if sc.get(j) == Some(&'#') {
            spec.flags |= 4;
            j += 1;
        }
        if sc.get(j) == Some(&'0') {
            spec.flags |= 1;
            j += 1;
        }
        // Width: a number, or `name$` / `number$`
        let ws = j;
        while sc.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
            j += 1;
        }
        let wtok: String = sc[ws..j].iter().collect();
        if sc.get(j) == Some(&'$') {
            j += 1;
            spec.width_arg = Some(match wtok.parse::<usize>() {
                Ok(k) => k,
                Err(_) => match named.iter().position(|n| *n == wtok) {
                    Some(k) => n_pos + k,
                    None => {
                        named.push(wtok);
                        n_pos + named.len() - 1
                    }
                },
            });
        } else {
            // Only the leading digits are the width; the rest is the type spec (`12e` / `x` / `b` etc.)
            j = ws;
            let mut w = 0;
            let mut any = false;
            while let Some(d) = sc.get(j).and_then(|c| c.to_digit(10)) {
                w = w * 10 + d as usize;
                j += 1;
                any = true;
            }
            if any {
                spec.width = w;
            }
        }
        if sc.get(j) == Some(&'.') {
            j += 1;
            let ps = j;
            while sc.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
                j += 1;
            }
            let ptok: String = sc[ps..j].iter().collect();
            if sc.get(ps) == Some(&'*') {
                // `.*`: the next positional argument is the precision, and the one after it is the value
                j = ps + 1;
                spec.prec = Some(0);
                if implicit {
                    spec.prec_arg = Some(arg);
                    arg = next;
                } else {
                    spec.prec_arg = Some(next);
                }
                next += 1;
            } else if sc.get(j) == Some(&'$') {
                j += 1;
                spec.prec = Some(0);
                spec.prec_arg = Some(match ptok.parse::<usize>() {
                    Ok(k) => k,
                    Err(_) => match named.iter().position(|n| *n == ptok) {
                        Some(k) => n_pos + k,
                        None => {
                            named.push(ptok);
                            n_pos + named.len() - 1
                        }
                    },
                });
            } else {
                // Take only the digits; the rest is the type spec
                j = ps;
                let mut p = 0;
                while let Some(d) = sc.get(j).and_then(|c| c.to_digit(10)) {
                    p = p * 10 + d as usize;
                    j += 1;
                }
                spec.prec = Some(p);
            }
        }
        let ty: String = sc[j..].iter().collect();
        let (debug, radix) = match ty.as_str() {
            "" => (false, 0),
            "?" => (true, 0),
            "x" => (false, 1),
            "X" => (false, 2),
            "b" => (false, 3),
            "o" => (false, 4),
            "x?" => (true, 1),
            "X?" => (true, 2),
            "e" => (false, 5),
            "E" => (false, 6),
            "p" => (false, 7),
            t => return err(span, format!("unsupported format specifier `{t}`")),
        };
        spec.radix = radix;
        out.push(FmtPiece::Arg { arg, debug, spec });
    }
    if !lit.is_empty() {
        out.push(FmtPiece::Lit(lit));
    }
    Ok(out)
}

impl Cx<'_> {
    /// A &str constant (the address of the fat pointer)
    pub(crate) fn str_const(&mut self, s: &str) -> Value {
        let n = s.len();
        let p = self.data_addr(s.as_bytes().to_vec());
        self.make_fat(p, n as i64)
    }

    pub(crate) fn formatter_ty(&self) -> R<Ty> {
        let Some(&sid) = self.prog.struct_index.get("Formatter") else {
            return Err("internal: Formatter is missing from the prelude".into());
        };
        Ok(Ty::Struct(sid))
    }

    pub(crate) fn string_ty(&self) -> R<Ty> {
        let Some(&sid) = self.prog.struct_index.get("String") else {
            return Err("internal: String is missing from the prelude".into());
        };
        Ok(Ty::Struct(sid))
    }

    /// Whether it can be written as a value (whether it implements Display for `{}`, Debug for `{:?}`)
    pub(crate) fn fmt_ok(&self, t: Ty, debug: bool) -> bool {
        match t {
            t if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Str) => true,
            Ty::Unit => debug,
            Ty::Ref(_, i) | Ty::Box(i) => self.fmt_ok(*i, debug),
            Ty::Array(e, _) | Ty::Slice(e) => debug && self.fmt_ok(*e, debug),
            Ty::Dyn(s) => dyn_has_fmt(s, debug) || (debug && self.prog.dyn_any_debug(s)),
            Ty::Struct(sid) => {
                let b = self.prog.base_of(sid);
                let tr = if debug { "Debug" } else { "Display" };
                if self.prog.impls.contains(&(b, tr.to_string())) {
                    return true;
                }
                debug && self.prog.is_tuple(b)
                    && self.prog.structs[sid as usize].args.iter().all(|&a| self.fmt_ok(a, true))
            }
            _ => false,
        }
    }

    /// The body of a formatting macro. If dest is Some, write! (writes to that Formatter / String)
    pub(crate) fn fmt_macro(&mut self, p: &Path, g: &str, name: &str) -> R<V> {
        let _ = p;
        let span = SP;
        if let Some(args) = self.macro_args_of(g) {
            return self.fmt_macro_args(args, name);
        }
        let Some(parsed) = crate::ast_closure::macro_args_at(g, self.mbase) else {
            return err(span, format!("`{name}!`: expected comma-separated expressions"));
        };
        // (the re-read arguments are kept for the duration of the fn: to pass them to code that uses tables looked up by address)
        let Expr::Tuple(kept) = self.keep_expr(Expr::Tuple(parsed)) else { unreachable!() };
        let args: Vec<&Expr> = kept.iter().collect();
        self.fmt_macro_args(args, name)
    }

    /// The continuation of fmt_macro (the arguments are the parsed expressions)
    pub(crate) fn fmt_macro_args(&mut self, args: Vec<&Expr>, name: &str) -> R<V> {
        let span = SP;
        let mut args = args;
        let dest = if matches!(name, "write" | "writeln") {
            if args.is_empty() {
                return err(span, format!("`{name}!` needs a destination"));
            }
            Some(args.remove(0))
        } else {
            None
        };
        let newline = matches!(name, "println" | "eprintln" | "writeln");
        let fmt_s = match args.first() {
            Some(Expr::Lit(t, _)) if matches!(lit::lit(t), Some(Lit::Str(_))) => match lit::lit(t) {
                Some(Lit::Str(s)) => s,
                _ => unreachable!(),
            },
            None if newline || dest.is_some() => String::new(),
            // (`write!(f, concat!(stringify!($Rng), " {{ ... }}"))`: a format string made by concat! / stringify!)
            Some(Expr::Macro(mp, g, _)) if mp.segs.len() == 1 && matches!(mp.segs[0].name.as_str(), "concat" | "stringify") => {
                crate::acg::p5_loops_macros::lit_macro_text(&mp.segs[0].name, g).or_else(|e| err(SP, e))?
            }
            // (`format_args!(a!())` with no macro `a`: that is the error, before the format string's)
            Some(Expr::Macro(mp, _, _)) if mp.segs.len() == 1 && !crate::lexer::is_std_macro(&mp.segs[0].name) => {
                return err(SP, format!("cannot find macro `{}` in this scope", mp.segs[0].name));
            }
            Some(_) => return err(SP, "format argument must be a string literal"),
            None => return err(span, format!("`{name}!` needs a format string")),
        };
        // (`{r#type}`: a raw identifier is not an argument's name, rustc says so (tests/ui fmt/raw-idents.rs))
        {
            let b = fmt_s.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'{' && b.get(i + 1) == Some(&b'{') {
                    i += 2;
                    continue;
                }
                if b[i] == b'{' && b[i + 1..].starts_with(b"r#") {
                    return err(SP, "invalid format string: raw identifiers are not supported");
                }
                i += 1;
            }
        }
        let fmt_arg = args.first().copied();
        let rest: Vec<&Expr> = args.into_iter().skip(1).collect();
        // Positional arguments and `name = expr`
        let mut pos: Vec<&Expr> = vec![];
        let mut named: Vec<String> = vec![];
        let mut named_e: Vec<&Expr> = vec![];
        for a in rest {
            match a {
                Expr::Assign(left, right) => {
                    let Expr::Path(_, p) = &**left else {
                        return err(SP, "expected `name = value`");
                    };
                    named.push(ident_of(p).map(|i| i.to_string()).unwrap_or_default());
                    named_e.push(&**right);
                }
                e => pos.push(e),
            }
        }
        let mut names = named.clone();
        // (a format string's error at its char in the literal, escapes and lines counted, as rustc points)
        let mut pieces = parse_fmt(&fmt_s, pos.len(), &mut names, span).map_err(|m| {
            match m.strip_prefix('\u{6}').and_then(|r| r.split_once('\u{6}')) {
                Some((k, msg)) => match (fmt_arg, k.parse::<usize>()) {
                    (Some(Expr::Lit(t, at)), Ok(k)) if at.line != 0 => match lit_char_pos(t, *at, k) {
                        Some(p) => self.at(p, msg.to_string()),
                        None => self.at(*at, msg.to_string()),
                    },
                    _ => msg.to_string(),
                },
                None => m,
            }
        })?;
        if newline {
            pieces.push(FmtPiece::Lit("\n".into()));
        }
        // A `{x}` in the format string (a name not among the arguments) is the variable of that name. Its position is the format string's (rustc reports at `{x}`)
        let lit_pos = match fmt_arg {
            Some(Expr::Lit(_, p)) => *p,
            _ => SP,
        };
        for n in &names[named.len()..] {
            let mut pe = path_expr(n);
            if let Expr::Path(_, p) = &mut pe {
                p.pos = lit_pos;
            }
            named_e.push(self.keep_expr(pe));
        }
        let all: Vec<&Expr> = pos.into_iter().chain(named_e).collect();
        // The Formatter to write to
        let fty = self.formatter_ty().or_else(|e| err(span, e))?;
        let fref = Ty::Ref(true, intern(fty));
        let sty = self.string_ty().or_else(|e| err(span, e))?;
        let sref = Ty::Ref(true, intern(sty));
        let mut owned_string = None;
        let mut writer: Option<(Value, Ty, u32)> = None;
        let f = match &dest {
            Some(d) => {
                let (v, t) = self.read_no_move(d)?;
                let v = v.ok_or("expected a destination")?;
                let mut t = t;
                let mut v = v;
                while let Ty::Ref(_, i) = t {
                    if matches!(i, Ty::Struct(_)) {
                        t = *i;
                        break;
                    }
                    v = self.b.ins().load(PTR, mf(), v, 0);
                    t = *i;
                }
                if t == fty {
                    // (a program's `write!(f, "{}", x)` into a Formatter formats its arguments with their own specs, as
                    // std's `f.write_fmt(args)` does with a Formatter of its own: `{:>5}` of a type whose Display
                    // writes `write!(f, "{}", self.0)` is not padded, nor is a LowerHex impl's `{}` in hex. A write! in
                    // the prelude stands for std's `self.0.fmt(f)`, which passes f's spec on)
                    let own = !self.prog.item_of.get(self.root as usize).copied().flatten().is_some_and(|i| i >= self.prog.prelude_from);
                    if own && pieces.iter().any(|p| matches!(p, FmtPiece::Arg { .. })) && self.prog.index.contains_key("__exr_sub_formatter") {
                        self.call_named("__exr_sub_formatter", vec![Ty::Unit], &[(v, fref)], span)?.0.unwrap()
                    } else {
                        v
                    }
                } else if t == sty {
                    self.call_named("__exr_formatter", vec![Ty::Unit], &[(v, sref)], span)?.0.unwrap()
                } else if let Some(id) = match t {
                    Ty::Struct(sid) => self.prog.method(sid, "write_str"),
                    _ => None,
                } {
                    // A destination that has `write_str` (Stdout / BufWriter / a type implementing fmt::Write):
                    // write to a String first, then pass it
                    writer = Some((v, t, id));
                    let (s, _) = self.call_named("String::new", vec![], &[], span)?;
                    let s = s.unwrap();
                    owned_string = Some(s);
                    self.call_named("__exr_formatter", vec![Ty::Unit], &[(s, sref)], span)?.0.unwrap()
                } else {
                    return err(
                        SP,
                        format!("cannot write into `{}`", self.prog.tn(t)),
                    );
                }
            }
            None => {
                // (the String of format! reserves the length of the fixed parts + 8 bytes per argument up front: saves the realloc on every growth)
                let (s, _) = if self.prog.index.contains_key("__exr_string_cap") {
                    let lits: usize = pieces.iter().map(|p| match p {
                        FmtPiece::Lit(l) => l.len(),
                        _ => 8,
                    }).sum();
                    let n = self.b.ins().iconst(types::I64, lits.max(8) as i64);
                    self.call_named("__exr_string_cap", vec![Ty::Unit], &[(n, Ty::Usize)], span)?
                } else {
                    self.call_named("String::new", vec![], &[], span)?
                };
                let s = s.unwrap();
                owned_string = Some(s);
                self.call_named("__exr_formatter", vec![Ty::Unit], &[(s, sref)], span)?.0.unwrap()
            }
        };
        // Each argument is evaluated once (without moving) and made into an address
        let mut vals: Vec<(Value, Ty, Sp)> = vec![];
        // Arguments used for the width (`{:w$}`) are usize
        let width_args: Vec<usize> = pieces
            .iter()
            .flat_map(|p| match p {
                FmtPiece::Arg { spec, .. } => vec![spec.width_arg, spec.prec_arg],
                _ => vec![],
            })
            .flatten()
            .collect();
        self.in_fmt_args += 1;
        let r = self.fmt_eval_args(&all, &width_args, &mut vals);
        self.in_fmt_args -= 1;
        r?;
        if vals.len() < all.len() {
            return Ok((None, Ty::Never));
        }
        let mut cur = FmtSpec {
            fill: ' ',
            ..Default::default()
        };
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        for pc in &pieces {
            match pc {
                FmtPiece::Lit(s) => {
                    let l = self.str_const(s);
                    self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (l, str_ref)], span)?;
                }
                FmtPiece::Arg { arg, debug, spec } => {
                    let Some(&(v, t, asp)) = vals.get(*arg) else {
                        return err(span, format!("format string refers to argument {arg}, but there are {}", vals.len()));
                    };
                    // (`{:?}` of a raw pointer, or of a reference to one: the address in hex)
                    let mut pt = t;
                    while let Ty::Ref(_, i) = pt {
                        pt = *i;
                    }
                    // (a radix or exponent spec of std's NonZero types: their number's, which is their one field, as
                    // std's impls delegate to it: `{:x}` of `NonZeroU8::new(255)` is `ff`)
                    let (v, t, pt) = match self.nonzero_inner(pt) {
                        Some(it) if (1..=6).contains(&spec.radix) => {
                            let (mut pv, mut rt) = (v, t);
                            while let Ty::Ref(_, i) = rt {
                                pv = self.b.ins().load(PTR, mf(), pv, 0);
                                rt = *i;
                            }
                            (pv, it, it)
                        }
                        _ => (v, t, pt),
                    };
                    let ptr_debug = *debug && matches!(pt, Ty::Ptr(_, i) if !i.is_fat()) && self.prog.index.contains_key("__exr_fmt_ptr");
                    // (`{:x}` / `{:X}` / `{:b}` / `{:o}` / `{:e}` / `{:E}` of a value that is no number (a float's only in
                    // `{:e}` / `{:E}`): its fmt trait's impl, the prelude's LowerHex and so on (`__p_LowerHex` where the
                    // program defines a LowerHex of its own))
                    // (`{:p}` of a reference / raw pointer / Box: its address; of one to a slice / str, the address and
                    // length; to a `dyn`, the address and the vtable's; of another type, its Pointer impl below)
                    let slice_ptr = matches!(t, Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) if matches!(*i, Ty::Slice(_) | Ty::Str));
                    let dyn_ptr = matches!(t, Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) if matches!(*i, Ty::Dyn(_)));
                    let ptr_fmt = spec.radix == 7
                        && matches!(t, Ty::Ref(..) | Ty::Ptr(..) | Ty::Box(_))
                        && (!t.is_fat()
                            || slice_ptr && self.prog.index.contains_key("__exr_fmt_fat_ptr")
                            || dyn_ptr && self.prog.index.contains_key("__exr_fmt_dyn_ptr"))
                        && self.prog.index.contains_key("__exr_fmt_ptr");
                    let radix_tr = if !*debug && spec.radix != 0 && !ptr_fmt && (spec.radix == 7 || !(pt.is_int() || (pt.is_float() && spec.radix >= 5))) {
                        let tr = match spec.radix {
                            1 => "LowerHex",
                            2 => "UpperHex",
                            3 => "Binary",
                            4 => "Octal",
                            5 => "LowerExp",
                            6 => "UpperExp",
                            _ => "Pointer",
                        };
                        let shadowed = format!("__p_{tr}");
                        let tn = if self.prog.traits.contains_key(&shadowed) { shadowed } else { tr.to_string() };
                        let h = format!("__exr_fmt_{}", tr.to_ascii_lowercase());
                        if !self.prog.index.contains_key(&h) || !self.prog.implements(pt, &tn) {
                            return err(asp, format!("the trait bound `{}: {tr}` is not satisfied (E0277)", self.prog.tn(pt)));
                        }
                        Some(h)
                    } else {
                        None
                    };
                    if radix_tr.is_none() && !ptr_debug && !ptr_fmt && !self.fmt_ok(t, *debug) {
                        return err(
                            asp,
                            format!(
                                "`{}` doesn't implement `{}`",
                                self.prog.tn(t),
                                if *debug { "Debug" } else { "Display" }
                            ),
                        );
                    }
                    if *spec != cur || spec.width_arg.is_some() || spec.prec_arg.is_some() {
                        let c = |this: &mut Self, ty: Type, x: i64| this.b.ins().iconst(ty, x);
                        let w = match spec.width_arg {
                            Some(k) => {
                                let Some(&(wv, wt, wsp)) = vals.get(k) else {
                                    return err(span, "missing width argument");
                                };
                                self.same(wsp, wt, Ty::Usize)?;
                                self.b.ins().load(types::I64, mf(), wv, 0)
                            }
                            None => c(self, types::I64, spec.width as i64),
                        };
                        let fill = c(self, types::I32, spec.fill as i64);
                        let al = c(self, types::I8, spec.align as i64);
                        let fl = c(self, types::I8, spec.flags as i64);
                        let pr = match spec.prec_arg {
                            Some(k) => {
                                let Some(&(pv, pt, psp)) = vals.get(k) else {
                                    return err(span, "missing precision argument");
                                };
                                self.same(psp, pt, Ty::Usize)?;
                                self.b.ins().load(types::I64, mf(), pv, 0)
                            }
                            None => c(self, types::I64, spec.prec.unwrap_or(0) as i64),
                        };
                        let hp = c(self, types::I8, spec.prec.is_some() as i64);
                        let rx = c(self, types::I8, spec.radix as i64);
                        self.call_named(
                            "__exr_fmt_spec",
                            vec![Ty::Unit],
                            &[
                                (f, fref),
                                (w, Ty::Usize),
                                (fill, Ty::Char),
                                (al, Ty::U8),
                                (fl, Ty::U8),
                                (pr, Ty::Usize),
                                (hp, Ty::Bool),
                                (rx, Ty::U8),
                            ],
                            span,
                        )?;
                        cur = spec.clone();
                    }
                    if ptr_fmt {
                        let a = self.b.ins().load(PTR, mf(), v, 0);
                        if dyn_ptr {
                            let vt = self.b.ins().load(PTR, mf(), v, 8);
                            self.call_named("__exr_fmt_dyn_ptr", vec![Ty::Unit], &[(f, fref), (a, Ty::Usize), (vt, Ty::Usize)], span)?;
                        } else if t.is_fat() {
                            let n = self.b.ins().load(types::I64, mf(), v, 8);
                            self.call_named("__exr_fmt_fat_ptr", vec![Ty::Unit], &[(f, fref), (a, Ty::Usize), (n, Ty::Usize)], span)?;
                        } else {
                            self.call_named("__exr_fmt_ptr", vec![Ty::Unit], &[(f, fref), (a, Ty::Usize)], span)?;
                        }
                        continue;
                    }
                    if ptr_debug {
                        let (mut pv, mut rt) = (v, t);
                        while let Ty::Ref(_, i) = rt {
                            pv = self.b.ins().load(PTR, mf(), pv, 0);
                            rt = *i;
                        }
                        let a = self.b.ins().load(PTR, mf(), pv, 0);
                        self.call_named("__exr_fmt_ptr", vec![Ty::Unit], &[(f, fref), (a, Ty::Usize)], span)?;
                        continue;
                    }
                    // (`{:x}` / `{:X}` / `{:b}` / `{:o}` of a signed number, `{:x?}` too: its bits, written as the
                    // unsigned type of its width, as std's: `{:x}` of `-1i8` is `ff`, not `-1`)
                    if (1..=4).contains(&spec.radix) && pt.is_int() && pt.signed() {
                        let ut = match pt {
                            Ty::I8 => Ty::U8,
                            Ty::I16 => Ty::U16,
                            Ty::I32 => Ty::U32,
                            Ty::I64 => Ty::U64,
                            Ty::I128 => Ty::U128,
                            _ => Ty::Usize,
                        };
                        let (mut pv, mut rt) = (v, t);
                        while let Ty::Ref(_, i) = rt {
                            pv = self.b.ins().load(PTR, mf(), pv, 0);
                            rt = *i;
                        }
                        let g = if *debug { "__exr_debug" } else { "__exr_display" };
                        self.call_named(g, vec![ut], &[(pv, Ty::Ref(false, intern(ut))), (f, fref)], span)?;
                        continue;
                    }
                    if let Some(h) = &radix_tr {
                        let (mut pv, mut rt) = (v, t);
                        while let Ty::Ref(_, i) = rt {
                            pv = self.b.ins().load(PTR, mf(), pv, 0);
                            rt = *i;
                        }
                        self.call_named(h, vec![rt], &[(pv, Ty::Ref(false, intern(rt))), (f, fref)], span)?;
                        continue;
                    }
                    // `{:e}` / `{:E}`: a number in exponent notation (the prelude's __exr_fmt_exp)
                    if spec.radix >= 5 && (t.is_int() || t.is_float()) {
                        let ct = clif_ty(t).unwrap();
                        let x = self.b.ins().load(ct, mf(), v, 0);
                        let up = self.b.ins().iconst(types::I8, (spec.radix == 6) as i64);
                        if t.is_float() {
                            let x = if t == Ty::F32 { self.b.ins().fpromote(types::F64, x) } else { x };
                            let is32 = self.b.ins().iconst(types::I8, (t == Ty::F32) as i64);
                            self.call_named("__exr_fmt_exp", vec![Ty::Unit], &[(f, fref), (x, Ty::F64), (is32, Ty::Bool), (up, Ty::Bool)], span)?;
                        } else {
                            // (pass the absolute value as a u128)
                            let w = if ct == types::I128 {
                                x
                            } else if t.signed() {
                                self.b.ins().sextend(types::I128, x)
                            } else {
                                self.b.ins().uextend(types::I128, x)
                            };
                            let z = self.int_const(Ty::I128, 0);
                            let neg = if t.signed() { self.b.ins().icmp(IntCC::SignedLessThan, w, z) } else { self.b.ins().iconst(types::I8, 0) };
                            let nw = self.b.ins().ineg(w);
                            let mag = self.b.ins().select(neg, nw, w);
                            self.call_named("__exr_fmt_exp_int", vec![Ty::Unit], &[(f, fref), (mag, Ty::U128), (neg, Ty::Bool), (up, Ty::Bool)], span)?;
                        }
                        continue;
                    }
                    // (`&dyn Display` / `Box<dyn Debug>`: the trait object's own `fmt`)
                    let (mut dv, mut dt) = (v, t);
                    // (dv is the address of a value of type dt; a fat pointer's value is its (data, vtable) pair's
                    // address, so `&Box<dyn ..>` is loaded once to reach the pair)
                    while let Ty::Ref(_, i @ (Ty::Ref(..) | Ty::Box(_))) = dt {
                        dv = self.b.ins().load(PTR, mf(), dv, 0);
                        dt = *i;
                    }
                    if let Ty::Ref(_, Ty::Dyn(s)) | Ty::Box(Ty::Dyn(s)) = dt {
                        if *debug && self.prog.dyn_any_debug(s) {
                            let l = self.str_const("Any { .. }");
                            self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (l, str_ref)], span)?;
                            continue;
                        }
                        if dyn_has_fmt(s, *debug) {
                            self.scopes.push(vec![]);
                            // (seen as `&dyn ..`: a Box is not dropped here)
                            self.bind("__exr_dv".into(), Ty::Ref(false, intern(Ty::Dyn(s))), Some(dv));
                            self.bind("__exr_df".into(), fref, Some(f));
                            let e = crate::parse::parse_expr("{ let _ = __exr_dv.fmt(__exr_df); }")?;
                            let e = self.keep_expr(e);
                            let r = self.expr(e, None);
                            self.scopes.pop();
                            r?;
                            continue;
                        }
                    }
                    let g = if *debug { "__exr_debug" } else { "__exr_display" };
                    self.call_named(g, vec![t], &[(v, Ty::Ref(false, intern(t))), (f, fref)], span)?;
                }
            }
        }
        if let Some((wv, wt, id)) = writer {
            let s = owned_string.unwrap();
            let str_ref = Ty::Ref(false, intern(Ty::Str));
            let (sv, _) = self.call_named("String::as_str", vec![], &[(s, Ty::Ref(false, intern(sty)))], span)?;
            let r = self.call_fn_vals(id, &[(wv, Ty::Ref(true, intern(wt))), (sv.unwrap(), str_ref)], span)?;
            self.own_temp_at(s, sty);
            return Ok(r);
        }
        match name {
            "format" => Ok((owned_string, sty)),
            "print" | "println" | "eprint" | "eprintln" => {
                let s = owned_string.unwrap();
                let pf = if name.starts_with('e') { "__exr_eprint" } else { "__exr_print" };
                self.call_named(pf, vec![Ty::Unit], &[(s, Ty::Ref(false, intern(sty)))], span)?;
                self.own_temp_at(s, sty);
                Ok((None, Ty::Unit))
            }
            _ => self.call_named("__exr_fmt_ok", vec![Ty::Unit], &[], span),
        }
    }

    /// Evaluates each argument of a format macro once and makes it an address (stops there if one diverges: vals then comes up short)
    pub(crate) fn fmt_eval_args(
        &mut self,
        all: &[&Expr],
        width_args: &[usize],
        vals: &mut Vec<(Value, Ty, Sp)>,
    ) -> R<()> {
        for (ai, a) in all.iter().enumerate() {
            let (v, t) = if is_place_expr(a) {
                match self.place(a)? {
                    Place::Mem(p, o, t) => (self.addr_plus(p, o), t),
                    Place::Var(var, t) => {
                        let x = self.b.use_var(var);
                        let ss = self.slot(t);
                        self.b.ins().stack_store(x, ss, 0);
                        (self.b.ins().stack_addr(PTR, ss, 0), t)
                    }
                    Place::Zst(t) => (self.b.ins().iconst(PTR, 0), t),
                }
            } else {
                let exp = width_args.contains(&ai).then_some(Ty::Usize);
                let (v, t) = self.expr(a, exp)?;
                if t == Ty::Never {
                    return Ok(());
                }
                let v = match v {
                    Some(v) if t.is_agg() => {
                        self.own_temp_at(v, t);
                        v
                    }
                    Some(v) => {
                        let ss = self.slot(t);
                        self.b.ins().stack_store(v, ss, 0);
                        let a = self.b.ins().stack_addr(PTR, ss, 0);
                        if self.needs_drop(t) {
                            self.own_temp_at(a, t);
                        }
                        a
                    }
                    None => self.b.ins().iconst(PTR, 0),
                };
                (v, t)
            };
            vals.push((v, t, SP));
        }
        Ok(())
    }

    /// `x.to_string()` (a type implementing Display): the same as `format!("{}", x)`
    pub(crate) fn to_string_value(&mut self, recv: &Expr, span: Sp) -> R<V> {
        // (build the arguments of `format!("{}", recv)`: recv is passed as is (so that the type table can be looked up))
        let _ = span;
        let fmt = self.keep_expr(Expr::Lit("\"{}\"".to_string(), crate::parse::Pos::default()));
        self.fmt_macro_args(vec![fmt, recv], "format")
    }

    /// The body of `__exr_display::<T>` / `__exr_debug::<T>`
    pub(crate) fn fmt_glue_body(&mut self, sig: &FnSig, debug: bool) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let ps = self.b.func_params().to_vec();
        let (x, f) = (ps[0], ps[1]);
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad fmt glue signature".into());
        };
        let t = **t;
        let fty = self.formatter_ty()?;
        let fref = Ty::Ref(true, intern(fty));
        let span = SP;
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        let me = if debug { "__exr_debug" } else { "__exr_display" };
        match t {
            t if t.bits() == 128 => {
                let v = self.b.ins().load(types::I128, mf(), x, 0);
                let (mag, neg) = if t.signed() {
                    let z = self.int_const(t, 0);
                    let neg = self.b.ins().icmp(IntCC::SignedLessThan, v, z);
                    let n = self.b.ins().ineg(v);
                    (self.b.ins().select(neg, n, v), neg)
                } else {
                    (v, self.b.ins().iconst(types::I8, 0))
                };
                self.call_named("__exr_fmt_int128", vec![Ty::Unit], &[(f, fref), (mag, Ty::U128), (neg, Ty::Bool)], span)?;
            }
            t if t.is_int() => {
                let ct = clif_ty(t).unwrap();
                let v = self.b.ins().load(ct, mf(), x, 0);
                let v64 = if ct == types::I64 {
                    v
                } else if t.signed() {
                    self.b.ins().sextend(types::I64, v)
                } else {
                    self.b.ins().uextend(types::I64, v)
                };
                let (mag, neg) = if t.signed() {
                    let neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, v64, 0);
                    let n = self.b.ins().ineg(v64);
                    (self.b.ins().select(neg, n, v64), neg)
                } else {
                    (v64, self.b.ins().iconst(types::I8, 0))
                };
                self.call_named("__exr_fmt_int", vec![Ty::Unit], &[(f, fref), (mag, Ty::U64), (neg, Ty::Bool)], span)?;
            }
            t if t.is_float() => {
                let v = self.b.ins().load(clif_ty(t).unwrap(), mf(), x, 0);
                let v = if t == Ty::F32 { self.b.ins().fpromote(types::F64, v) } else { v };
                let is32 = self.b.ins().iconst(types::I8, (t == Ty::F32) as i64);
                let dbg = self.b.ins().iconst(types::I8, debug as i64);
                self.call_named("__exr_fmt_float", vec![Ty::Unit], &[(f, fref), (v, Ty::F64), (is32, Ty::Bool), (dbg, Ty::Bool)], span)?;
            }
            Ty::Bool => {
                let v = self.b.ins().load(types::I8, mf(), x, 0);
                let h = if debug { "__exr_fmt_bool" } else { "__exr_fmt_bool" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (v, Ty::Bool)], span)?;
            }
            Ty::Char => {
                let v = self.b.ins().load(types::I32, mf(), x, 0);
                let h = if debug { "__exr_fmt_char_debug" } else { "__exr_fmt_char" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (v, Ty::Char)], span)?;
            }
            Ty::Unit => {
                let l = self.str_const("()");
                self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (l, str_ref)], span)?;
            }
            // x: &str (the address of the fat pointer)
            Ty::Str => {
                let h = if debug { "__exr_fmt_str_debug" } else { "__exr_fmt_str" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (x, str_ref)], span)?;
            }
            Ty::Ref(_, Ty::Str) => {
                let h = if debug { "__exr_fmt_str_debug" } else { "__exr_fmt_str" };
                self.call_named(h, vec![Ty::Unit], &[(f, fref), (x, str_ref)], span)?;
            }
            Ty::Ref(_, i) | Ty::Box(i) if !t.is_fat() => {
                let p = self.b.ins().load(PTR, mf(), x, 0);
                self.call_named(me, vec![*i], &[(p, Ty::Ref(false, intern(*i))), (f, fref)], span)?;
            }
            // (a `dyn Any` inside a value: `Err(Box<dyn Any + Send>)`, std's `Any { .. }`)
            Ty::Ref(_, Ty::Dyn(s)) | Ty::Box(Ty::Dyn(s)) if debug && self.prog.dyn_any_debug(s) => {
                let l = self.str_const("Any { .. }");
                self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (l, str_ref)], span)?;
            }
            // &[T] / &mut [T]: the sequence of elements
            Ty::Ref(_, Ty::Slice(e)) | Ty::Box(Ty::Slice(e)) => {
                let p = self.b.ins().load(PTR, mf(), x, 0);
                let n = self.b.ins().load(types::I64, mf(), x, 8);
                self.fmt_list(f, p, n, **e, me)?;
            }
            Ty::Array(e, n) => {
                let n = self.b.ins().iconst(types::I64, n as i64);
                self.fmt_list(f, x, n, *e, me)?;
            }
            Ty::Struct(sid) => {
                let b = self.prog.base_of(sid);
                let tr = if debug { "Debug" } else { "Display" };
                let bname = self.prog.structs[b as usize].name.clone();
                let mname = format!("{bname}::<{tr}>::fmt");
                if let Some(&m) = self.prog.index.get(&mname) {
                    // Hand-written (and prelude) impls
                    let ms = &self.prog.sigs[m as usize];
                    let mut fid = m;
                    if ms.is_generic() {
                        let mut sol = vec![None; ms.generics.len()];
                        if let Some(st) = ms.self_ty {
                            self.prog.match_ty(st, t, &mut sol);
                        }
                        let recv = [Ty::Ref(false, intern(t)), fref];
                        fid = self.resolve_call(m, &recv, sol, None, span)?.0;
                    }
                    let fs = self.insts.lock().unwrap().sig(self.prog, fid).clone();
                    let fr = self.fref(fid, &fs);
                    let mut cv = vec![];
                    if is_sret(&fs) {
                        cv.push(self.temp(fs.ret));
                    }
                    cv.push(x);
                    cv.push(f);
                    self.b.ins().call(fr, &cv);
                } else if debug {
                    self.fmt_derived(sid, x, f)?;
                } else {
                    return Err(format!("`{bname}` doesn't implement `Display`"));
                }
            }
            t => return Err(format!("`{}` cannot be formatted", self.prog.tn(t))),
        }
        self.b.ins().return_(&[]);
        Ok(())
    }

    /// `[a, b, ..]`
    pub(crate) fn fmt_list(&mut self, f: Value, p: Value, n: Value, e: Ty, me: &str) -> R<()> {
        let fty = self.formatter_ty()?;
        let fref = Ty::Ref(true, intern(fty));
        let span = SP;
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        let empty = self.str_const("");
        let kind = self.b.ins().iconst(types::I8, 2);
        self.call_named("__exr_dbg_begin", vec![Ty::Unit], &[(f, fref), (empty, str_ref), (kind, Ty::U8)], span)?;
        let (es, _) = self.prog.size_align(e);
        let i = self.b.declare_var(types::I64);
        let z = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(i, z);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(head);
        let iv = self.b.use_var(i);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, n);
        self.b.ins().brif(more, body, &[], done, &[]);
        self.b.switch_to_block(body);
        let iv = self.b.use_var(i);
        let empty = self.str_const("");
        self.call_named("__exr_dbg_item", vec![Ty::Unit], &[(f, fref), (iv, Ty::Usize), (empty, str_ref)], span)?;
        let off = self.b.ins().imul_imm_s(iv, es as i64);
        let a = self.b.ins().iadd(p, off);
        self.call_named(me, vec![e], &[(a, Ty::Ref(false, intern(e))), (f, fref)], span)?;
        self.call_named("__exr_dbg_item_end", vec![Ty::Unit], &[(f, fref)], span)?;
        let iv = self.b.use_var(i);
        let next = self.b.ins().iadd_imm_s(iv, 1);
        self.b.def_var(i, next);
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(done);
        let fl = self.b.ins().iconst(types::I8, 0);
        self.call_named("__exr_dbg_end", vec![Ty::Unit], &[(f, fref), (n, Ty::Usize), (kind, Ty::U8), (fl, Ty::Bool)], span)?;
        Ok(())
    }

    /// `#[derive(Debug)]` structs / enums and tuples
    pub(crate) fn fmt_derived(&mut self, sid: u32, x: Value, f: Value) -> R<()> {
        let fty = self.formatter_ty()?;
        let fref = Ty::Ref(true, intern(fty));
        let span = SP;
        let b = self.prog.base_of(sid);
        let sd = &self.prog.structs[sid as usize];
        let tuple = self.prog.is_tuple(b);
        let bname = self.prog.structs[b as usize].name.clone();
        // (a name inside a module `m__Ty` (a clashing name flattened) is shown with its original name)
        let bname = match bname.rfind("__") {
            Some(i) if i > 0 && !bname.starts_with('_') => bname[i + 2..].to_string(),
            _ => bname,
        };
        // The candidates of (name, fields (name, type, offset), whether named). Per discriminant for an enum
        let shapes: Vec<(Option<i64>, String, Vec<(String, Ty, u32)>, bool)> = if sd.is_enum {
            sd.variants
                .iter()
                .map(|v| {
                    (
                        Some(v.discr),
                        v.name.clone(),
                        v.fields.clone(),
                        v.kind == crate::types::VariantKind::Struct,
                    )
                })
                .collect()
        } else {
            let named = sd.fields.first().is_some_and(|f| f.0.parse::<usize>().is_err());
            vec![(None, if tuple { String::new() } else { bname }, sd.fields.clone(), named)]
        };
        let tag = if sd.is_enum {
            Some(self.b.ins().load(clif_ty(sd.tag).unwrap(), mf(), x, 0))
        } else {
            None
        };
        let done = self.b.create_block();
        for (discr, name, fields, named) in shapes {
            let next = self.b.create_block();
            if let (Some(d), Some(tag)) = (discr, tag) {
                let yes = self.b.create_block();
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                self.b.ins().brif(c, yes, &[], next, &[]);
                self.b.switch_to_block(yes);
            }
            // (a name the compiler renamed prints as it was: `__u_Iter` / `__p_ThreadId`, prelude::unrenamed; a variant
            // the program itself names `__p_Ready` prints as `__p_Ready`)
            let nm = self.str_const(crate::prelude::unrenamed(&name));
            let str_ref = Ty::Ref(false, intern(Ty::Str));
            if fields.is_empty() && !tuple {
                self.call_named("__exr_fmt_lit", vec![Ty::Unit], &[(f, fref), (nm, str_ref)], span)?;
            } else {
                let kind = self.b.ins().iconst(types::I8, if named { 0 } else { 1 });
                self.call_named("__exr_dbg_begin", vec![Ty::Unit], &[(f, fref), (nm, str_ref), (kind, Ty::U8)], span)?;
                for (i, (fname, ft, off)) in fields.iter().enumerate() {
                    let iv = self.b.ins().iconst(types::I64, i as i64);
                    // (a raw field's name without its `r#`, as rustc's derive prints it: `HasKeywordField { struct: 3 }`)
                    let key = self.str_const(if named { fname.strip_prefix("r#").unwrap_or(fname) } else { "" });
                    self.call_named("__exr_dbg_item", vec![Ty::Unit], &[(f, fref), (iv, Ty::Usize), (key, str_ref)], span)?;
                    let a = self.addr_plus(x, *off as i32);
                    self.call_named("__exr_debug", vec![*ft], &[(a, Ty::Ref(false, intern(*ft))), (f, fref)], span)?;
                    self.call_named("__exr_dbg_item_end", vec![Ty::Unit], &[(f, fref)], span)?;
                }
                let n = self.b.ins().iconst(types::I64, fields.len() as i64);
                let tf = self.b.ins().iconst(types::I8, tuple as i64);
                self.call_named("__exr_dbg_end", vec![Ty::Unit], &[(f, fref), (n, Ty::Usize), (kind, Ty::U8), (tf, Ty::Bool)], span)?;
            }
            self.b.ins().jump(done, &[]);
            self.b.switch_to_block(next);
            if discr.is_none() {
                self.b.ins().jump(done, &[]);
                let dead = self.b.create_block();
                self.b.switch_to_block(dead);
            }
        }
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok(())
    }
}

impl Cx<'_> {
    /// The body of `__exr_hash::<T>(x: &T) -> u64` (integers are mixed by multiplication, strings with FNV over their bytes,
    /// struct / enum / tuple / array combine the hashes of their elements)
    pub(crate) fn hash_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let x = self.b.func_params()[0];
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad hash glue signature".into());
        };
        let h = self.hash_at(x, **t)?;
        self.b.ins().return_(&[h]);
        Ok(())
    }

    pub(crate) fn mix(&mut self, h: Value, v: Value) -> Value {
        let a = self.b.ins().imul_imm_s(h, 31);
        let s = self.b.ins().iadd(a, v);
        let m = self.b.ins().imul_imm_s(s, 0x9E3779B97F4A7C15u64 as i64);
        let sh = self.b.ins().ushr_imm_u(m, 29);
        self.b.ins().bxor(m, sh)
    }

    pub(crate) fn hash_call(&mut self, a: Value, t: Ty) -> R<Value> {
        let span = SP;
        let (v, _) = self.call_named("__exr_hash", vec![t], &[(a, Ty::Ref(false, intern(t)))], span)?;
        Ok(v.unwrap())
    }

    pub(crate) fn hash_at(&mut self, x: Value, t: Ty) -> R<Value> {
        let span = SP;
        let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
        Ok(match t {
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => {
                let ct = clif_ty(t).unwrap();
                let v = self.b.ins().load(ct, mf(), x, 0);
                let z = self.b.ins().iconst(types::I64, 0);
                if ct == types::I128 {
                    let (lo, hi) = self.b.ins().isplit(v);
                    let h = self.mix(z, lo);
                    self.mix(h, hi)
                } else {
                    let v = if ct == types::I64 { v } else { self.b.ins().uextend(types::I64, v) };
                    self.mix(z, v)
                }
            }
            Ty::Str => self.call_named("__exr_hash_bytes", vec![Ty::Unit], &[(x, u8s)], span)?.0.unwrap(),
            Ty::Ref(_, Ty::Str) => self.call_named("__exr_hash_bytes", vec![Ty::Unit], &[(x, u8s)], span)?.0.unwrap(),
            Ty::Ref(_, i) | Ty::Box(i) if !t.is_fat() => {
                let p = self.b.ins().load(PTR, mf(), x, 0);
                self.hash_call(p, *i)?
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                let mut h = self.b.ins().iconst(types::I64, 0);
                for i in 0..n {
                    let a = self.addr_plus(x, (i * es) as i32);
                    let v = self.hash_call(a, *e)?;
                    h = self.mix(h, v);
                }
                h
            }
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                let bname = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                // A String by its bytes
                if bname == "String" {
                    let (fat, _) = self
                        .deref_to_slice(x, sid, false)
                        .ok_or("internal: bad String")?;
                    return Ok(self.call_named("__exr_hash_bytes", vec![Ty::Unit], &[(fat, u8s)], span)?.0.unwrap());
                }
                // A Vec by its length and elements (not by the buffer's address)
                if let Some((po, lo, et)) = self.vec_fields(sid) {
                    let (es, _) = self.prog.size_align(et);
                    let p = self.b.ins().load(PTR, mf(), x, po as i32);
                    let n = self.b.ins().load(types::I64, mf(), x, lo as i32);
                    let z = self.b.ins().iconst(types::I64, 0);
                    let h0 = self.mix(z, n);
                    let (i, h) = (self.b.declare_var(types::I64), self.b.declare_var(types::I64));
                    self.b.def_var(i, z);
                    self.b.def_var(h, h0);
                    let (head, body, done) = (self.b.create_block(), self.b.create_block(), self.b.create_block());
                    self.b.ins().jump(head, &[]);
                    self.b.switch_to_block(head);
                    let iv = self.b.use_var(i);
                    let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, n);
                    self.b.ins().brif(more, body, &[], done, &[]);
                    self.b.switch_to_block(body);
                    let off = self.b.ins().imul_imm_s(iv, es as i64);
                    let a = self.b.ins().iadd(p, off);
                    let v = self.hash_call(a, et)?;
                    let hv = self.b.use_var(h);
                    let h2 = self.mix(hv, v);
                    self.b.def_var(h, h2);
                    let i2 = self.b.ins().iadd_imm_s(iv, 1);
                    self.b.def_var(i, i2);
                    self.b.ins().jump(head, &[]);
                    self.b.switch_to_block(done);
                    return Ok(self.b.use_var(h));
                }
                let is_enum = sd.is_enum;
                let tag_ty = sd.tag;
                let fields = sd.fields.clone();
                let variants: Vec<(i64, Vec<(String, Ty, u32)>)> =
                    sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect();
                if is_enum {
                    let tag = self.b.ins().load(clif_ty(tag_ty).unwrap(), mf(), x, 0);
                    let t64 = if clif_ty(tag_ty) == Some(types::I64) {
                        tag
                    } else {
                        self.b.ins().uextend(types::I64, tag)
                    };
                    let z = self.b.ins().iconst(types::I64, 0);
                    let h0 = self.mix(z, t64);
                    let done = self.b.create_block();
                    let res = self.b.append_block_param(done, types::I64);
                    for (d, fs) in variants {
                        let yes = self.b.create_block();
                        let next = self.b.create_block();
                        let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                        self.b.ins().brif(c, yes, &[], next, &[]);
                        self.b.switch_to_block(yes);
                        let mut h = h0;
                        for (_, ft, off) in fs {
                            let a = self.addr_plus(x, off as i32);
                            let v = self.hash_call(a, ft)?;
                            h = self.mix(h, v);
                        }
                        self.b.ins().jump(done, &[h]);
                        self.b.switch_to_block(next);
                    }
                    self.b.ins().jump(done, &[h0]);
                    self.b.switch_to_block(done);
                    res
                } else {
                    let mut h = self.b.ins().iconst(types::I64, 0);
                    for (_, ft, off) in fields {
                        let a = self.addr_plus(x, off as i32);
                        let v = self.hash_call(a, ft)?;
                        h = self.mix(h, v);
                    }
                    h
                }
            }
            // Raw pointers by address (same as std)
            Ty::Ptr(..) => {
                let v = self.b.ins().load(PTR, mf(), x, 0);
                let z = self.b.ins().iconst(types::I64, 0);
                self.mix(z, v)
            }
            t => return Err(format!("`{}` cannot be used as a hash key", self.prog.tn(t))),
        })
    }
}

// --- Ordering comparison (PartialOrd / Ord) ---------------------------------------------

impl Cx<'_> {
    /// Turns a value v (of type t) into an (address, pointee type) that can be passed to the comparison glue.
    /// Thin references are followed to their pointee, and scalar values are put in a temporary slot
    pub(crate) fn ord_addr(&mut self, v: Value, t: Ty) -> (Value, Ty) {
        let (mut v, mut t) = (v, t);
        loop {
            match t {
                Ty::Ref(_, i) if !t.is_fat() => {
                    if matches!(i, Ty::Ref(..)) && !i.is_fat() {
                        v = self.b.ins().load(PTR, mf(), v, 0);
                        t = *i;
                        continue;
                    }
                    return (v, *i);
                }
                t if t.is_agg() || t.is_fat() => return (v, t),
                t => {
                    let a = self.temp(t);
                    self.b.ins().store(mf(), v, a, 0);
                    return (a, t);
                }
            }
        }
    }

    /// `__exr_cmp::<t>(a, b)` (a, b are addresses of values of t) → an i32 of -1 / 0 / 1
    pub(crate) fn cmp_call(&mut self, a: Value, b: Value, t: Ty) -> R<Value> {
        if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Unit | Ty::Ptr(..)) {
            return self.cmp_at(a, b, t);
        }
        // If there is no glue, expand it in place
        if !self.prog.index.contains_key("__exr_cmp") {
            if self.cmp_depth > 16 {
                return Err(format!("comparison of the recursive type `{}` is not supported", self.prog.tn(t)));
            }
            self.cmp_depth += 1;
            let r = self.cmp_at(a, b, t);
            self.cmp_depth -= 1;
            return r;
        }
        let span = SP;
        let r = Ty::Ref(false, intern(t));
        let (v, _) = self.call_named("__exr_cmp", vec![t], &[(a, r), (b, r)], span)?;
        Ok(v.unwrap())
    }

    /// The body of `__exr_cmp::<T>(a: &T, b: &T) -> i32`
    pub(crate) fn cmp_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let a = self.b.func_params()[0];
        let b = self.b.func_params()[1];
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad cmp glue signature".into());
        };
        let c = self.cmp_at(a, b, **t)?;
        self.b.ins().return_(&[c]);
        Ok(())
    }

    /// The body of `__exr_eq::<T>(a: &T, b: &T) -> bool`
    pub(crate) fn eq_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let a = self.b.func_params()[0];
        let b = self.b.func_params()[1];
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad eq glue signature".into());
        };
        let span = SP;
        let c = self.agg_or_scalar_eq(a, b, **t, span)?;
        self.b.ins().return_(&[c]);
        Ok(())
    }

    /// The ordering of 2 scalars (x > y) - (x < y)
    pub(crate) fn cmp_scalar(&mut self, x: Value, y: Value, signed: bool) -> Value {
        let (lt, gt) = if signed {
            (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
        } else {
            (IntCC::UnsignedLessThan, IntCC::UnsignedGreaterThan)
        };
        let l = self.b.ins().icmp(lt, x, y);
        let g = self.b.ins().icmp(gt, x, y);
        let l = self.b.ins().uextend(types::I32, l);
        let g = self.b.ins().uextend(types::I32, g);
        self.b.ins().isub(g, l)
    }

    /// If nonzero, go to done (with that value); if 0, go on to the next block
    pub(crate) fn cmp_chain(&mut self, c: Value, done: Block) {
        let next = self.b.create_block();
        self.b.ins().brif(c, done, &[c], next, &[]);
        self.b.switch_to_block(next);
    }

    /// Compares the sequences (pa, la) and (pb, lb) of element type e lexicographically
    pub(crate) fn cmp_seq(&mut self, pa: Value, la: Value, pb: Value, lb: Value, e: Ty) -> R<Value> {
        let (es, _) = self.prog.size_align(e);
        let done = self.b.create_block();
        let res = self.b.append_block_param(done, types::I32);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let tail = self.b.create_block();
        let i = self.b.declare_var(types::I64);
        let z = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(i, z);
        let n = self.b.ins().umin(la, lb);
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(head);
        let iv = self.b.use_var(i);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, n);
        self.b.ins().brif(more, body, &[], tail, &[]);
        self.b.switch_to_block(body);
        let off = self.b.ins().imul_imm_s(iv, es as i64);
        let xa = self.b.ins().iadd(pa, off);
        let xb = self.b.ins().iadd(pb, off);
        let c = self.cmp_call(xa, xb, e)?;
        let nx = self.b.ins().iadd_imm_s(iv, 1);
        self.b.def_var(i, nx);
        self.b.ins().brif(c, done, &[c], head, &[]);
        self.b.switch_to_block(tail);
        let c = self.cmp_scalar(la, lb, false);
        self.b.ins().jump(done, &[c]);
        self.b.switch_to_block(done);
        Ok(res)
    }

    /// The ordering of a, b (addresses of values of t). Like derive(PartialOrd), compares fields / elements lexicographically from the front.
    /// If there is a hand-written partial_cmp / cmp, calls it
    pub(crate) fn cmp_at(&mut self, a: Value, b: Value, t: Ty) -> R<Value> {
        Ok(match t {
            Ty::Unit => self.b.ins().iconst(types::I32, 0),
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char | Ty::Ptr(..)) => {
                let ct = clif_ty(t).unwrap();
                let x = self.b.ins().load(ct, mf(), a, 0);
                let y = self.b.ins().load(ct, mf(), b, 0);
                self.cmp_scalar(x, y, t.signed())
            }
            // (NaN: 2, partial_cmp's None; `<` / `>=` and the others are all false for it)
            t if t.is_float() => {
                let ct = clif_ty(t).unwrap();
                let x = self.b.ins().load(ct, mf(), a, 0);
                let y = self.b.ins().load(ct, mf(), b, 0);
                let l = self.b.ins().fcmp(FloatCC::LessThan, x, y);
                let g = self.b.ins().fcmp(FloatCC::GreaterThan, x, y);
                let l = self.b.ins().uextend(types::I32, l);
                let g = self.b.ins().uextend(types::I32, g);
                let c = self.b.ins().isub(g, l);
                let uno = self.b.ins().fcmp(FloatCC::Unordered, x, y);
                let two = self.b.ins().iconst(types::I32, 2);
                self.b.ins().select(uno, two, c)
            }
            Ty::Ref(_, Ty::Str | Ty::Slice(_)) => {
                let e = match t {
                    Ty::Ref(_, Ty::Slice(e)) => **e,
                    _ => Ty::U8,
                };
                let pa = self.b.ins().load(PTR, mf(), a, 0);
                let la = self.b.ins().load(types::I64, mf(), a, 8);
                let pb = self.b.ins().load(PTR, mf(), b, 0);
                let lb = self.b.ins().load(types::I64, mf(), b, 8);
                self.cmp_seq(pa, la, pb, lb, e)?
            }
            Ty::Ref(_, i) | Ty::Box(i) if !t.is_fat() => {
                let pa = self.b.ins().load(PTR, mf(), a, 0);
                let pb = self.b.ins().load(PTR, mf(), b, 0);
                self.cmp_call(pa, pb, *i)?
            }
            Ty::Array(e, n) => {
                let l = self.b.ins().iconst(types::I64, n as i64);
                self.cmp_seq(a, l, b, l, *e)?
            }
            Ty::Struct(sid) => {
                // Vec / String by their sequence of contents
                if let Some((fa, ft)) = self.deref_to_slice(a, sid, false) {
                    let (fb, _) = self.deref_to_slice(b, sid, false).unwrap();
                    return self.cmp_at(fa, fb, ft);
                }
                let user = ["partial_cmp", "cmp"]
                    .into_iter()
                    .find(|m| !self.prog.is_tuple(sid) && self.prog.self_method(sid, m).is_some());
                if let Some(m) = user {
                    // A hand-written impl: `__exr_pord_i32(&__a.partial_cmp(__b))`
                    let r = Ty::Ref(false, intern(t));
                    self.scopes.push(vec![]);
                    self.bind("__exr_ca".into(), r, Some(a));
                    self.bind("__exr_cb".into(), r, Some(b));
                    let src = if m == "partial_cmp" && self.prog.index.contains_key("__exr_pord_code") {
                        "__exr_pord_code::<()>(&__exr_ca.partial_cmp(__exr_cb))"
                    } else if m == "partial_cmp" {
                        "__exr_pord_i32::<()>(&__exr_ca.partial_cmp(__exr_cb))"
                    } else {
                        "__exr_ord_i32::<()>(&__exr_ca.cmp(__exr_cb))"
                    };
                    let e = crate::parse::parse_expr(src)?;
                    let e = self.keep_expr(e);
                    let (v, _) = self.expr(e, Some(Ty::I32))?;
                    self.scopes.pop();
                    return Ok(v.unwrap());
                }
                let sd = &self.prog.structs[sid as usize];
                let is_enum = sd.is_enum;
                let tag_ty = sd.tag;
                let fields = sd.fields.clone();
                let variants: Vec<(i64, Vec<(String, Ty, u32)>)> =
                    sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect();
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I32);
                if is_enum {
                    // By the order of the variants' discriminants; for the same variant, lexicographically by fields
                    let ct = clif_ty(tag_ty).unwrap();
                    let ta = self.b.ins().load(ct, mf(), a, 0);
                    let tb = self.b.ins().load(ct, mf(), b, 0);
                    let c = self.cmp_scalar(ta, tb, tag_ty.signed());
                    self.cmp_chain(c, done);
                    for (d, fs) in variants {
                        if fs.is_empty() {
                            continue;
                        }
                        let yes = self.b.create_block();
                        let next = self.b.create_block();
                        let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, d);
                        self.b.ins().brif(is_v, yes, &[], next, &[]);
                        self.b.switch_to_block(yes);
                        for (_, ft, off) in fs {
                            let (fa, fb) = (self.addr_plus(a, off as i32), self.addr_plus(b, off as i32));
                            let c = self.cmp_call(fa, fb, ft)?;
                            self.cmp_chain(c, done);
                        }
                        let z = self.b.ins().iconst(types::I32, 0);
                        self.b.ins().jump(done, &[z]);
                        self.b.switch_to_block(next);
                    }
                } else {
                    for (_, ft, off) in fields {
                        let (fa, fb) = (self.addr_plus(a, off as i32), self.addr_plus(b, off as i32));
                        let c = self.cmp_call(fa, fb, ft)?;
                        self.cmp_chain(c, done);
                    }
                }
                let z = self.b.ins().iconst(types::I32, 0);
                self.b.ins().jump(done, &[z]);
                self.b.switch_to_block(done);
                res
            }
            t => return Err(format!("`{}` cannot be compared with `<`", self.prog.tn(t))),
        })
    }

    /// `a.cmp(&b)` / `a.partial_cmp(&b)` (types with no hand-written impl: integers, tuples, derived types, etc.)
    pub(crate) fn ord_method(&mut self, mc: &Expr) -> R<Option<V>> {
        let Expr::MethodCall(receiver, method, _, margs, _) = mc else { return Ok(None) };
        let name = method.to_string();
        if !matches!(name.as_str(), "cmp" | "partial_cmp") || margs.len() != 1 {
            return Ok(None);
        }
        let helper = if name == "cmp" { "__exr_cmp_m" } else { "__exr_pcmp_m" };
        if !self.prog.index.contains_key(helper) {
            return Ok(None);
        }
        // If there is a hand-written cmp, an ordinary method call
        if let Some(mut t) = self.peek(&**receiver) {
            while let Ty::Ref(_, i) = t {
                t = *i;
            }
            if let Ty::Struct(sid) = t {
                if self.prog.self_method(sid, &name).is_some() {
                    return Ok(None);
                }
            }
        }
        let (v, t) = self.operand(&**receiver, None, true)?;
        let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
        let (ra, rt) = self.ord_addr(v, t);
        // (`Abs(-3).cmp(&b)`: a hand-written impl found only once the receiver's type is known after evaluation calls that method with the evaluated value)
        if let Ty::Struct(sid) = rt {
            if let Some(id) = self.prog.self_method(sid, &name) {
                let args: Vec<&Expr> = margs.iter().collect();
                return self.call_fn(id, Some((ra, Ty::Ref(false, intern(rt)))), &args, None, None, SP).map(Some);
            }
        }
        let arg = &margs[0];
        let (av, at) = self.operand(arg, Some(Ty::Ref(false, intern(rt))), true)?;
        let Some(av) = av else { return Ok(Some((None, Ty::Never))) };
        let (aa, at2) = self.ord_addr(av, at);
        if !matches!(at, Ty::Ref(..)) || at2 != rt {
            return err(
                SP,
                format!("mismatched types: expected `&{}`, found `{}`", self.prog.tn(rt), self.prog.tn(at)),
            );
        }
        let r = Ty::Ref(false, intern(rt));
        // (a floating-point partial_cmp is None on NaN)
        if name == "partial_cmp" && rt.is_float() && self.prog.index.contains_key("__exr_pcmp_fl") {
            let ct = clif_ty(rt).unwrap();
            let x = self.b.ins().load(ct, mf(), ra, 0);
            let y = self.b.ins().load(ct, mf(), aa, 0);
            return self.call_named("__exr_pcmp_fl", vec![rt], &[(x, rt), (y, rt)], SP).map(Some);
        }
        self.call_named(helper, vec![rt], &[(ra, r), (aa, r)], SP).map(Some)
    }
}


/// The trait object type `s` (`Display`, `fmt::Debug + Send` ..) has the formatting trait's `fmt`
impl Cx<'_> {
    /// The number type of std's NonZero type t (a prelude struct whose one field is that number), if t is one
    pub(crate) fn nonzero_inner(&self, t: Ty) -> Option<Ty> {
        let Ty::Struct(sid) = t else { return None };
        let b = self.prog.base_of(sid);
        if !self.prog.struct_item.get(b as usize).is_some_and(|&i| i >= self.prog.prelude_from) {
            return None;
        }
        Some(match self.prog.structs[b as usize].name.as_str() {
            "NonZeroU8" => Ty::U8,
            "NonZeroU16" => Ty::U16,
            "NonZeroU32" => Ty::U32,
            "NonZeroU64" => Ty::U64,
            "NonZeroU128" => Ty::U128,
            "NonZeroUsize" => Ty::Usize,
            "NonZeroI8" => Ty::I8,
            "NonZeroI16" => Ty::I16,
            "NonZeroI32" => Ty::I32,
            "NonZeroI64" => Ty::I64,
            "NonZeroI128" => Ty::I128,
            "NonZeroIsize" => Ty::Isize,
            _ => return None,
        })
    }
}

fn dyn_has_fmt(s: &str, debug: bool) -> bool {
    let want = if debug { "Debug" } else { "Display" };
    s.split('+').any(|p| p.trim().rsplit("::").next() == Some(want))
}

/// Where the k-th char of a string literal's value is in its source text `t` (which starts at `start`): escapes,
/// `\` line continuations and raw strings counted; k past the end is the closing quote
pub(crate) fn lit_char_pos(t: &str, start: Sp, k: usize) -> Option<Sp> {
    let cs: Vec<char> = t.chars().collect();
    let (mut line, mut col) = (start.line, start.col);
    let mut i = 0;
    let step = |i: &mut usize, line: &mut u32, col: &mut u32| {
        if cs[*i] == '\n' {
            *line += 1;
            *col = 0;
        } else {
            *col += 1;
        }
        *i += 1;
    };
    // (the prefix: `b` / `c`, then `r` and its `#`s, then the quote)
    while i < cs.len() && matches!(cs[i], 'b' | 'c') {
        step(&mut i, &mut line, &mut col);
    }
    let raw = i < cs.len() && cs[i] == 'r';
    let mut hashes = 0;
    if raw {
        step(&mut i, &mut line, &mut col);
        while i < cs.len() && cs[i] == '#' {
            hashes += 1;
            step(&mut i, &mut line, &mut col);
        }
    }
    if i >= cs.len() || cs[i] != '"' {
        return None;
    }
    step(&mut i, &mut line, &mut col);
    let mut n = 0;
    while i < cs.len() {
        let at = Sp { line, col };
        // (the closing quote)
        if cs[i] == '"' && (!raw || cs[i + 1..].iter().take(hashes).filter(|&&c| c == '#').count() == hashes) {
            return Some(at);
        }
        if !raw && cs[i] == '\\' && i + 1 < cs.len() {
            if cs[i + 1] == '\n' {
                // (a line continuation: the newline and the white space after it are no chars of the value)
                step(&mut i, &mut line, &mut col);
                while i < cs.len() && cs[i].is_whitespace() {
                    step(&mut i, &mut line, &mut col);
                }
                continue;
            }
            if n == k {
                return Some(at);
            }
            let len = match cs[i + 1] {
                'x' => 4,
                'u' => cs[i..].iter().position(|&c| c == '}').map_or(2, |p| p + 1),
                _ => 2,
            };
            for _ in 0..len.min(cs.len() - i) {
                step(&mut i, &mut line, &mut col);
            }
            n += 1;
            continue;
        }
        if n == k {
            return Some(at);
        }
        step(&mut i, &mut line, &mut col);
        n += 1;
    }
    None
}
