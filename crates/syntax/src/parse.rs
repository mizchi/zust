//! Rust parsing without syn (the subset zustc handles + what is around it).
//!
//! The groundwork for leaving syn: this module depends on no external crate and is written in a style that zustc itself can compile too
//! (enum + Box + Vec + String, a minimum of traits).
//! - Lexing: punctuation is one character at a time (Joint / Alone), and `>>` etc. are assembled on the parser's side (compatible with the `>` of generics)
//! - Expression precedence is the same as Rust's. The heads of `if` / `while` / `match` / `for` take no struct literals
//! - Attributes and macro calls are kept as the text of their token sequences

// ---------------------------------------------------------------- Lexing

#[derive(Clone, Debug, PartialEq)]
pub enum TokKind {
    Ident,
    Lifetime,
    Lit,
    Punct,
    Open,
    Close,
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub kind: TokKind,
    pub text: String,
    /// Whether another punctuation character immediately follows (for assembling `>>` / `::` / `->`)
    pub joint: bool,
    pub line: u32,
    pub col: u32,
    pub lo: u32,
}

fn is_ident_start(c: char) -> bool {
    if c.is_ascii() {
        return c == '_' || c.is_ascii_alphabetic();
    }
    c.is_alphabetic()
}

fn is_ident_cont(c: char) -> bool {
    if c.is_ascii() {
        return c == '_' || c.is_ascii_alphanumeric();
    }
    c.is_alphanumeric()
}

pub fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;
    let mut line: u32 = 1;
    let mut line_start = 0;
    let (mut cline_start, mut cpos, mut ccol) = (usize::MAX, 0usize, 0u32);
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            line_start = i;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && src[i..].starts_with("/*") {
            let mut depth = 0;
            // (by bytes: a byte inside a multi-byte character is no `/*` or `*/`, and a str slice there would panic)
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
                    if b[i] == b'\n' {
                        line += 1;
                        line_start = i + 1;
                    }
                    i += 1;
                }
            }
            continue;
        }
        let start = i;
        // (the column is the number of chars from the line start: counted on from the previous token's position (so that long lines do not go quadratic))
        if cline_start != line_start {
            cline_start = line_start;
            cpos = line_start;
            ccol = 0;
        }
        while cpos < i {
            if (b[cpos] & 0xC0) != 0x80 {
                ccol += 1;
            }
            cpos += 1;
        }
        let col = ccol;
        let tline = line;
        // (ASCII stays a byte; it is made a char only when it is not ASCII)
        let ch = if c < 0x80 { c as char } else { src[i..].chars().next().unwrap() };
        let kind;
        if c == b'"' || ((c == b'b' || c == b'c') && b.get(i + 1) == Some(&b'"')) {
            if c != b'"' {
                i += 1;
            }
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                if i < b.len() && b[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            if i >= b.len() {
                return Err(format!("{tline}:{}: unterminated string", col + 1));
            }
            i += 1;
            i = skip_suffix(src, i);
            kind = TokKind::Lit;
        } else if (c == b'r' && (b.get(i + 1) == Some(&b'"') || (b.get(i + 1) == Some(&b'#') && raw_follows(b, i + 1))))
            || (c == b'b' && b.get(i + 1) == Some(&b'r') && matches!(b.get(i + 2), Some(&b'"') | Some(&b'#')))
        {
            i += if c == b'b' { 2 } else { 1 };
            let mut h = 0;
            while i < b.len() && b[i] == b'#' {
                h += 1;
                i += 1;
            }
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(format!("{tline}:{}: unterminated raw string", col + 1));
                }
                if b[i] == b'"' && b.len() >= i + 1 + h && b[i + 1..i + 1 + h].iter().all(|&x| x == b'#') {
                    i += 1 + h;
                    break;
                }
                if b[i] == b'\n' {
                    line += 1;
                    line_start = i + 1;
                }
                i += 1;
            }
            i = skip_suffix(src, i);
            kind = TokKind::Lit;
        } else if c == b'\'' || (c == b'b' && b.get(i + 1) == Some(&b'\'')) {
            // A char 'x' / '\n' / b'x', or a lifetime 'a
            let q = if c == b'b' { i + 1 } else { i };
            let mut it = src[q + 1..].chars();
            let c1 = it.next();
            let c2 = it.next();
            if c == b'b' || c1 == Some('\\') || (c1.is_some() && c2 == Some('\'')) {
                i = q + 1;
                while i < b.len() && b[i] != b'\'' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                // (no closing quote before the end: `b'` or `'\` as the source's last characters)
                // (no closing quote before the end: `b'` or `'\` as the source's last characters)
                if i >= b.len() {
                    let what = if c == b'b' { "unterminated byte constant (E0763)" } else { "unterminated character literal (E0762)" };
                    return Err(format!("{tline}:{}: {what}", col + 1));
                }
                i += 1;
                i = skip_suffix(src, i);
                kind = TokKind::Lit;
            } else {
                i += 1;
                // (a raw lifetime `'r#a` is `'a`)
                let raw = b.get(i) == Some(&b'r') && b.get(i + 1) == Some(&b'#') && b.get(i + 2).is_some_and(|&d| d == b'_' || d.is_ascii_alphabetic() || d >= 0x80);
                if raw {
                    i += 2;
                }
                while i < b.len() {
                    let d = src[i..].chars().next().unwrap();
                    if !is_ident_cont(d) {
                        break;
                    }
                    i += d.len_utf8();
                }
                if raw {
                    raw_lifetime(&mut out, src, start, i, tline, col);
                    continue;
                }
                kind = TokKind::Lifetime;
            }
        } else if c.is_ascii_digit() {
            let hex = src[i..].starts_with("0x") || src[i..].starts_with("0b") || src[i..].starts_with("0o");
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_alphanumeric() || d == b'_' {
                    let e = (d == b'e' || d == b'E') && !hex;
                    i += 1;
                    if e && i < b.len() && (b[i] == b'-' || b[i] == b'+') {
                        i += 1;
                    }
                } else if d == b'.' && !hex && !src[start..i].contains('.') {
                    let n = b.get(i + 1).copied().unwrap_or(0);
                    if n.is_ascii_digit() {
                        i += 1;
                    } else if n != b'.' && !(n == b'_' || n.is_ascii_alphabetic()) {
                        i += 1;
                        break;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            kind = TokKind::Lit;
        } else if is_ident_start(ch) {
            if c == b'r' && b.get(i + 1) == Some(&b'#') {
                i += 2;
                // (`r#self` / `r#_`: a name no raw identifier can be)
                if let Some(w) = reserved_raw(src, i) {
                    return Err(format!("{tline}:{}: `{w}` cannot be a raw identifier", col + 1));
                }
            }
            while i < b.len() {
                let d0 = b[i];
                if d0 < 0x80 {
                    if d0.is_ascii_alphanumeric() || d0 == b'_' {
                        i += 1;
                        continue;
                    }
                    break;
                }
                let d = src[i..].chars().next().unwrap();
                if !is_ident_cont(d) {
                    break;
                }
                i += d.len_utf8();
            }
            kind = TokKind::Ident;
        } else if c == b'(' || c == b'[' || c == b'{' {
            i += 1;
            kind = TokKind::Open;
        } else if c == b')' || c == b']' || c == b'}' {
            i += 1;
            kind = TokKind::Close;
        } else if c < 0x80 && is_punct_byte(c) {
            i += 1;
            kind = TokKind::Punct;
        } else if other_white_space(ch) {
            // (Rust's other white space characters: vertical tab, next line, the direction marks, line / paragraph
            // separators)
            i += ch.len_utf8();
            continue;
        } else {
            return Err(format!("{tline}:{}: unexpected character {ch:?}", col + 1));
        }
        let joint = kind == TokKind::Punct && i < b.len() && is_punct_byte(b[i]);
        out.push(Tok { kind: kind, text: src[start..i].to_string(), joint: joint, line: tline, col: col, lo: start as u32 });
    }
    Ok(out)
}

/// The name after `r#` at src[i..] when it is one no raw identifier can be, `r#self` / `r#_`
#[cold]
#[inline(never)]
fn reserved_raw(src: &str, i: usize) -> Option<&str> {
    let n = src[i..].bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_').count();
    let w = &src[i..i + n];
    matches!(w, "self" | "super" | "crate" | "Self" | "_").then_some(w)
}

/// A raw lifetime `'r#a` (src[start..end]) as the token `'a`, marked `joint` (which a lifetime has no other use for):
/// `'r#break` is a name where a label goes, `'break` a keyword
#[cold]
#[inline(never)]
fn raw_lifetime(out: &mut Vec<Tok>, src: &str, start: usize, end: usize, line: u32, col: u32) {
    let text = format!("'{}", &src[start + 3..end]);
    out.push(Tok { kind: TokKind::Lifetime, text, joint: true, line, col, lo: start as u32 });
}

/// Rust's white space characters other than ASCII (and the vertical tab / form feed)
#[cold]
#[inline(never)]
fn other_white_space(ch: char) -> bool {
    matches!(ch, '\u{b}' | '\u{c}' | '\u{85}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}')
}

/// Whether it is a punctuation character (`+-*/%^!&|=<>@.,;:#$?~\`)
fn is_punct_byte(c: u8) -> bool {
    matches!(c, b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'!' | b'&' | b'|' | b'=' | b'<' | b'>' | b'@' | b'.' | b',' | b';' | b':' | b'#' | b'$' | b'?' | b'~' | b'\\')
}

fn raw_follows(b: &[u8], mut i: usize) -> bool {
    while i < b.len() && b[i] == b'#' {
        i += 1;
    }
    i < b.len() && b[i] == b'"'
}

fn skip_suffix(src: &str, mut i: usize) -> usize {
    let b = src.as_bytes();
    if i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphabetic()) {
        while i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
            i += 1;
        }
    }
    i
}

// ---------------------------------------------------------------- Syntax tree

/// A position in the source (lines are 1-based, columns are 0-based char counts: same as proc_macro2)
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl std::fmt::Debug for Pos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// An expression's position: start is the start of the expression (syn's `e.span()`), at is a per-expression position (the method name, `[`, the operator, the opening bracket of a macro's arguments)
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: Pos,
    pub at: Pos,
}

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{:?}/{:?}", self.start, self.at)
    }
}

#[derive(Clone, Debug)]
pub struct Path {
    pub global: bool,
    pub segs: Vec<Seg>,
    /// The source position of the start of the path (for error locations)
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub name: String,
    pub args: Vec<GArg>,
    /// Arguments of the form `Fn(A, B) -> C`
    pub fn_args: Option<(Vec<Ty>, Option<Box<Ty>>)>,
    /// The text of this segment's tokens (spaced the same as proc_macro2's display: `Iterator < Item = u32 >`)
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum GArg {
    Ty(Ty),
    Lifetime(String),
    Bind(String, Ty),
    Const(Expr),
}

#[derive(Clone, Debug)]
pub enum Ty {
    Path(Option<Box<Ty>>, Path),
    Ref(Option<String>, bool, Box<Ty>),
    Ptr(bool, Box<Ty>),
    Slice(Box<Ty>),
    Array(Box<Ty>, Box<Expr>),
    Tuple(Vec<Ty>),
    Fn(Vec<Ty>, Box<Ty>),
    Impl(Vec<Bound>),
    Dyn(Vec<Bound>),
    Never,
    Infer,
    Macro(String),
}

#[derive(Clone, Debug)]
pub enum Bound {
    Trait(bool, Path),
    Lifetime(String),
}

/// the initializer text of a `static X: T;` declared in an extern block
pub const EXTERN_STATIC: &str = "__exr_extern_static";
/// the initializer text Program keeps for such a static: this, then its line in the extern block, a space and the symbol
pub const EXTERN_STATIC_SYM: &str = "\u{1}extern ";

/// An extern static's (line in its extern block, symbol), from the initializer text Program keeps
pub fn extern_static(expr: &str) -> Option<(usize, &str)> {
    let (l, sym) = expr.strip_prefix(EXTERN_STATIC_SYM)?.split_once(' ')?;
    Some((l.parse().ok()?, sym))
}

#[derive(Clone, Debug)]
pub enum Pat {
    Wild,
    Rest,
    Ident(bool, bool, String, Option<Box<Pat>>),
    Lit(Box<Expr>),
    Range(Option<Box<Expr>>, Option<Box<Expr>>, bool),
    Tuple(Vec<Pat>),
    TupleStruct(Path, Vec<Pat>),
    Struct(Path, Vec<(String, Pat)>, bool),
    Path(Path),
    Ref(bool, Box<Pat>),
    /// (the alternatives, and where each starts)
    Or(Vec<Pat>, Vec<Pos>),
    Slice(Vec<Pat>),
    Macro(String),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Lit(String, Pos),
    Path(Option<Box<Ty>>, Path),
    Unary(String, Box<Expr>, Span),
    Binary(String, Box<Expr>, Box<Expr>, Span),
    Assign(Box<Expr>, Box<Expr>),
    AssignOp(String, Box<Expr>, Box<Expr>, Span),
    Call(Box<Expr>, Vec<Expr>),
    MethodCall(Box<Expr>, String, Vec<GArg>, Vec<Expr>, Span),
    Field(Box<Expr>, String, Span),
    Index(Box<Expr>, Box<Expr>, Span),
    Range(Option<Box<Expr>>, Option<Box<Expr>>, bool),
    Ref(bool, Box<Expr>),
    RawRef(bool, Box<Expr>),
    Cast(Box<Expr>, Box<Ty>),
    Block(Option<String>, bool, Block),
    Const(Block),
    If(Box<Expr>, Block, Option<Box<Expr>>),
    Let(Box<Pat>, Box<Expr>),
    Match(Box<Expr>, Vec<Arm>),
    While(Option<String>, Box<Expr>, Block),
    Loop(Option<String>, Block),
    For(Option<String>, Box<Pat>, Box<Expr>, Block),
    /// (the last field: where its label is, else where the keyword is)
    Break(Option<String>, Option<Box<Expr>>, Pos),
    Continue(Option<String>, Pos),
    /// (the last field: where the keyword is)
    Return(Option<Box<Expr>>, Pos),
    /// (the last field: where the closure starts)
    Closure(bool, Vec<(Pat, Option<Ty>)>, Option<Box<Ty>>, Box<Expr>, Pos),
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    Repeat(Box<Expr>, Box<Expr>),
    Struct(Path, Vec<(String, Expr)>, Option<Box<Expr>>),
    Macro(Path, String, Span),
    Try(Box<Expr>),
    Paren(Box<Expr>),
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pat,
    /// where the pattern starts
    pub pos: Pos,
    pub guard: Option<Expr>,
    pub body: Expr,
}

/// Where a pattern starts, when it has a path or holds one (else no position)
pub fn pat_pos(p: &Pat) -> Pos {
    match p {
        Pat::Path(pa) | Pat::TupleStruct(pa, _) | Pat::Struct(pa, _, _) => pa.pos,
        Pat::Ref(_, i) => pat_pos(i),
        Pat::Tuple(ps) | Pat::Or(ps, _) | Pat::Slice(ps) => ps.iter().map(pat_pos).find(|q| q.line != 0).unwrap_or_default(),
        _ => Pos::default(),
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// where each statement starts, as parsed (one per statement; empty for a block made by the compiler): for an
    /// error with no position of its own, that of its statement
    pub spos: Spos,
}

/// The starts of those statements of a block whose expressions have no position of their own, by the statement's
/// index: one pointer (a `Vec` in Block made every `Expr` larger), nothing allocated for a block without one
#[derive(Clone, Debug, Default)]
pub struct Spos(Option<Box<Vec<(u32, Pos)>>>);

impl Spos {
    pub fn of(v: Vec<(u32, Pos)>) -> Spos {
        Spos(if v.is_empty() { None } else { Some(Box::new(v)) })
    }
    pub fn entries(&self) -> &[(u32, Pos)] {
        self.0.as_deref().map_or(&[], |v| v.as_slice())
    }
    /// The start of statement i, if kept
    pub fn get(&self, i: usize) -> Option<Pos> {
        self.entries().iter().find(|e| e.0 as usize == i).map(|e| e.1)
    }
    /// The same starts for the statements moved by `by` (`from` and on: the ones before are dropped)
    pub fn shifted(&self, from: usize, by: isize) -> Spos {
        Spos::of(self.entries().iter().filter(|e| e.0 as usize >= from).map(|&(i, p)| ((i as isize + by) as u32, p)).collect())
    }
}

/// The suffix of a number literal that is not a numeric type (`123X1a3`: `X1a3`; `0XYZ`: `XYZ`, an uppercase base
/// prefix being no prefix), or None (not a number, or a valid suffix)
pub fn bad_num_suffix(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    if !b.first().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    // (digits, then letters only at the end: `0u32` / `1_000usize`, the common suffixed literal, decided by its tail)
    let k = b.iter().position(|c| c.is_ascii_alphabetic()).unwrap_or(b.len());
    if k == b.len() {
        return None;
    }
    if b[..k].iter().all(|c| c.is_ascii_digit() || *c == b'_') && b[k] != b'e' && b[k] != b'E' && !(k == 1 && b[0] == b'0') {
        let sfx = &text[k..];
        let ok = matches!(sfx, "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "f16" | "f32" | "f64" | "f128");
        return (!ok).then_some(sfx);
    }
    let (mut i, digit): (usize, fn(u8) -> bool) = match text.get(..2) {
        Some("0x") => (2, |c| c.is_ascii_hexdigit() || c == b'_'),
        Some("0o") => (2, |c| (b'0'..=b'7').contains(&c) || c == b'_'),
        Some("0b") => (2, |c| c == b'0' || c == b'1' || c == b'_'),
        _ => (0, |c| c.is_ascii_digit() || c == b'_'),
    };
    let dec = i == 0;
    while i < b.len() && digit(b[i]) {
        i += 1;
    }
    if dec {
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            }
            // (`1e_2`: underscores before the exponent's first digit)
            while j < b.len() && b[j] == b'_' {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_digit() {
                i = j;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
            }
        }
    }
    let sfx = &text[i..];
    let ok = matches!(sfx, "" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "f16" | "f32" | "f64" | "f128");
    (!ok).then_some(sfx)
}

/// `(a, b) = e;` / `[a, _] = e;` (destructuring assignment): `{ let (t0, t1) = e; a = t0; b = t1; }` (the right side
/// first, then the places in order). None for a left side this does not cover (`..`, struct patterns)
fn destructure_assign(l: &Expr, r: &Expr, at: Pos) -> Option<Expr> {
    fn pat(l: &Expr, n: &mut usize, assigns: &mut Vec<(Expr, String)>, line: u32) -> Option<Pat> {
        match l {
            Expr::Tuple(xs) => Some(Pat::Tuple(xs.iter().map(|x| pat(x, n, assigns, line)).collect::<Option<Vec<_>>>()?)),
            Expr::Array(xs) => Some(Pat::Slice(xs.iter().map(|x| pat(x, n, assigns, line)).collect::<Option<Vec<_>>>()?)),
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "_" => Some(Pat::Wild),
            Expr::Range(..) => None,
            Expr::Paren(x) => pat(x, n, assigns, line),
            e => {
                let t = format!("__exr_da{}_{}", line, *n);
                *n += 1;
                assigns.push((e.clone(), t.clone()));
                Some(Pat::Ident(false, false, t, None))
            }
        }
    }
    let mut n = 0;
    let mut assigns = vec![];
    let p = pat(l, &mut n, &mut assigns, at.line as u32)?;
    let mut stmts = vec![Stmt::Let(p, None, Some(r.clone()), None, at)];
    for (place, t) in assigns {
        let v = Expr::Path(None, Path { global: false, segs: vec![Seg { name: t.clone(), args: Vec::new(), fn_args: None, text: t }], pos: at });
        stmts.push(Stmt::Expr(Expr::Assign(Box::new(place), Box::new(v)), true));
    }
    Some(Expr::Block(None, false, Block { stmts, spos: Default::default() }))
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// (the last field: where `let` starts)
    Let(Pat, Option<Ty>, Option<Expr>, Option<Block>, Pos),
    Expr(Expr, bool),
    Item(Item),
}

#[derive(Clone, Debug)]
pub struct Generics {
    pub params: Vec<GParam>,
    pub preds: Vec<(Ty, Vec<Bound>)>,
}

#[derive(Clone, Debug)]
pub enum GParam {
    Lifetime(String),
    Ty(String, Vec<Bound>, Option<Ty>),
    Const(String, Ty),
}

#[derive(Clone, Debug)]
pub struct FnSig {
    pub name: String,
    pub generics: Generics,
    /// How self is taken ("self" / "&self" / "&mut self" / "mut self")
    pub recv: Option<String>,
    /// The type of self (`&self` → `&Self`, `self: Box<Self>` → `Box<Self>`)
    pub recv_ty: Option<Ty>,
    pub params: Vec<(Pat, Ty)>,
    pub ret: Option<Ty>,
    pub quals: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Fields {
    Unit,
    Tuple(Vec<Ty>),
    Named(Vec<(String, Ty)>),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub attrs: Vec<String>,
    pub vis: bool,
    pub kind: ItemKind,
    pub line: u32,
    /// The token text of a const / static's initializer expression (spaced the same as proc_macro2's display)
    pub init_text: String,
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    Fn(FnSig, Option<Block>),
    Struct(String, Generics, Fields),
    Enum(String, Generics, Vec<(String, Fields, Option<Expr>)>),
    Union(String, Generics, Fields),
    Impl(Generics, Option<(bool, Path)>, Ty, Vec<Item>),
    Trait(String, Generics, Vec<Bound>, Vec<Item>),
    Use(String),
    Const(String, Option<Ty>, Option<Expr>),
    Static(bool, String, Ty, Expr),
    Type(String, Generics, Vec<Bound>, Option<Ty>),
    Mod(String, Option<Vec<Item>>),
    Extern(Vec<Item>),
    MacroRules(String, String),
    Macro(Path, String),
    ExternCrate(String),
}

#[derive(Clone, Debug)]
pub struct File {
    pub attrs: Vec<String>,
    pub items: Vec<Item>,
}

// ---------------------------------------------------------------- Parsing

pub struct Parser {
    toks: Vec<Tok>,
    i: usize,
    init_text: String,
    /// The base of positions (when re-reading a macro's arguments, the position of the start of that text; at line 0, no shift)
    base: Pos,
    /// Check that `.await` is inside an `async` fn / block (when reading a whole file; a re-read macro argument
    /// does not know where it was)
    async_on: bool,
    /// Inside an `async` fn / block / closure (a plain closure inside one is not)
    in_async: bool,
    /// Inside an `extern "C" { .. }` block: a `static X: T;` without a value is allowed there only
    in_extern: bool,
    /// A validation error rustc reports after parsing (a binder's bounds or type parameter, `for<'a: 'b>`): returned by
    /// parse_file, marked `\u{7}`, when nothing fails to parse
    deferred: Option<String>,
    /// The parameters the last generics() read with a default: (the index, the parameter's first token, its name's
    /// token, whether it is a const one)
    gdefaults: Vec<(u32, u32, u32, bool)>,
    /// Inside a value that must be a constant (a const's or static's value, a discriminant, an array's length) and
    /// not in a fn or closure body in it: `return` there is E0572
    in_const: bool,
    /// E0416 is checked where a pattern is parsed; off while a fn parameter may still turn out to be a 2015 anonymous
    /// one (`fn f((u8, u8));`, a type)
    no_dup_check: bool,
}

type R<T> = Result<T, String>;

fn bin_prec(op: &str) -> u32 {
    match op {
        "||" => 3,
        "&&" => 4,
        "==" | "!=" | "<" | ">" | "<=" | ">=" => 5,
        "|" => 6,
        "^" => 7,
        "&" => 8,
        "<<" | ">>" => 9,
        "+" | "-" => 10,
        "*" | "/" | "%" => 11,
        _ => 0,
    }
}

/// The text of the tokens toks, after (the token that follows them, if any) only telling whether the last one is
/// joint: a token apart from the next by one space, a joint punctuation's next and a group's contents not (as
/// proc_macro2's Display)
pub fn join_tokens(toks: &[Tok], after: Option<&Tok>) -> String {
    let mut s = String::new();
    let mut prev_joint = true;
    let mut prev_open = false;
    for (k, t) in toks.iter().enumerate() {
        if t.kind == TokKind::Close {
            // (`{ .. }` has a space on both sides of the contents: same as proc_macro2)
            if t.text == "}" && !prev_open {
                s.push(' ');
            }
            s.push_str(&t.text);
            prev_joint = false;
            prev_open = false;
            continue;
        }
        if !s.is_empty() && !prev_joint && !prev_open {
            s.push(' ');
        }
        if prev_open && s.ends_with('{') {
            s.push(' ');
        }
        s.push_str(&t.text);
        prev_open = t.kind == TokKind::Open;
        // (a punctuation is Joint when the next token is punctuation too (`,` is not joined); a lifetime is one token)
        prev_joint = t.kind == TokKind::Punct
            && t.joint
            && toks.get(k + 1).or(if k + 1 == toks.len() { after } else { None }).is_some_and(|n| n.kind == TokKind::Punct && n.text != ",");
    }
    s
}

/// Source text with its tokens joined as join_tokens does (`A<&i32>` is `A <& i32 >`, as a `dyn` type's text)
pub fn norm_tokens(s: &str) -> Option<String> {
    Some(join_tokens(&tokenize(s).ok()?, None))
}

/// A `dyn` type's text (`A < T >`, the tokens joined as join_tokens does) with some of its identifiers replaced by
/// source text: f gives an identifier token's replacement, from the tokens around it. The replaced text is read as
/// if written without spaces (`A<&i32>` is `A <& i32 >`). None where nothing was replaced, or the text does not lex
pub fn rewrite_tokens(s: &str, f: &mut dyn FnMut(usize, &[Tok]) -> Option<String>) -> Option<String> {
    let toks = tokenize(s).ok()?;
    let mut out = String::new();
    let mut any = false;
    for (k, t) in toks.iter().enumerate() {
        let piece = match t.kind {
            TokKind::Ident => f(k, &toks).inspect(|_| any = true).unwrap_or_else(|| t.text.clone()),
            _ => t.text.clone(),
        };
        let word = |c: char| c.is_alphanumeric() || c == '_' || c == '\'' || c == '"';
        // (two punctuations apart in the text stay apart: `/ *` is no comment, `- >` no arrow)
        let apart = k > 0 && toks[k - 1].kind == TokKind::Punct && !toks[k - 1].joint && piece.starts_with(|c: char| !word(c) && !c.is_whitespace());
        if (out.ends_with(word) && piece.starts_with(word)) || apart {
            out.push(' ');
        }
        out.push_str(&piece);
    }
    if !any {
        return None;
    }
    let toks = tokenize(&out).ok()?;
    Some(join_tokens(&toks, None))
}

pub fn parse_file(src: &str) -> R<File> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks: toks, i: 0, init_text: String::new(), base: Pos::default(), async_on: true, in_async: false, in_extern: false, deferred: None, in_const: false, no_dup_check: false, gdefaults: Vec::new() };
    let mut attrs = Vec::new();
    while p.is_punct("#") && p.peek_is(1, "!") {
        attrs.push(p.attr()?);
    }
    let mut items = Vec::new();
    while p.i < p.toks.len() {
        items.push(p.item()?);
    }
    if let Some(d) = p.deferred {
        return Err(format!("\u{7}{d}"));
    }
    Ok(File { attrs: attrs, items: items })
}

/// The source position of byte offset off in text (which starts at position base)
pub fn pos_at(text: &str, off: usize, base: Pos) -> Pos {
    let before = &text[..off.min(text.len())];
    let nl = before.bytes().filter(|&c| c == b'\n').count() as u32;
    if base.line == 0 {
        return Pos::default();
    }
    if nl == 0 {
        Pos { line: base.line, col: base.col + before.chars().count() as u32 }
    } else {
        Pos { line: base.line + nl, col: before.rsplit('\n').next().map(|x| x.chars().count() as u32).unwrap_or(0) }
    }
}

pub fn parse_expr(src: &str) -> R<Expr> {
    parse_expr_at(src, Pos::default())
}

/// Parses an expression. Positions are made relative to base (the source position of the start of src)
pub fn parse_expr_at(src: &str, base: Pos) -> R<Expr> {
    let toks = tokenize(src)?;
    let mut p = Parser { toks: toks, i: 0, init_text: String::new(), base, async_on: false, in_async: false, in_extern: false, deferred: None, in_const: false, no_dup_check: false, gdefaults: Vec::new() };
    let e = p.expr(true)?;
    if p.i < p.toks.len() {
        return Err(p.err("unexpected token after expression"));
    }
    Ok(e)
}

/// `pat` / `pat if guard` (the second argument of matches!)
pub fn parse_pat_guard(src: &str) -> Option<(Pat, Option<Expr>)> {
    parse_pat_guard_at(src, Pos::default())
}

pub fn parse_pat_guard_at(src: &str, base: Pos) -> Option<(Pat, Option<Expr>)> {
    let toks = tokenize(src).ok()?;
    let mut p = Parser { toks: toks, i: 0, init_text: String::new(), base, async_on: false, in_async: false, in_extern: false, deferred: None, in_const: false, no_dup_check: false, gdefaults: Vec::new() };
    let pat = p.pat().ok()?;
    let guard = if p.eat_ident("if") { Some(p.expr(true).ok()?) } else { None };
    p.eat_punct(",");
    if !p.at_end() {
        return None;
    }
    Some((pat, guard))
}

/// `Box<F>` / `Pin<F>` / `Pin<&mut F>` around the marker F of `dyn Future<Output = T>`: the marker (it is T, see
/// ast_types::tysyn)
fn fut_output(p: &Path) -> Option<Ty> {
    let last = p.segs.last()?;
    if !matches!(last.name.as_str(), "Box" | "Pin") {
        return None;
    }
    let [GArg::Ty(t)] = &last.args[..] else { return None };
    let marker = |t: &Ty| matches!(t, Ty::Path(None, q) if q.segs.len() == 1 && q.segs[0].name == "__exr_fut");
    match t {
        t if marker(t) => Some(t.clone()),
        Ty::Ref(_, _, i) if last.name == "Pin" && marker(i) => Some((**i).clone()),
        _ => None,
    }
}

impl Parser {
    /// E0582: a lifetime a binder (`for<'a>`) declares that the bound's result (`-> T`) or an associated type's binding
    /// (`Item = T`) names, but not its inputs (`Fn(..)`'s parameters, the trait's arguments): no type can be it
    fn hr_output_check(&self, binder: &[GArg], p: &Path, at: usize, self_ty: Option<&Ty>) -> R<()> {
        if binder.is_empty() {
            return Ok(());
        }
        let Some(last) = p.segs.last() else { return Ok(()) };
        // (a generic associated type's binding (`Item<'a> = &'a T`, `Gat<'b>: Tr`) drops its arguments, which may be
        // the binder's inputs: not checked)
        let t: String = last.text.chars().filter(|c| !c.is_whitespace()).collect();
        if t.contains(">=") || t.match_indices(">:").any(|(k, _)| !t[k + 2..].starts_with(':')) {
            return Ok(());
        }
        for b in binder {
            let GArg::Lifetime(l) = b else { continue };
            // (the predicate's own type is an input: `for<'a> &'a T: IntoIterator<Item = &'a U>`)
            let mut ins = self_ty.is_some_and(|t| crate::regions::mentions_lt(t, l)) || last.args.iter().any(|a| match a {
                GArg::Ty(t) => crate::regions::mentions_lt(t, l),
                GArg::Lifetime(x) => x == l,
                _ => false,
            });
            // (`Item = T` binds; `Item: 'a` (parsed as `impl` bounds) only bounds it, which a binder's lifetime may)
            let mut out = last.args.iter().any(|a| matches!(a, GArg::Bind(_, t) if !matches!(t, Ty::Impl(_)) && crate::regions::mentions_lt(t, l)));
            if let Some((ps, r)) = &last.fn_args {
                ins |= ps.iter().any(|t| crate::regions::mentions_lt(t, l));
                out |= r.as_deref().is_some_and(|t| crate::regions::mentions_lt(t, l));
            }
            // (an earlier segment's arguments are inputs too: `a::Tr<'a>::..` is rare, taken as one)
            ins |= p.segs[..p.segs.len() - 1].iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Lifetime(x) if x == l) || matches!(a, GArg::Ty(t) if crate::regions::mentions_lt(t, l))));
            if out && !ins {
                let line = self.toks.get(at).map(|t| (t.line, t.col + 1)).unwrap_or((0, 0));
                let bound_name = last.args.iter().find_map(|a| match a {
                    GArg::Bind(n, t) if !matches!(t, Ty::Impl(_)) && crate::regions::mentions_lt(t, l) => Some(n.clone()),
                    _ => None,
                });
                let what = format!("`{}`", bound_name.unwrap_or_else(|| "Output".to_string()));
                return Err(format!("{}:{}: binding for associated type {what} references lifetime `{l}`, which does not appear in the trait input types (E0582)", line.0, line.1));
            }
        }
        Ok(())
    }

    fn err(&self, msg: &str) -> String {
        match self.toks.get(self.i) {
            Some(t) => format!("{}:{}: {msg} (found `{}`)", t.line, t.col + 1, t.text),
            None => format!("end of input: {msg}"),
        }
    }
    /// an error at the current token, without the token's text
    fn err_plain(&self, msg: &str) -> String {
        match self.toks.get(self.i) {
            Some(t) => format!("{}:{}: {msg}", t.line, t.col + 1),
            None => format!("end of input: {msg}"),
        }
    }
    fn at_end(&self) -> bool {
        self.i >= self.toks.len()
    }
    fn tok(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.i + k)
    }
    fn is_ident(&self, s: &str) -> bool {
        self.tok(0).is_some_and(|t| t.kind == TokKind::Ident && t.text == s)
    }
    fn peek_ident(&self, k: usize, s: &str) -> bool {
        self.tok(k).is_some_and(|t| t.kind == TokKind::Ident && t.text == s)
    }
    fn is_punct(&self, s: &str) -> bool {
        self.punct_at(0, s)
    }
    fn peek_is(&self, k: usize, s: &str) -> bool {
        self.tok(k).is_some_and(|t| t.text == s)
    }
    /// Whether the multi-character punctuation s (`::` / `->` / `>>=` etc.) follows from position k
    fn punct_at(&self, k: usize, s: &str) -> bool {
        // (punctuation is ASCII: compare bytes; this is called many times, once per operator precedence level)
        let sb = s.as_bytes();
        let n = sb.len();
        let mut j = 0;
        while j < n {
            let Some(t) = self.tok(k + j) else { return false };
            if t.kind != TokKind::Punct || t.text.as_bytes().first() != Some(&sb[j]) {
                return false;
            }
            if j + 1 < n && !t.joint {
                return false;
            }
            j += 1;
        }
        true
    }
    fn eat_punct(&mut self, s: &str) -> bool {
        if self.is_punct(s) {
            self.i += s.len();
            true
        } else {
            false
        }
    }
    fn expect_punct(&mut self, s: &str) -> R<()> {
        if self.eat_punct(s) {
            Ok(())
        } else {
            // (a missing `;` before a token on a later line that can begin a statement, or a `}`: right after the
            // previous token, where it goes, as rustc points: `}` then `let` on the next line. Not `,` / `:` / `.`)
            if let (true, Some(p), Some(t)) = (s == ";", self.i.checked_sub(1).and_then(|k| self.toks.get(k)), self.toks.get(self.i)) {
                let begins = match t.kind {
                    TokKind::Ident | TokKind::Lit | TokKind::Lifetime | TokKind::Open => true,
                    TokKind::Close => t.text == "}",
                    TokKind::Punct => matches!(t.text.as_str(), "-" | "!" | "*" | "&" | "|" | "||" | ".." | "<" | "::" | "&&"),
                };
                if p.line < t.line && begins && !p.text.contains('\n') {
                    let pos = self.shift(p.line, p.col + p.text.chars().count() as u32);
                    return Err(format!("{}:{}: expected `{s}` (found `{}`)", pos.line, pos.col + 1, t.text));
                }
            }
            Err(self.err(&format!("expected `{s}`")))
        }
    }
    fn eat_ident(&mut self, s: &str) -> bool {
        if self.is_ident(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn is_open(&self, c: &str) -> bool {
        self.tok(0).is_some_and(|t| t.kind == TokKind::Open && t.text == c)
    }
    fn expect_open(&mut self, c: &str) -> R<()> {
        if self.is_open(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected `{c}`")))
        }
    }
    fn is_close(&self) -> bool {
        self.tok(0).is_some_and(|t| t.kind == TokKind::Close)
    }
    fn expect_close(&mut self, c: &str) -> R<()> {
        if self.tok(0).is_some_and(|t| t.kind == TokKind::Close && t.text == c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected `{c}`")))
        }
    }
    /// A declared name (an item's, a field's, a variant's, a binding's): not a reserved keyword (the 2018 ones
    /// `async` / `await` / `dyn` / `try` stay names, as an edition 2015 file may use them)
    fn decl_ident(&mut self) -> R<String> {
        if let Some(t) = self.tok(0).filter(|t| t.kind == TokKind::Ident) {
            if reserved_kw(&t.text, true) {
                let k = t.text.clone();
                return Err(self.err(&format!("expected identifier, found keyword `{k}`")));
            }
            if unused_kw(&t.text) {
                let k = t.text.clone();
                return Err(self.err(&format!("expected identifier, found reserved keyword `{k}`")));
            }
        }
        self.ident()
    }
    fn ident(&mut self) -> R<String> {
        match self.tok(0) {
            Some(t) if t.kind == TokKind::Ident => {
                let s = t.text.clone();
                self.i += 1;
                Ok(s)
            }
            _ => Err(self.err("expected an identifier")),
        }
    }
    /// The position of the current token (line / column shifted by base: when re-reading a macro's arguments)
    fn pos(&self) -> Pos {
        match self.tok(0).or_else(|| self.toks.last()) {
            Some(t) => self.shift(t.line, t.col),
            None => self.base,
        }
    }

    fn shift(&self, line: u32, col: u32) -> Pos {
        if self.base.line == 0 {
            return Pos { line, col };
        }
        Pos { line: self.base.line + line - 1, col: if line == 1 { self.base.col + col } else { col } }
    }

    /// Turns a bracket pair (we are at the opening one) into text keeping the original line / column layout, and skips it (for expression macros).
    /// The opening bracket comes first: when re-reading, shifting positions from the opening bracket's position gives the original positions
    fn group_text_laid_out(&mut self) -> R<String> {
        let mut depth = 0;
        let mut s = String::new();
        let (mut line, mut col) = match self.tok(0) {
            Some(t) => (t.line, t.col),
            None => return Err(self.err("unclosed delimiter")),
        };
        let mut first = true;
        loop {
            let Some(t) = self.tok(0) else { return Err(self.err("unclosed delimiter")) };
            match t.kind {
                TokKind::Open => depth += 1,
                TokKind::Close => depth -= 1,
                _ => {}
            }
            if !first {
                if t.line > line {
                    for _ in line..t.line {
                        s.push('\n');
                    }
                    line = t.line;
                    // (from the second line on, the original column itself; only the first line has columns from the opening bracket)
                    for _ in 0..t.col {
                        s.push(' ');
                    }
                    col = t.col;
                } else {
                    // (the same line: pad with spaces up to the original column; on the first line, the difference from the opening bracket)
                    let want = t.col;
                    let mut c = col;
                    // (if the original column is already passed (tabs etc.), leave one space so they do not stick together)
                    if c > want {
                        s.push(' ');
                        c += 1;
                    }
                    while c < want {
                        s.push(' ');
                        c += 1;
                    }
                    col = c;
                }
            }
            first = false;
            s.push_str(&t.text);
            // (the line / column after a multi-line token (a string))
            let nl = t.text.bytes().filter(|&c| c == b'\n').count() as u32;
            if nl > 0 {
                line += nl;
                col = t.text.rsplit('\n').next().map(|x| x.chars().count() as u32).unwrap_or(0);
            } else {
                col += t.text.chars().count() as u32;
            }
            self.i += 1;
            if depth == 0 {
                break;
            }
        }
        Ok(s)
    }

    fn group_text(&mut self) -> R<String> {
        let mut depth = 0;
        let mut s = String::new();
        loop {
            let Some(t) = self.tok(0) else { return Err(self.err("unclosed delimiter")) };
            match t.kind {
                TokKind::Open => depth += 1,
                TokKind::Close => depth -= 1,
                _ => {}
            }
            if !s.is_empty() && !(t.kind == TokKind::Punct && self.i > 0 && self.toks[self.i - 1].joint) {
                s.push(' ');
            }
            s.push_str(&t.text);
            self.i += 1;
            if depth == 0 {
                return Ok(s);
            }
        }
    }
    /// The text of tokens a..b (same as proc_macro2's Display: one space between tokens, none after a Joint punctuation,
    /// brackets enclose their contents tightly)
    fn tokens_text(&self, a: usize, b: usize) -> String {
        join_tokens(&self.toks[a..b.min(self.toks.len())], self.toks.get(b))
    }

    fn attr(&mut self) -> R<String> {
        self.expect_punct("#")?;
        let inner = self.eat_punct("!");
        if !self.is_open("[") {
            return Err(self.err("expected `[`"));
        }
        let g = self.group_text()?;
        Ok(if inner { format!("#!{g}") } else { format!("#{g}") })
    }
    fn attrs(&mut self) -> R<Vec<String>> {
        let mut v = Vec::new();
        while self.is_punct("#") && !self.peek_is(1, "!") {
            v.push(self.attr()?);
        }
        Ok(v)
    }
    /// The outer attributes on an expression of a list (a call's argument, an array's element), the ones rustc takes
    /// there: lint levels, `doc`, `must_use` / `deprecated` (warnings), and `inline` / `cold` on a closure (itertools'
    /// `self.try_fold(init, #[inline(always)] |acc, v| ..)`). Another is an error at it: `#[track_caller]` on a closure
    /// is unstable, `#[target_feature]` / `#[no_mangle]` cannot be used on closures, `#[inline]` not on other expressions
    #[cold]
    fn expr_attrs(&mut self) -> R<()> {
        let mut names = Vec::new();
        while self.is_punct("#") && !self.peek_is(1, "!") {
            let at = self.i;
            let a = self.attr()?;
            let name: String = a[1..].trim_start_matches('[').trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':').collect();
            names.push((at, name));
        }
        let closure = self.is_punct("|") || self.is_ident("move") || (self.is_ident("async") && (self.peek_is(1, "|") || self.peek_is(1, "||") || self.peek_is(1, "move")));
        for (at, name) in names {
            let ok = matches!(name.as_str(), "allow" | "warn" | "deny" | "forbid" | "expect" | "doc" | "must_use" | "deprecated")
                || (closure && matches!(name.as_str(), "inline" | "cold"));
            if !ok {
                self.i = at;
                if closure && name == "track_caller" {
                    return Err(self.err_plain("`#[track_caller]` on closures is currently unstable (E0658)"));
                }
                return Err(self.err_plain(&format!("`#[{name}]` attribute cannot be used on {}", if closure { "closures" } else { "expressions" })));
            }
        }
        Ok(())
    }

    // ------------------------------------------------ Paths, types

    fn lifetime(&mut self) -> Option<String> {
        match self.tok(0) {
            Some(t) if t.kind == TokKind::Lifetime => {
                let s = t.text.clone();
                self.i += 1;
                Some(s)
            }
            _ => None,
        }
    }

    /// The contents of `<..>` (from after the opening `<`; eats up to the closing `>`)
    fn generic_args(&mut self) -> R<Vec<GArg>> {
        let mut v = Vec::new();
        loop {
            if self.eat_close_angle() {
                return Ok(v);
            }
            if let Some(l) = self.lifetime() {
                v.push(GArg::Lifetime(l));
            } else if self.tok(0).is_some_and(|t| t.kind == TokKind::Ident)
                && self.punct_at(1, "=")
                && !self.punct_at(1, "==")
            {
                let n = self.ident()?;
                self.expect_punct("=")?;
                v.push(GArg::Bind(n, self.ty()?));
            } else if self.tok(0).is_some_and(|t| t.kind == TokKind::Ident) && self.punct_at(1, ":") && !self.punct_at(1, "::") {
                // (an associated type bound `Iterator<Item: Debug>`: `Item = impl Debug`)
                let n = self.ident()?;
                self.expect_punct(":")?;
                v.push(GArg::Bind(n, Ty::Impl(self.bounds()?)));
            } else if self.tok(0).is_some_and(|t| t.kind == TokKind::Ident) && self.is_gat_binding() {
                // (a generic associated type's binding / bound, `Item<'a> = &'a T` / `Gat<'b>: Tr`: the arguments are
                // dropped)
                let n = self.ident()?;
                self.i += 1;
                let _ = self.generic_args()?;
                if self.eat_punct("=") {
                    v.push(GArg::Bind(n, self.ty()?));
                } else {
                    self.expect_punct(":")?;
                    v.push(GArg::Bind(n, Ty::Impl(self.bounds()?)));
                }
            } else if self.tok(0).is_some_and(|t| t.kind == TokKind::Lit) || self.is_punct("-") {
                v.push(GArg::Const(self.unary_expr(true)?));
            } else if self.is_open("{") {
                let b = self.block()?;
                v.push(GArg::Const(Expr::Block(None, false, b)));
            } else {
                v.push(GArg::Ty(self.ty()?));
            }
            if !self.eat_punct(",") {
                if self.eat_close_angle() {
                    return Ok(v);
                }
                return Err(self.err("expected `,` or `>`"));
            }
        }
    }

    /// At `Name<..>` followed by `=` or `:` (a generic associated type in generic arguments)
    fn is_gat_binding(&self) -> bool {
        if !self.punct_at(1, "<") || self.punct_at(1, "<=") {
            return false;
        }
        let mut depth = 0u32;
        let mut k = 1;
        while let Some(t) = self.tok(k) {
            if t.kind == TokKind::Punct {
                match t.text.as_str() {
                    "<" => depth += 1,
                    ">" if !(k > 0 && self.toks[self.i + k - 1].text == "-" && self.toks[self.i + k - 1].joint) => {
                        depth -= 1;
                        if depth == 0 {
                            return (self.punct_at(k + 1, "=") && !self.punct_at(k + 1, "==")) || (self.punct_at(k + 1, ":") && !self.punct_at(k + 1, "::"));
                        }
                    }
                    ";" => return false,
                    _ => {}
                }
            } else if t.kind == TokKind::Open && t.text == "{" {
                return false;
            }
            k += 1;
        }
        false
    }

    /// Eats one `>` (also the leading `>` of `>>` / `>=` / `>>=`)
    fn eat_close_angle(&mut self) -> bool {
        match self.toks.get(self.i) {
            Some(t) if t.kind == TokKind::Punct && t.text == ">" => {
                self.i += 1;
                true
            }
            _ => false,
        }
    }

    /// A path in an expression (`a::b::<T>::c`). Inside a type, `<..>` without `::` is taken too
    fn path(&mut self, in_ty: bool) -> R<Path> {
        let pos = self.pos();
        // (`::name::..` names the extern crate `name` (2018 and later): the crate's items are reached as `name::..`
        // is. Kept for `::crate` / `::self` / `::super`, which are errors (E0433). One name, `::f()` (2015: the crate
        // root's), is the item `f`, which is where the flattened program has it)
        let global = self.eat_punct("::") && self.tok(0).is_some_and(|t| matches!(t.text.as_str(), "crate" | "self" | "super" | "Self"));
        let mut segs = Vec::new();
        loop {
            let st = self.i;
            let name = self.ident()?;
            let mut seg = Seg { name: name, args: Vec::new(), fn_args: None, text: String::new() };
            // (a `<<` in a type is a `<` and the `<` of a qualified path: `Either<<&L as IntoIterator>::IntoIter, ..>`)
            if in_ty && self.is_punct("<") && !self.punct_at(0, "<=") {
                self.i += 1;
                seg.args = self.generic_args()?;
            } else if self.punct_at(0, "::") && self.punct_at(2, "<") {
                self.i += 3;
                seg.args = self.generic_args()?;
            } else if in_ty && self.is_open("(") && matches!(seg.name.as_str(), "Fn" | "FnMut" | "FnOnce" | "AsyncFn" | "AsyncFnMut" | "AsyncFnOnce") {
                self.i += 1;
                let mut ps = Vec::new();
                while !self.is_close() {
                    ps.push(self.ty()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_close(")")?;
                let ret = if self.eat_punct("->") { Some(Box::new(self.ty_no_bounds()?)) } else { None };
                seg.fn_args = Some((ps, ret));
            }
            seg.text = self.tokens_text(st, self.i);
            segs.push(seg);
            if self.punct_at(0, "::") && self.tok(2).is_some_and(|t| t.kind == TokKind::Ident) {
                if !segs[segs.len() - 1].args.is_empty() {
                    self.module_args(&segs, st)?;
                }
                self.i += 2;
                continue;
            }
            return Ok(Path { global: global, segs: segs, pos });
        }
    }

    /// E0109: `std::marker<i32>::Copy`, generic arguments on a module of std (a path of lowercase names from
    /// `std` / `core` / `alloc` that goes on). st: the segment's first token
    #[cold]
    fn module_args(&self, segs: &[Seg], st: usize) -> R<()> {
        let last = &segs[segs.len() - 1];
        if matches!(segs[0].name.as_str(), "std" | "core" | "alloc") && segs.iter().all(|s| s.name.starts_with(|c: char| c.is_ascii_lowercase())) {
            let kind = if last.args.iter().any(|a| matches!(a, GArg::Ty(_) | GArg::Const(_))) { "type" } else { "lifetime" };
            let t = &self.toks[st];
            return Err(format!("{}:{}: {kind} arguments are not allowed on module `{}` (E0109)", t.line, t.col + 1, last.name));
        }
        Ok(())
    }

    fn bounds(&mut self) -> R<Vec<Bound>> {
        let mut v = Vec::new();
        loop {
            let b = self.bounds_one()?;
            if b.is_empty() {
                break;
            }
            v.extend(b);
            if !self.eat_punct("+") {
                break;
            }
        }
        Ok(v)
    }

    /// One bound (none when no bound starts here)
    fn bounds_one(&mut self) -> R<Vec<Bound>> {
        let mut v = Vec::new();
        if let Some(l) = self.lifetime() {
            v.push(Bound::Lifetime(l));
        } else if self.is_open("(") {
            self.i += 1;
            let q = self.eat_punct("?");
            let mut binder = vec![];
            if self.eat_ident("for") {
                self.expect_punct("<")?;
                binder = self.generic_args()?;
            }
            let at = self.i;
            let p = self.path(true)?;
            self.hr_output_check(&binder, &p, at, None)?;
            v.push(Bound::Trait(q, p));
            self.expect_close(")")?;
        } else {
            let q = self.eat_punct("?");
            let mut binder = vec![];
            if self.eat_ident("for") {
                self.expect_punct("<")?;
                binder = self.generic_args()?;
            }
            if self.tok(0).is_some_and(|t| t.kind == TokKind::Ident) || self.is_punct("::") {
                let at = self.i;
                let p = self.path(true)?;
                self.hr_output_check(&binder, &p, at, None)?;
                v.push(Bound::Trait(q, p));
            }
        }
        Ok(v)
    }

    pub fn ty(&mut self) -> R<Ty> {
        self.ty_inner(true)
    }

    fn ty_no_bounds(&mut self) -> R<Ty> {
        self.ty_inner(false)
    }

    fn ty_inner(&mut self, plus: bool) -> R<Ty> {
        if self.eat_punct("!") {
            return Ok(Ty::Never);
        }
        // (a higher-ranked type `for<'a> fn(&'a u8)` / `for<'a> Fn(&'a T)`: the binder is dropped)
        if self.is_ident("for") && self.punct_at(1, "<") {
            self.i += 2;
            // (what rustc's parser takes there: `'a`, `'a:`, `'a: 'b + 'c`, a type parameter (rejected later); `,` alone
            // is a parse error)
            while !self.eat_close_angle() {
                if self.lifetime().is_some() {
                    if self.eat_punct(":") {
                        let at = self.i;
                        while self.lifetime().is_some() {
                            if !self.eat_punct("+") {
                                break;
                            }
                        }
                        if self.i > at {
                            self.defer(at, "bounds cannot be used in this context");
                        }
                    }
                } else if self.tok(0).is_some_and(|t| t.kind == TokKind::Ident) {
                    self.defer(self.i, "only lifetime parameters can be used in this context");
                    self.i += 1;
                } else {
                    return Err(self.err_plain("expected one of `#`, `>`, `const`, identifier, or lifetime"));
                }
                if !self.eat_punct(",") && !self.punct_at(0, ">") {
                    return Err(self.err("expected one of `,`, `:`, or `>`"));
                }
            }
            if self.is_ident("dyn") || self.is_ident("impl") {
                return Err(self.err("`for<...>` expected after `dyn` / `impl`, not before"));
            }
            return self.ty_inner(plus);
        }
        if self.is_punct("&") {
            // `&&T` is two &s
            if self.punct_at(0, "&&") {
                // (read the first &, and read from the second & on as the inner reference type)
                self.i += 1;
                self.toks[self.i].joint = false;
                let inner = self.ty_inner(plus)?;
                return Ok(Ty::Ref(None, false, Box::new(inner)));
            }
            self.i += 1;
            return self.ty_ref_rest().map(|t| t);
        }
        if self.eat_punct("*") {
            let m = if self.eat_ident("mut") {
                true
            } else if self.eat_ident("const") {
                false
            } else {
                // (`*T`, `**const T`: a raw pointer type says `const` or `mut`, rustc's error at the `*`)
                return Err(self.bare_pointer_err());
            };
            return Ok(Ty::Ptr(m, Box::new(self.ty_no_bounds()?)));
        }
        if self.is_open("[") {
            self.i += 1;
            let t = self.ty()?;
            if self.eat_punct(";") {
                let n = self.const_expr()?;
                self.expect_close("]")?;
                return Ok(Ty::Array(Box::new(t), Box::new(n)));
            }
            self.expect_close("]")?;
            return Ok(Ty::Slice(Box::new(t)));
        }
        if self.is_open("(") {
            self.i += 1;
            // (a macro call in a type position, expanded as `({ T })`)
            if self.is_open("{") {
                self.i += 1;
                let t = self.ty()?;
                self.expect_close("}")?;
                self.expect_close(")")?;
                return Ok(t);
            }
            let mut v = Vec::new();
            let mut comma = false;
            while !self.is_close() {
                v.push(self.ty()?);
                comma = self.eat_punct(",");
                if !comma {
                    break;
                }
            }
            self.expect_close(")")?;
            if v.len() == 1 && !comma {
                return Ok(v.pop().unwrap());
            }
            return Ok(Ty::Tuple(v));
        }
        if self.is_ident("_") {
            self.i += 1;
            return Ok(Ty::Infer);
        }
        if self.is_ident("impl") {
            self.i += 1;
            let bs = self.bounds()?;
            // (`impl Future<Output = T>`: futures run at once (see `async`), so the type is T)
            let out = bs.iter().find_map(|b| match b {
                Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name == "Future" || s.name == "IntoFuture") => {
                    p.segs.last().unwrap().args.iter().find_map(|a| match a {
                        GArg::Bind(n, t) if n == "Output" => Some(t.clone()),
                        _ => None,
                    })
                }
                _ => None,
            });
            if let Some(t) = out {
                return Ok(t);
            }
            return Ok(Ty::Impl(bs));
        }
        if self.is_ident("dyn") {
            self.i += 1;
            let bs = if plus || self.is_ident("for") || self.is_open("(") {
                // (`&dyn for<'a> Fn(&'a u8)` / `&dyn (Tr)`: one bound, `+` is not taken without `plus`)
                let mut bs = self.bounds_one()?;
                if plus && self.eat_punct("+") {
                    bs.extend(self.bounds()?);
                }
                bs
            } else {
                vec![Bound::Trait(false, self.path(true)?)]
            };
            // (`dyn Tr<Item: Bound>`)
            if !bs.iter().any(|b| matches!(b, Bound::Trait(..))) {
                return Err(self.err("at least one trait is required for an object type"));
            }
            let atb = bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Bind(_, Ty::Impl(_)))))));
            if atb {
                return Err(self.err("associated type bounds are not allowed in `dyn` types"));
            }
            // (`dyn Future<Output = T>` (+ Send ..): futures run at once, so behind `Box` / `Pin<Box<..>>` the type is T;
            // a marker type until the Box or Pin around it)
            let out = bs.iter().find_map(|b| match b {
                Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name == "Future") => p.segs.last().unwrap().args.iter().find_map(|a| match a {
                    GArg::Bind(n, t) if n == "Output" => Some(t.clone()),
                    _ => None,
                }),
                _ => None,
            });
            if let Some(t) = out {
                return Ok(Ty::Path(None, Path { global: false, segs: vec![Seg { name: "__exr_fut".into(), args: vec![GArg::Ty(t)], fn_args: None, text: String::new() }], pos: self.pos() }));
            }
            return Ok(Ty::Dyn(bs));
        }
        if self.is_ident("fn") || self.is_ident("unsafe") || self.is_ident("extern") {
            while self.eat_ident("unsafe") || self.eat_ident("extern") {
                if self.tok(0).is_some_and(|t| t.kind == TokKind::Lit) {
                    self.i += 1;
                }
            }
            if !self.eat_ident("fn") {
                return Err(self.err("expected `fn`"));
            }
            self.expect_open("(")?;
            let mut ps = Vec::new();
            while !self.is_close() {
                // (a C variadic function pointer, `extern "C" fn(u8, ...)`)
                if self.punct_at(0, "...") {
                    self.i += 3;
                    break;
                }
                if self.tok(0).is_some_and(|t| t.kind == TokKind::Ident) && self.punct_at(1, ":") && !self.punct_at(1, "::") {
                    self.i += 2;
                }
                ps.push(self.ty()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_close(")")?;
            let ret = if self.eat_punct("->") { self.ty_no_bounds()? } else { Ty::Tuple(Vec::new()) };
            return Ok(Ty::Fn(ps, Box::new(ret)));
        }
        if self.is_punct("<") {
            // `<T as Trait>::Name`
            self.i += 1;
            let q = self.ty()?;
            let mut path = Path { global: false, segs: Vec::new(), pos: self.pos() };
            if self.eat_ident("as") {
                let st = self.i;
                path = self.path(true)?;
                constraint_check(&self.toks, &path, st)?;
            }
            if !self.eat_close_angle() {
                return Err(self.err("expected `>`"));
            }
            while self.eat_punct("::") {
                let n = self.ident()?;
                // (`<T as Tr>::Gat<'a>`: the arguments of a generic associated type)
                let args = if self.eat_punct("<") { self.generic_args()? } else { Vec::new() };
                path.segs.push(Seg { name: n, args, fn_args: None, text: String::new() });
            }
            return Ok(Ty::Path(Some(Box::new(q)), path));
        }
        if self.tok(0).is_some_and(|t| t.kind == TokKind::Ident) || self.is_punct("::") {
            let p = self.path(true)?;
            if self.is_punct("!") && !self.punct_at(0, "!=") && self.tok(1).is_some_and(|t| t.kind == TokKind::Open) {
                self.i += 1;
                return Ok(Ty::Macro(self.group_text()?));
            }
            if plus && self.is_punct("+") && !self.punct_at(0, "+=") {
                let mut bs = vec![Bound::Trait(false, p)];
                self.i += 1;
                bs.extend(self.bounds()?);
                return Ok(Ty::Dyn(bs));
            }
            // (`Box<dyn Future<Output = T>>` / `Pin<Box<..>>`: T, see `dyn`)
            if let Some(t) = fut_output(&p) {
                return Ok(t);
            }
            return Ok(Ty::Path(None, p));
        }
        Err(self.err("expected a type"))
    }

    fn ty_ref_rest(&mut self) -> R<Ty> {
        let l = self.lifetime();
        let m = self.eat_ident("mut");
        Ok(Ty::Ref(l, m, Box::new(self.ty_no_bounds()?)))
    }

    // ------------------------------------------------ Patterns

    fn pat(&mut self) -> R<Pat> {
        self.eat_punct("|");
        let p0 = self.pos();
        let first = self.pat_one()?;
        if !self.is_punct("|") || self.punct_at(0, "||") {
            self.no_double_binding(&first)?;
            return Ok(first);
        }
        let mut v = vec![first];
        let mut at = vec![p0];
        while self.is_punct("|") && !self.punct_at(0, "||") {
            self.i += 1;
            at.push(self.pos());
            v.push(self.pat_one()?);
        }
        let p = Pat::Or(v, at);
        self.no_double_binding(&p)?;
        Ok(p)
    }

    /// E0416: a name bound twice in one pattern (`(x, x)`); the alternatives of `|` bind the same names
    fn no_double_binding(&self, p: &Pat) -> R<()> {
        if self.no_dup_check {
            return Ok(());
        }
        // (most patterns bind one name or none: no set for them)
        if matches!(p, Pat::Ident(_, _, _, None) | Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Path(_) | Pat::Range(..) | Pat::Macro(_)) {
            return Ok(());
        }
        let mut seen = crate::fxhash::FxSet::default();
        if let Some(n) = double_binding(p, &mut seen) {
            return Err(self.err(&format!("identifier `{n}` is bound more than once in the same pattern (E0416)")));
        }
        Ok(())
    }

    /// A closure parameter (`|` is used as the delimiter, so no or-patterns are taken)
    fn pat_no_or(&mut self) -> R<Pat> {
        self.pat_one()
    }

    fn pat_list(&mut self, close: &str) -> R<Vec<Pat>> {
        let mut v = Vec::new();
        while !self.is_close() {
            let p = self.pat()?;
            // (`[a @ 3.., ..]`: a nightly feature)
            if close == "]" {
                let sub = match &p {
                    Pat::Ident(_, _, _, Some(b)) => &**b,
                    p => p,
                };
                if matches!(sub, Pat::Range(Some(_), None, _)) {
                    return Err(self.err("`X..` patterns in slices are experimental"));
                }
            }
            v.push(p);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_close(close)?;
        Ok(v)
    }

    fn pat_range_end(&mut self) -> R<Option<Box<Expr>>> {
        if self.is_close() || self.is_punct(",") || self.is_punct("|") || self.is_punct("=") || self.is_ident("if") {
            return Ok(None);
        }
        Ok(Some(Box::new(self.pat_lit_expr()?)))
    }

    fn pat_lit_expr(&mut self) -> R<Expr> {
        let p0 = self.pos();
        if self.eat_punct("-") {
            let lp = self.pos();
            let t = self.tok(0).ok_or_else(|| self.err("expected a literal"))?.text.clone();
            self.i += 1;
            return Ok(Expr::Unary("-".to_string(), Box::new(Expr::Lit(t, lp)), Span { start: p0, at: p0 }));
        }
        if self.tok(0).is_some_and(|t| t.kind == TokKind::Lit) {
            let t = self.toks[self.i].text.clone();
            self.i += 1;
            return Ok(Expr::Lit(t, p0));
        }
        let p = self.path(false)?;
        Ok(Expr::Path(None, p))
    }

    fn range_pat(&self, lo: Option<Box<Expr>>, hi: Option<Box<Expr>>, incl: bool) -> R<Pat> {
        let is_str = |e: &Option<Box<Expr>>| matches!(e.as_deref(), Some(Expr::Lit(t, _)) if t.starts_with('"') || t.starts_with("b\"") || t.starts_with('r'));
        if is_str(&lo) || is_str(&hi) {
            return Err(self.err("only `char` and numeric types are allowed in range patterns"));
        }
        Ok(Pat::Range(lo, hi, incl))
    }

    fn pat_one(&mut self) -> R<Pat> {
        let p = self.pat_atom()?;
        // Ranges `a..=b` / `a..`
        if self.punct_at(0, "..=") || self.punct_at(0, "...") {
            let lo = match p {
                Pat::Lit(e) => e,
                Pat::Path(pa) => Box::new(Expr::Path(None, pa)),
                Pat::Ident(false, false, n, None) => Box::new(Expr::Path(None, Path { global: false, segs: vec![Seg { name: n, args: Vec::new(), fn_args: None, text: String::new() }], pos: Default::default() })),
                _ => return Err(self.err("bad range pattern")),
            };
            self.i += 3;
            let hi = self.pat_range_end()?;
            if hi.is_none() {
                return Err(no_end_err(&self.toks, self.i - 3));
            }
            return self.range_pat(Some(lo), hi, true);
        }
        if self.punct_at(0, "..") && !self.punct_at(0, "...") {
            let lo = match p {
                Pat::Lit(e) => e,
                Pat::Path(pa) => Box::new(Expr::Path(None, pa)),
                _ => return Ok(p),
            };
            self.i += 2;
            let hi = self.pat_range_end()?;
            return self.range_pat(Some(lo), hi, false);
        }
        Ok(p)
    }

    fn pat_atom(&mut self) -> R<Pat> {
        if self.is_ident("_") {
            self.i += 1;
            return Ok(Pat::Wild);
        }
        if self.punct_at(0, "..=") {
            self.i += 3;
            let hi = self.pat_lit_expr()?;
            return self.range_pat(None, Some(Box::new(hi)), true);
        }
        if self.punct_at(0, "..") {
            self.i += 2;
            // (a half-open range `..5` / `..-7` / `..MAX`; `..` alone is the rest)
            if self.tok(0).is_some_and(|t| t.kind == TokKind::Lit || (t.kind == TokKind::Ident && t.text != "if")) || self.is_punct("-") || self.punct_at(0, "::") {
                let hi = self.pat_lit_expr()?;
                return self.range_pat(None, Some(Box::new(hi)), false);
            }
            return Ok(Pat::Rest);
        }
        if self.is_punct("&") {
            if self.punct_at(0, "&&") {
                self.i += 1;
                self.toks[self.i].joint = false;
                // (read from the second `&`: `&&x` is Ref(Ref(x)))
                let inner = self.pat_atom()?;
                return Ok(Pat::Ref(false, Box::new(inner)));
            }
            self.i += 1;
            return self.pat_ref_rest();
        }
        if self.is_open("(") {
            self.i += 1;
            // (a macro call in a pattern position, expanded as `({ p })`)
            if self.is_open("{") {
                self.i += 1;
                let p = self.pat()?;
                self.expect_close("}")?;
                self.expect_close(")")?;
                return Ok(p);
            }
            let mut v = Vec::new();
            let mut comma = false;
            while !self.is_close() {
                v.push(self.pat()?);
                comma = self.eat_punct(",");
                if !comma {
                    break;
                }
            }
            self.expect_close(")")?;
            if v.len() == 1 && !comma {
                return Ok(v.pop().unwrap());
            }
            return Ok(Pat::Tuple(v));
        }
        if self.is_open("[") {
            self.i += 1;
            return Ok(Pat::Slice(self.pat_list("]")?));
        }
        if self.tok(0).is_some_and(|t| t.kind == TokKind::Lit) || self.is_punct("-") {
            return Ok(Pat::Lit(Box::new(self.pat_lit_expr()?)));
        }
        // (a qualified path to a constant, `<T as Tr>::C`)
        if self.is_punct("<") && !self.punct_at(0, "<<") {
            return Ok(Pat::Lit(Box::new(self.primary_expr(false)?)));
        }
        // (`true` / `false` are literal patterns)
        if (self.is_ident("true") || self.is_ident("false")) && !self.punct_at(1, "::") {
            let lp = self.pos();
            let t = self.toks[self.i].text.clone();
            self.i += 1;
            return Ok(Pat::Lit(Box::new(Expr::Lit(t, lp))));
        }
        if self.is_ident("ref") || self.is_ident("mut") {
            let r = self.eat_ident("ref");
            let m = self.eat_ident("mut");
            let n = self.decl_ident()?;
            let sub = if self.eat_punct("@") { Some(Box::new(self.pat_one()?)) } else { None };
            return Ok(Pat::Ident(r, m, n, sub));
        }
        let path = self.path(false)?;
        if self.is_punct("!") {
            self.i += 1;
            return Ok(Pat::Macro(self.group_text()?));
        }
        if self.is_open("(") {
            self.i += 1;
            return Ok(Pat::TupleStruct(path, self.pat_list(")")?));
        }
        if self.is_open("{") {
            self.i += 1;
            let mut fs = Vec::new();
            let mut rest = false;
            while !self.is_close() {
                if self.is_punct("#") {
                    self.attrs()?;
                }
                if self.punct_at(0, "..") {
                    self.i += 2;
                    rest = true;
                    break;
                }
                let r = self.eat_ident("ref");
                let m = self.eat_ident("mut");
                let n = self.decl_ident()?;
                // (`Foo { ref x: pat }`: a binding mode is the shorthand's, so no `:` follows it, rustc's "expected `,`"
                // at the `:`)
                if (r || m) && self.is_punct(":") && !self.punct_at(1, ":") {
                    return Err(self.err_plain("expected `,`"));
                }
                if self.eat_punct(":") {
                    fs.push((field_name(n), self.pat()?));
                } else {
                    fs.push((field_name(n.clone()), Pat::Ident(r, m, n, None)));
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_close("}")?;
            return Ok(Pat::Struct(path, fs, rest));
        }
        if path.segs.len() == 1 && !path.global && path.segs[0].args.is_empty() {
            let n = path.segs[0].name.clone();
            // (a keyword is no binding: `let extern = 0;`; `Self` / `self` are a unit struct and a receiver here)
            if reserved_kw(&n, false) {
                return Err(self.err(&format!("expected identifier, found keyword `{n}`")));
            }
            // (`let box = 0;`: `box` starts a box pattern, whose pattern is missing)
            if n == "box" {
                return Err(self.err("expected pattern"));
            }
            if unused_kw(&n) {
                return Err(self.err(&format!("expected identifier, found reserved keyword `{n}`")));
            }
            if self.eat_punct("@") {
                return Ok(Pat::Ident(false, false, n, Some(Box::new(self.pat_one()?))));
            }
            return Ok(Pat::Ident(false, false, n, None));
        }
        Ok(Pat::Path(path))
    }

    fn pat_ref_rest(&mut self) -> R<Pat> {
        let m = self.eat_ident("mut");
        Ok(Pat::Ref(m, Box::new(self.pat_atom()?)))
    }

    // ------------------------------------------------ Expressions

    /// ss: whether struct literals may be taken (not in the x of `if x {`)
    pub fn expr(&mut self, ss: bool) -> R<Expr> {
        self.assign_expr(ss)
    }

    fn assign_expr(&mut self, ss: bool) -> R<Expr> {
        let start = self.pos();
        let l = self.range_expr(ss)?;
        if !self.tok(0).is_some_and(|t| t.kind == TokKind::Punct) {
            return Ok(l);
        }
        if self.is_punct("=") && !self.punct_at(0, "==") && !self.punct_at(0, "=>") {
            self.i += 1;
            let r = self.assign_expr(ss)?;
            // (`_ = e;` too: `{ let _ = e; }`)
            if matches!(&l, Expr::Tuple(_) | Expr::Array(_)) || matches!(&l, Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "_") {
                if let Some(e) = destructure_assign(&l, &r, start) {
                    // (in parentheses: an assignment, not a block, so a statement of it still needs its `;`)
                    return Ok(Expr::Paren(Box::new(e)));
                }
            }
            return Ok(Expr::Assign(Box::new(l), Box::new(r)));
        }
        for op in ["<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|="] {
            if self.punct_at(0, op) {
                let at = self.pos();
                self.i += op.len();
                let r = self.assign_expr(ss)?;
                return Ok(Expr::AssignOp(op[..op.len() - 1].to_string(), Box::new(l), Box::new(r), Span { start, at }));
            }
        }
        Ok(l)
    }

    fn range_end_follows(&self) -> bool {
        match self.tok(0) {
            None => false,
            Some(t) => match t.kind {
                TokKind::Close => false,
                TokKind::Punct => matches!(t.text.as_str(), "-" | "*" | "!" | "&" | "|" | "<" | ":") && !(t.text == "=" ),
                // (`{` too: `(1..{ 2 })`; the callers do not take it where a block follows (`for i in 0.. {`))
                TokKind::Open => true,
                _ => !(t.kind == TokKind::Ident && t.text == "as"),
            },
        }
    }

    #[inline(always)]
    fn range_expr(&mut self, ss: bool) -> R<Expr> {
        if self.punct_at(0, "..") {
            let incl = self.punct_at(0, "..=");
            self.i += if incl { 3 } else { 2 };
            let hi = if self.range_end_follows() && !(self.is_open("{") && !ss) { Some(Box::new(self.bin_expr(0, ss)?)) } else { None };
            if incl && hi.is_none() {
                return Err(no_end_err(&self.toks, self.i - 3));
            }
            return Ok(Expr::Range(None, hi, incl));
        }
        let l = self.bin_expr(0, ss)?;
        if self.punct_at(0, "..") && !self.punct_at(0, "...") {
            let incl = self.punct_at(0, "..=");
            self.i += if incl { 3 } else { 2 };
            let ok = self.range_end_follows() && !(self.is_open("{") && !ss);
            if incl && !ok {
                return Err(no_end_err(&self.toks, self.i - 3));
            }
            let hi = if ok { Some(Box::new(self.bin_expr(0, ss)?)) } else { None };
            return Ok(Expr::Range(Some(Box::new(l)), hi, incl));
        }
        Ok(l)
    }

    /// The binary operator at the current position (None if there is none)
    fn bin_op(&self) -> Option<String> {
        // (not punctuation means not an operator: what follows an expression is usually `)` / `,` / `;` / an identifier)
        let t0 = self.tok(0)?;
        if t0.kind != TokKind::Punct {
            return None;
        }
        for op in ["||", "&&", "==", "!=", "<=", ">=", "<<", ">>"] {
            if self.punct_at(0, op) {
                // (`<<=` / `>>=` are assignments)
                if (op == "<<" || op == ">>") && self.punct_at(0, &format!("{op}=")) {
                    return None;
                }
                return Some(op.to_string());
            }
        }
        let t = self.tok(0)?;
        if t.kind != TokKind::Punct {
            return None;
        }
        let c = t.text.as_str();
        if !matches!(c, "+" | "-" | "*" | "/" | "%" | "^" | "&" | "|" | "<" | ">") {
            return None;
        }
        // (`+=` etc. are assignments)
        if t.joint && self.punct_at(1, "=") {
            return None;
        }
        Some(c.to_string())
    }

    fn bin_expr(&mut self, min: u32, ss: bool) -> R<Expr> {
        let start = self.pos();
        let mut l = self.cast_expr(ss)?;
        loop {
            let Some(op) = self.bin_op() else { break };
            let p = bin_prec(&op);
            if p <= min && !(p == 0) {
                break;
            }
            if p == 0 {
                break;
            }
            let at = self.pos();
            self.i += op.len();
            let r = self.bin_expr(p, ss)?;
            l = Expr::Binary(op, Box::new(l), Box::new(r), Span { start, at });
        }
        Ok(l)
    }

    /// The rest of a binary expression whose first operand is l (casts of l first)
    #[cold]
    #[inline(never)]
    fn bin_continue(&mut self, mut l: Expr, start: Pos) -> R<Expr> {
        while self.is_ident("as") {
            self.i += 1;
            let t = self.ty_no_bounds()?;
            l = Expr::Cast(Box::new(l), Box::new(t));
        }
        loop {
            let Some(op) = self.bin_op() else { break };
            let p = bin_prec(&op);
            if p == 0 {
                break;
            }
            let at = self.pos();
            self.i += op.len();
            let r = self.bin_expr(p, true)?;
            l = Expr::Binary(op, Box::new(l), Box::new(r), Span { start, at });
        }
        Ok(l)
    }

    fn cast_expr(&mut self, ss: bool) -> R<Expr> {
        let mut e = self.unary_expr(ss)?;
        while self.is_ident("as") {
            self.i += 1;
            let t = self.ty_no_bounds()?;
            e = Expr::Cast(Box::new(e), Box::new(t));
        }
        Ok(e)
    }

    fn unary_expr(&mut self, ss: bool) -> R<Expr> {
        if self.is_punct("-") || self.is_punct("!") || (self.is_punct("*") && !self.punct_at(0, "*=")) {
            let p0 = self.pos();
            let op = self.toks[self.i].text.clone();
            self.i += 1;
            let e = self.unary_expr(ss)?;
            return Ok(Expr::Unary(op, Box::new(e), Span { start: p0, at: p0 }));
        }
        if self.is_punct("&") {
            if self.punct_at(0, "&&") {
                // `&&x` is two &s
                self.i += 1;
                self.toks[self.i].joint = false;
                let inner = self.unary_expr(ss)?;
                return Ok(Expr::Ref(false, Box::new(inner)));
            }
            self.i += 1;
            // (`&..` / `&..0`: a reference to a range)
            if self.tok(0).is_some_and(|t| t.text.as_bytes()[0] == b'.') && self.punct_at(0, "..") {
                return self.ref_range(ss);
            }
            if self.is_ident("raw") && (self.peek_ident(1, "const") || self.peek_ident(1, "mut")) {
                self.i += 1;
                let m = self.eat_ident("mut");
                if !m {
                    self.eat_ident("const");
                }
                return Ok(Expr::RawRef(m, Box::new(self.unary_expr(ss)?)));
            }
            let m = self.eat_ident("mut");
            let e = self.unary_expr(ss)?;
            return Ok(Expr::Ref(m, Box::new(e)));
        }
        self.postfix_expr(ss)
    }

    #[cold]
    #[inline(never)]
    fn ref_range(&mut self, ss: bool) -> R<Expr> {
        Ok(Expr::Ref(false, Box::new(self.range_expr(ss)?)))
    }

    fn call_args(&mut self) -> R<Vec<Expr>> {
        self.expect_open("(")?;
        let mut v = Vec::new();
        while !self.is_close() {
            // (an argument's outer attributes, `f(x, #[inline] |a| a)`: rustc takes them in a list of expressions)
            if self.is_punct("#") {
                self.expr_attrs()?;
            }
            v.push(self.expr(true)?);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_close(")")?;
        Ok(v)
    }

    fn postfix_expr(&mut self, ss: bool) -> R<Expr> {
        let start = self.pos();
        let e = self.primary_expr(ss)?;
        self.postfix_rest(e, start)
    }

    fn postfix_rest(&mut self, e: Expr, start: Pos) -> R<Expr> {
        let mut e = e;
        loop {
            if self.is_punct("?") {
                self.i += 1;
                e = Expr::Try(Box::new(e));
            } else if self.is_punct(".") && !self.punct_at(0, "..") {
                self.i += 1;
                if self.eat_ident("await") {
                    if self.async_on && !self.in_async {
                        // (at the `await`, as rustc: not the token after it)
                        let t = &self.toks[self.i - 1];
                        return Err(format!("{}:{}: `await` is only allowed inside `async` functions and blocks (E0728)", t.line, t.col + 1));
                    }
                    continue;
                }
                let t = self.tok(0).ok_or_else(|| self.err("expected a field"))?.clone();
                let fpos = self.pos();
                if t.kind == TokKind::Lit {
                    // `x.0` / `x.0.1` (the lexer makes 0.1 a single number)
                    self.i += 1;
                    for part in t.text.split('.') {
                        // (`t.0. 0`: the literal `0.` ends with the next field's dot)
                        if part.is_empty() {
                            if self.tok(0).is_some_and(|n| n.kind == TokKind::Lit && n.text.bytes().all(|c| c.is_ascii_digit())) {
                                let n = self.toks[self.i].text.clone();
                                self.i += 1;
                                e = Expr::Field(Box::new(e), n, Span { start, at: fpos });
                            }
                            continue;
                        }
                        e = Expr::Field(Box::new(e), part.to_string(), Span { start, at: fpos });
                    }
                    continue;
                }
                let at = self.pos();
                let name = self.ident()?;
                let mut targs = Vec::new();
                if self.punct_at(0, "::") && self.punct_at(2, "<") {
                    self.i += 3;
                    targs = self.generic_args()?;
                }
                if self.is_open("(") {
                    let args = self.call_args()?;
                    e = Expr::MethodCall(Box::new(e), name, targs, args, Span { start, at });
                } else {
                    e = Expr::Field(Box::new(e), name, Span { start, at });
                }
            } else if self.is_open("(") {
                let args = self.call_args()?;
                e = Expr::Call(Box::new(e), args);
            } else if self.is_open("[") {
                let at = self.pos();
                self.i += 1;
                let ix = self.expr(true)?;
                self.expect_close("]")?;
                e = Expr::Index(Box::new(e), Box::new(ix), Span { start, at });
            } else {
                break;
            }
        }
        Ok(e)
    }

    /// rustc's `eat_label`: a lifetime where a label goes (`'static: loop`, `break '_`) whose name is a keyword, at it
    fn keyword_label(&self) -> Option<String> {
        let t = self.tok(0).filter(|t| t.kind == TokKind::Lifetime && !t.joint)?;
        let n = &t.text[1..];
        (n == "_" || reserved_kw(n, true) || unused_kw(n)).then(|| format!("{}:{}: labels cannot use keyword names", t.line, t.col + 1))
    }

    fn label(&mut self) -> Option<String> {
        if self.tok(0).is_some_and(|t| t.kind == TokKind::Lifetime) && self.punct_at(1, ":") && !self.punct_at(1, "::") {
            let l = self.toks[self.i].text.clone();
            self.i += 2;
            return Some(l);
        }
        None
    }

    /// The condition of `if` / `while` (including `let p = e` and chains with `&&`)
    fn cond(&mut self) -> R<Expr> {
        // (for `let p = e`, primary_expr takes up to what binds tighter than &&)
        self.expr(false)
    }

    /// rustc's error for a condition (or an iterator) that took the block meant to follow it, at the keyword `kw`
    /// (the token index): `if {}` / `for x in {}` (bare: no condition), `if 5 == { .. }` (unfinished). None when a
    /// block follows, or the expression does not end with one
    fn swallowed_block(&self, kw: usize, e: &Expr, bare: &str, unfinished: Option<&str>) -> Option<String> {
        fn ends_with_block(e: &Expr) -> bool {
            match e {
                Expr::Block(None, false, _) => true,
                Expr::Binary(_, _, r, _) => ends_with_block(r),
                Expr::Unary(_, x, _) | Expr::Let(_, x) => ends_with_block(x),
                _ => false,
            }
        }
        if self.is_open("{") {
            return None;
        }
        let msg = match e {
            Expr::Block(None, false, _) => bare,
            e if ends_with_block(e) => unfinished?,
            _ => return None,
        };
        let t = self.toks.get(kw)?;
        Some(format!("{}:{}: {msg}", t.line, t.col + 1))
    }

    fn if_expr(&mut self) -> R<Expr> {
        let kw = self.i.saturating_sub(1);
        let c = self.cond()?;
        if let Some(m) = self.swallowed_block(kw, &c, "missing condition for `if` expression", Some("this `if` expression is missing a block after the condition")) {
            return Err(m);
        }
        let then = self.block()?;
        let els = if self.eat_ident("else") {
            if self.eat_ident("if") {
                Some(Box::new(self.if_expr()?))
            } else {
                let b = self.block()?;
                Some(Box::new(Expr::Block(None, false, b)))
            }
        } else {
            None
        };
        Ok(Expr::If(Box::new(c), then, els))
    }

    fn closure(&mut self) -> R<Expr> {
        // (an async closure's body may `.await`; a plain closure's may not, even inside an async block)
        let is_async = self.eat_ident("async");
        let saved = std::mem::replace(&mut self.in_async, is_async);
        let saved_c = std::mem::replace(&mut self.in_const, false);
        let r = self.closure_inner();
        self.in_async = saved;
        self.in_const = saved_c;
        r
    }

    /// Keep the first validation error, at token `at` (see `deferred`)
    #[cold]
    fn defer(&mut self, at: usize, msg: &str) {
        if self.deferred.is_none() {
            if let Some(t) = self.toks.get(at) {
                self.deferred = Some(format!("{}:{}: {msg}", t.line, t.col + 1));
            }
        }
    }

    /// An expression that must be a constant (E0572 for a `return` in it)
    fn const_expr(&mut self) -> R<Expr> {
        let o = std::mem::replace(&mut self.in_const, true);
        let e = self.expr(true);
        self.in_const = o;
        e
    }

    fn closure_inner(&mut self) -> R<Expr> {
        let start = self.pos();
        let mv = self.eat_ident("move");
        let mut ps = Vec::new();
        if self.punct_at(0, "||") {
            self.i += 2;
        } else {
            self.expect_punct("|")?;
            while !self.is_punct("|") {
                let p = self.pat_no_or()?;
                let t = if self.eat_punct(":") { Some(self.ty_no_bounds()?) } else { None };
                ps.push((p, t));
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct("|")?;
        }
        let ret = if self.eat_punct("->") { Some(Box::new(self.ty_no_bounds()?)) } else { None };
        let body = if ret.is_some() {
            let b = self.block()?;
            Expr::Block(None, false, b)
        } else {
            self.expr(true)?
        };
        Ok(Expr::Closure(mv, ps, ret, Box::new(body), start))
    }

    fn primary_expr(&mut self, ss: bool) -> R<Expr> {
        let Some(t) = self.tok(0) else { return Err(self.err("expected an expression")) };
        let t = t.clone();
        // `async { .. }` / `async move { .. }`: run at once (no executor: `.await` is the value itself), as a
        // closure called on the spot so that `return` / `?` leave the block
        if t.kind == TokKind::Ident && t.text == "async" && (self.peek_is(1, "{") || (self.peek_ident(1, "move") && self.peek_is(2, "{"))) {
            let start = self.pos();
            self.i += 1;
            let mv = self.eat_ident("move");
            let saved = std::mem::replace(&mut self.in_async, true);
            let b = self.block();
            self.in_async = saved;
            let b = b?;
            let c = Expr::Closure(mv, Vec::new(), None, Box::new(Expr::Block(None, false, b)), start);
            return Ok(Expr::Call(Box::new(Expr::Paren(Box::new(c))), Vec::new()));
        }
        match t.kind {
            TokKind::Lit => {
                let lp = self.pos();
                // (most number literals are one digit or digits only: no suffix to look at)
                let tb = t.text.as_bytes();
                let suffixed = tb.len() > 1 && tb[0].is_ascii_digit() && tb.iter().any(|c| c.is_ascii_alphabetic());
                if let Some(sfx) = if suffixed { bad_num_suffix(&t.text) } else { None } {
                    let base = match t.text.get(..2) {
                        Some("0b") => 2,
                        Some("0o") => 8,
                        _ => 0,
                    };
                    // (`0b121`: a digit the base does not have; `0XFF`: an uppercase base prefix before digits of it)
                    let upper = t.text.get(..2).and_then(|p| match p {
                        "0X" => Some(16),
                        "0O" => Some(8),
                        "0B" => Some(2),
                        _ => None,
                    });
                    let msg = if base > 0 && sfx.starts_with(|c: char| c.is_ascii_digit()) {
                        format!("invalid digit for a base {base} literal")
                    } else if upper.is_some_and(|r| !t.text[2..].is_empty() && t.text[2..].chars().all(|c| c == '_' || c.to_digit(r).is_some())) {
                        "invalid base prefix for number literal".to_string()
                    } else {
                        format!("invalid suffix `{sfx}` for number literal")
                    };
                    return Err(self.err(&msg));
                }
                self.i += 1;
                return Ok(Expr::Lit(t.text, lp));
            }
            TokKind::Lifetime => {
                if let Some(e) = self.keyword_label() {
                    return Err(e);
                }
                let l = self.label();
                if self.eat_ident("loop") {
                    return Ok(Expr::Loop(l, self.block()?));
                }
                if self.eat_ident("while") {
                    let c = self.cond()?;
                    return Ok(Expr::While(l, Box::new(c), self.block()?));
                }
                if self.eat_ident("for") {
                    return self.for_rest(l);
                }
                if self.is_open("{") {
                    return Ok(Expr::Block(l, false, self.block()?));
                }
                return Err(self.err("expected a loop after the label"));
            }
            TokKind::Open => {
                if t.text == "(" {
                    self.i += 1;
                    let mut v = Vec::new();
                    let mut comma = false;
                    while !self.is_close() {
                        // (`(#[allow(x)] e)`: an expression fragment of a macro with its attribute)
                        if self.is_punct("#") {
                            self.attrs()?;
                        }
                        v.push(self.expr(true)?);
                        comma = self.eat_punct(",");
                        if !comma {
                            break;
                        }
                    }
                    self.expect_close(")")?;
                    if v.len() == 1 && !comma {
                        return Ok(Expr::Paren(Box::new(v.pop().unwrap())));
                    }
                    return Ok(Expr::Tuple(v));
                }
                if t.text == "[" {
                    self.i += 1;
                    if self.is_close() {
                        self.expect_close("]")?;
                        return Ok(Expr::Array(Vec::new()));
                    }
                    // (an element's outer attributes, `[#[allow(x)] a, b]`, as in a call's arguments)
                    if self.is_punct("#") {
                        self.expr_attrs()?;
                    }
                    let first = self.expr(true)?;
                    if self.eat_punct(";") {
                        let n = self.const_expr()?;
                        self.expect_close("]")?;
                        return Ok(Expr::Repeat(Box::new(first), Box::new(n)));
                    }
                    let mut v = vec![first];
                    while self.eat_punct(",") {
                        if self.is_close() {
                            break;
                        }
                        if self.is_punct("#") {
                            self.expr_attrs()?;
                        }
                        v.push(self.expr(true)?);
                    }
                    self.expect_close("]")?;
                    return Ok(Expr::Array(v));
                }
                return Ok(Expr::Block(None, false, self.block()?));
            }
            TokKind::Punct => {
                if t.text == "|" {
                    return self.closure();
                }
                if t.text == "<" {
                    // `<T>::f` / `<T as Tr>::f`
                    self.i += 1;
                    let q = self.ty()?;
                    let mut path = Path { global: false, segs: Vec::new(), pos: self.pos() };
                    let mut had_as = None;
                    if self.eat_ident("as") {
                        let st = self.i;
                        path = self.path(true)?;
                        constraint_check(&self.toks, &path, st)?;
                        had_as = Some(());
                    }
                    if !self.eat_close_angle() {
                        return Err(self.err("expected `>`"));
                    }
                    while self.eat_punct("::") {
                        let n = self.ident()?;
                        let mut seg = Seg { name: n, args: Vec::new(), fn_args: None, text: String::new() };
                        if self.punct_at(0, "::") && self.punct_at(2, "<") {
                            self.i += 3;
                            seg.args = self.generic_args()?;
                        }
                        path.segs.push(seg);
                    }
                    // (`<u16>::from_be_bytes` / `<S>::f`, no trait: the path `u16::from_be_bytes`)
                    if let (Ty::Path(None, qp), true) = (&q, had_as.is_none()) {
                        if !qp.global && qp.segs.len() == 1 && !path.segs.is_empty() {
                            let mut segs = qp.segs.clone();
                            segs.extend(path.segs);
                            return Ok(Expr::Path(None, Path { global: false, segs, pos: qp.pos }));
                        }
                    }
                    return Ok(Expr::Path(Some(Box::new(q)), path));
                }
                if t.text == ":" && self.punct_at(0, "::") {
                    return self.path_expr(ss);
                }
                return Err(self.err("expected an expression"));
            }
            TokKind::Close => return Err(self.err("expected an expression")),
            TokKind::Ident => {}
        }
        match t.text.as_str() {
            "true" | "false" => {
                let lp = self.pos();
                self.i += 1;
                return Ok(Expr::Lit(t.text, lp));
            }
            "if" => {
                self.i += 1;
                return self.if_expr();
            }
            "match" => {
                self.i += 1;
                let e = self.expr(false)?;
                self.expect_open("{")?;
                let mut arms = Vec::new();
                while !self.is_close() {
                    self.attrs()?;
                    let apos = self.pos();
                    let pat = self.pat()?;
                    let guard = if self.eat_ident("if") { Some(self.expr(true)?) } else { None };
                    self.expect_punct("=>")?;
                    let body = if self.blocky_start() {
                        let start = self.pos();
                        let e = self.primary_expr(true)?;
                        if self.is_punct(".") || self.is_punct("?") { self.postfix_rest(e, start)? } else { e }
                    } else {
                        self.expr(true)?
                    };
                    let blocky = matches!(body, Expr::Block(..) | Expr::If(..) | Expr::Match(..) | Expr::Loop(..) | Expr::While(..) | Expr::For(..) | Expr::Const(..));
                    arms.push(Arm { pat: pat, pos: apos, guard: guard, body: body });
                    if !self.eat_punct(",") && !blocky && !self.is_close() {
                        return Err(self.err("expected `,` after a match arm"));
                    }
                }
                self.expect_close("}")?;
                return Ok(Expr::Match(Box::new(e), arms));
            }
            "while" => {
                self.i += 1;
                let c = self.cond()?;
                return Ok(Expr::While(None, Box::new(c), self.block()?));
            }
            "loop" => {
                self.i += 1;
                return Ok(Expr::Loop(None, self.block()?));
            }
            "for" => {
                self.i += 1;
                return self.for_rest(None);
            }
            "unsafe" => {
                self.i += 1;
                return Ok(Expr::Block(None, true, self.block()?));
            }
            "const" if self.tok(1).is_some_and(|t| t.kind == TokKind::Open && t.text == "{") => {
                self.i += 1;
                return Ok(Expr::Const(self.block()?));
            }
            "move" => return self.closure(),
            // (`async |x| ..` / `async move || ..`)
            "async" if self.peek_is(1, "|") || self.peek_is(1, "||") || (self.peek_ident(1, "move") && (self.peek_is(2, "|") || self.peek_is(2, "||"))) => {
                return self.closure();
            }
            "return" => {
                if self.in_const {
                    return Err(self.err_plain("return statement outside of function body (E0572)"));
                }
                let kw = self.pos();
                self.i += 1;
                let v = if self.expr_follows() { Some(Box::new(self.expr(ss)?)) } else { None };
                return Ok(Expr::Return(v, kw));
            }
            "break" => {
                let kw = self.pos();
                self.i += 1;
                if let Some(e) = self.keyword_label() {
                    return Err(e);
                }
                let lp = self.pos();
                let l = self.lifetime();
                let pos = if l.is_some() { lp } else { kw };
                let v = if self.expr_follows() && !(self.is_open("{") && !ss) { Some(Box::new(self.expr(ss)?)) } else { None };
                return Ok(Expr::Break(l, v, pos));
            }
            "continue" => {
                let kw = self.pos();
                self.i += 1;
                if let Some(e) = self.keyword_label() {
                    return Err(e);
                }
                let lp = self.pos();
                let l = self.lifetime();
                return Ok(Expr::Continue(l.clone(), if l.is_some() { lp } else { kw }));
            }
            "let" => {
                self.i += 1;
                let p = self.pat()?;
                self.expect_punct("=")?;
                // (a struct literal only where the caller takes one: a match guard `if let P = S { .. } =>`)
                let e = self.bin_expr(4, ss)?;
                return Ok(Expr::Let(Box::new(p), Box::new(e)));
            }
            _ => {}
        }
        self.path_expr(ss)
    }

    fn expr_follows(&self) -> bool {
        match self.tok(0) {
            None => false,
            Some(t) => match t.kind {
                TokKind::Close => false,
                TokKind::Punct => !matches!(t.text.as_str(), ";" | "," | "=" ) || t.text == "-" || t.text == "!" || t.text == "*" || t.text == "&" || t.text == "|",
                _ => true,
            },
        }
    }

    fn for_rest(&mut self, l: Option<String>) -> R<Expr> {
        let kw = self.i.saturating_sub(1);
        let p = self.pat()?;
        if !self.eat_ident("in") {
            return Err(self.err("expected `in`"));
        }
        let e = self.expr(false)?;
        if let Some(m) = self.swallowed_block(kw, &e, "missing expression to iterate on in `for` loop", None) {
            return Err(m);
        }
        let b = self.block()?;
        Ok(Expr::For(l, Box::new(p), Box::new(e), b))
    }

    fn path_expr(&mut self, ss: bool) -> R<Expr> {
        let start = self.pos();
        let p = self.path(false)?;
        if self.is_punct("!") && !self.punct_at(0, "!=") {
            self.i += 1;
            if !self.tok(0).is_some_and(|t| t.kind == TokKind::Open) {
                return Err(self.err("expected one of `(`, `[`, or `{`"));
            }
            let at = self.pos();
            let g = self.group_text_laid_out()?;
            return Ok(Expr::Macro(p, g, Span { start, at }));
        }
        if ss && self.is_open("{") && self.struct_lit_follows() {
            self.i += 1;
            let mut fs = Vec::new();
            let mut base = None;
            while !self.is_close() {
                if self.is_punct("#") {
                    self.attrs()?;
                }
                if self.punct_at(0, "..") {
                    self.i += 2;
                    base = Some(Box::new(self.expr(true)?));
                    break;
                }
                let at = self.pos();
                let n = match self.tok(0) {
                    Some(t) if t.kind == TokKind::Lit => {
                        let s = t.text.clone();
                        self.i += 1;
                        s
                    }
                    _ => self.ident()?,
                };
                if self.eat_punct(":") {
                    fs.push((field_name(n), self.expr(true)?));
                } else {
                    // (shorthand `S { t }`: the local `t`, at the field's name)
                    let e = Expr::Path(None, Path { global: false, segs: vec![Seg { name: n.clone(), args: Vec::new(), fn_args: None, text: String::new() }], pos: at });
                    fs.push((field_name(n), e));
                }
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_close("}")?;
            return Ok(Expr::Struct(p, fs, base));
        }
        Ok(Expr::Path(None, p))
    }

    /// Whether what follows `Path {` looks like a struct literal (`{}` / `{ x` + `:` `,` `}` / `{ ..`)
    fn struct_lit_follows(&self) -> bool {
        match self.tok(1) {
            Some(t) if t.kind == TokKind::Close => true,
            Some(t) if t.kind == TokKind::Ident || t.kind == TokKind::Lit => {
                self.punct_at(2, ":") && !self.punct_at(2, "::") || self.peek_is(2, ",") || self.tok(2).is_some_and(|x| x.kind == TokKind::Close)
            }
            Some(_) => self.punct_at(1, "..") || (self.punct_at(1, "#") && self.tok(2).is_some_and(|t| t.kind == TokKind::Open && t.text == "[")),
            None => false,
        }
    }

    // ------------------------------------------------ Statements, blocks

    pub fn block(&mut self) -> R<Block> {
        self.expect_open("{")?;
        let mut stmts = Vec::new();
        // (the statements' starts, kept only for a block with a statement whose expression has no position of its own:
        // `loop` / `unsafe` / `const` / `async` blocks, a block, `return;`, `[]`, `()`, `match () {}`. Most blocks have
        // none, and allocate nothing)
        let mut spos: Vec<(u32, Pos)> = Vec::new();
        while !self.is_close() {
            if self.at_end() {
                return Err(self.err("unclosed block"));
            }
            if self.eat_punct(";") {
                continue;
            }
            // (an inner attribute at the start of the body, `#![allow(x)]`)
            if stmts.is_empty() && self.is_punct("#") && self.peek_is(1, "!") && self.tok(2).is_some_and(|t| t.kind == TokKind::Open && t.text == "[") {
                self.attr()?;
                continue;
            }
            // (`let (a, mut b);` / `let (a, b): (T, U);`: a `let` for each name, without a value yet. Only a `let (`
            // is looked at: moving every statement once more costs a copy of it)
            let need = match self.tok(0) {
                Some(t) if t.kind == TokKind::Ident => match t.text.as_str() {
                    "loop" | "unsafe" | "const" | "async" => true,
                    "return" => self.peek_is(1, ";") || self.tok(1).is_some_and(|t| t.kind == TokKind::Close),
                    "match" => self.peek_is(1, "(") && self.tok(2).is_some_and(|t| t.kind == TokKind::Close),
                    // (`for _ in ..10`, `for x in mac!(..)`: an iterable that may have none either; `while {} {}`)
                    "for" => true,
                    "while" | "if" => self.peek_is(1, "{"),
                    _ => false,
                },
                // (`&mut if c { a } else { b }`: a reference has no position of its own, nor has what it borrows here)
                Some(t) if t.kind == TokKind::Punct && (t.text == "&" || t.text == "&&") => {
                    let k = if self.tok(1).is_some_and(|t| t.kind == TokKind::Ident && t.text == "mut") { 2 } else { 1 };
                    self.tok(k).is_some_and(|t| t.kind == TokKind::Ident && matches!(t.text.as_str(), "if" | "match" | "loop" | "unsafe" | "while" | "for"))
                }
                // (`{ .. }`, `[]`, `()`, `(return)(..)`)
                Some(t) if t.kind == TokKind::Open => {
                    t.text == "{"
                        || self.tok(1).is_some_and(|t| t.kind == TokKind::Close || (t.kind == TokKind::Ident && matches!(t.text.as_str(), "return" | "loop" | "break" | "match" | "unsafe")))
                }
                _ => false,
            };
            if need {
                spos.push((stmts.len() as u32, self.pos()));
            }
            if !(self.is_ident("let") && self.peek_is(1, "(")) {
                stmts.push(self.stmt()?);
                continue;
            }
            let st = self.stmt()?;
            if let Stmt::Let(Pat::Tuple(ps), ann, None, None, lp) = &st {
                if ps.iter().all(|p| matches!(p, Pat::Ident(false, _, _, None) | Pat::Wild))
                    && ann.as_ref().is_none_or(|t| matches!(t, Ty::Tuple(ts) if ts.len() == ps.len()))
                {
                    for (k, p) in ps.iter().enumerate() {
                        if matches!(p, Pat::Wild) {
                            continue;
                        }
                        let t = match ann {
                            Some(Ty::Tuple(ts)) => Some(ts[k].clone()),
                            _ => None,
                        };
                        stmts.push(Stmt::Let(p.clone(), t, None, None, *lp));
                    }
                    continue;
                }
            }
            stmts.push(st);
        }
        self.expect_close("}")?;
        Ok(Block { stmts, spos: Spos::of(spos) })
    }

    fn item_follows(&self) -> bool {
        let mut k = 0;
        if self.peek_ident(0, "pub") {
            k = 1;
            if self.tok(1).is_some_and(|t| t.kind == TokKind::Open && t.text == "(") {
                return true;
            }
        }
        let Some(t) = self.tok(k) else { return false };
        if t.kind != TokKind::Ident {
            return false;
        }
        match t.text.as_str() {
            "fn" | "struct" | "enum" | "trait" | "impl" | "mod" | "use" | "static" | "type" | "extern" | "union" => {
                !(t.text == "union" && !self.tok(k + 1).is_some_and(|x| x.kind == TokKind::Ident))
            }
            "const" => self.tok(k + 1).is_some_and(|x| x.kind == TokKind::Ident && x.text != "move") || self.peek_is(k + 1, "_"),
            "unsafe" => self.peek_ident(k + 1, "fn") || self.peek_ident(k + 1, "impl") || self.peek_ident(k + 1, "trait") || self.peek_ident(k + 1, "extern"),
            "async" => self.peek_ident(k + 1, "fn"),
            "macro_rules" => self.punct_at(k + 1, "!"),
            _ => false,
        }
    }

    /// Whether it starts with a block-like expression (a statement / match arm ends there)
    fn blocky_start(&self) -> bool {
        self.is_open("{")
            || self.tok(0).is_some_and(|t| t.kind == TokKind::Lifetime)
            || ["if", "match", "loop", "while", "for"].iter().any(|k| self.is_ident(k))
            || (self.tok(1).is_some_and(|t| t.kind == TokKind::Open && t.text == "{") && (self.is_ident("unsafe") || self.is_ident("const")))
    }

    fn stmt(&mut self) -> R<Stmt> {
        let attrs = self.attrs()?;
        if self.item_follows() {
            let spos = self.pos();
            let mut it = self.item()?;
            let mut a = attrs;
            a.extend(it.attrs);
            it.attrs = a;
            // (a `const` / `static` in a body (often from a macro): an immutable binding of its value)
            match it.kind {
                ItemKind::Const(n, t, Some(e)) => {
                    let p = if n == "_" { Pat::Wild } else { Pat::Ident(false, false, n, None) };
                    return Ok(Stmt::Let(p, t, Some(e), None, spos));
                }
                ItemKind::Static(false, n, t, e) => return Ok(Stmt::Let(Pat::Ident(false, false, n, None), Some(t), Some(e), None, spos)),
                k => it.kind = k,
            }
            return Ok(Stmt::Item(it));
        }
        if self.is_ident("let") {
            let spos = self.pos();
            self.i += 1;
            let p = self.pat()?;
            let t = if self.eat_punct(":") { Some(self.ty()?) } else { None };
            let init = if self.eat_punct("=") { Some(self.expr(true)?) } else { None };
            let els = if self.eat_ident("else") { Some(self.block()?) } else { None };
            self.expect_punct(";")?;
            return Ok(Stmt::Let(p, t, init, els, spos));
        }
        // (a statement starting with `if .. {}` / `match .. {}` / `{ .. }` etc. ends there: the next `*p = 1;` is not made a multiplication)
        if self.blocky_start() {
            let start = self.pos();
            let e = self.primary_expr(true)?;
            if !(self.is_punct(".") || self.is_punct("?")) {
                let semi = self.eat_punct(";");
                return Ok(Stmt::Expr(e, semi));
            }
            let e = self.postfix_rest(e, start)?;
            // (after the postfix part it is no longer block-like: `unsafe { a.get_unchecked(i) }.shr(n) & 1 != 0`
            //  goes on through casts and binary operators, as in rustc)
            let e = self.bin_continue(e, start)?;
            // (`{ x }.a = 1;` / `{ t }.0 += 1;`: an assignment to the place)
            let e = if self.tok(0).is_some_and(|t| t.kind == TokKind::Punct && t.text != ";") { self.place_assign(e, start)? } else { e };
            let semi = self.eat_punct(";");
            return Ok(Stmt::Expr(e, semi));
        }
        let e = self.expr(true)?;
        if self.eat_punct(";") {
            return Ok(Stmt::Expr(e, true));
        }
        let blocky = matches!(e, Expr::Block(..) | Expr::If(..) | Expr::Match(..) | Expr::Loop(..) | Expr::While(..) | Expr::For(..) | Expr::Const(..));
        let mac_brace = matches!(&e, Expr::Macro(_, g, _) if g.starts_with('{'));
        if blocky || mac_brace || self.is_close() {
            return Ok(Stmt::Expr(e, false));
        }
        // (where expect_punct puts a missing `;`: right after the expression when the next token is on a later line)
        self.expect_punct(";")?;
        Ok(Stmt::Expr(e, true))
    }

    /// An assignment to a place that starts with a block, `{ x }.a = 1` / `{ t }.0 += 1` (e when none follows)
    #[cold]
    #[inline(never)]
    fn place_assign(&mut self, e: Expr, start: Pos) -> R<Expr> {
        if self.is_punct("=") && !self.punct_at(0, "==") && !self.punct_at(0, "=>") {
            self.i += 1;
            return Ok(Expr::Assign(Box::new(e), Box::new(self.expr(true)?)));
        }
        if let Some(op) = ["<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|="].iter().find(|op| self.punct_at(0, op)) {
            let at = self.pos();
            self.i += op.len();
            return Ok(Expr::AssignOp(op[..op.len() - 1].to_string(), Box::new(e), Box::new(self.expr(true)?), Span { start, at }));
        }
        Ok(e)
    }

    // ------------------------------------------------ Items

    fn generics(&mut self) -> R<Generics> {
        let mut g = Generics { params: Vec::new(), preds: Vec::new() };
        self.gdefaults.clear();
        if !self.is_punct("<") {
            return Ok(g);
        }
        self.i += 1;
        let mut defaults = Vec::new();
        loop {
            if self.eat_close_angle() {
                break;
            }
            if let Some(l) = self.lifetime() {
                if self.eat_punct(":") {
                    // (`'a: 'b` as the where clause has it)
                    let bs = self.bounds()?;
                    g.preds.push((Ty::Path(None, Path { global: false, segs: vec![Seg { name: l.clone(), args: Vec::new(), fn_args: None, text: String::new() }], pos: Default::default() }), bs));
                }
                g.params.push(GParam::Lifetime(l));
            } else if self.eat_ident("const") {
                let (kw, nt) = (self.i - 1, self.i);
                let n = self.ident()?;
                self.expect_punct(":")?;
                let t = self.ty()?;
                if self.eat_punct("=") {
                    let _ = self.unary_expr(true)?;
                    self.gdefaults.push((g.params.len() as u32, kw as u32, nt as u32, true));
                }
                g.params.push(GParam::Const(n, t));
            } else {
                let nt = self.i;
                let n = self.ident()?;
                let bs = if self.eat_punct(":") { self.bounds()? } else { Vec::new() };
                let d = if self.eat_punct("=") {
                    let s = self.i;
                    let t = self.ty()?;
                    defaults.push((g.params.len(), s, self.i));
                    self.gdefaults.push((g.params.len() as u32, nt as u32, nt as u32, false));
                    Some(t)
                } else {
                    None
                };
                g.params.push(GParam::Ty(n, bs, d));
            }
            if !self.eat_punct(",") {
                if !self.eat_close_angle() {
                    return Err(self.err("expected `>`"));
                }
                break;
            }
        }
        if !defaults.is_empty() {
            self.forward_default(&g, &defaults)?;
        }
        Ok(g)
    }

    /// A generic parameter's default where rustc rejects it (after parsing, as rustc's checks of the items' generics):
    /// `defs` a type's or a trait's parameters, where one without a default may not follow one with ("generic parameters
    /// with a default must be trailing", at its name); else a fn's or an impl's, where a const parameter's default is
    /// "defaults for generic parameters are not allowed here", at the parameter (a type parameter's is the lint
    /// invalid_type_param_default, which a crate may allow)
    #[cold]
    fn misplaced_default(&mut self, g: &Generics, defs: bool) {
        let ds = std::mem::take(&mut self.gdefaults);
        if defs {
            let d = ds[0];
            let has = |j: usize| ds.iter().any(|e| e.0 as usize == j);
            if g.params.iter().enumerate().skip(d.0 as usize + 1).any(|(j, p)| !matches!(p, GParam::Lifetime(_)) && !has(j)) {
                self.defer(d.2 as usize, "generic parameters with a default must be trailing");
            }
        } else if let Some(d) = ds.iter().find(|d| d.3) {
            self.defer(d.1 as usize, "defaults for generic parameters are not allowed here");
        }
    }

    /// A raw pointer type without its keyword, at its `*` (out of the type parser's hot path)
    #[cold]
    #[inline(never)]
    fn bare_pointer_err(&self) -> String {
        let t = &self.toks[self.i - 1];
        format!("{}:{}: expected `mut` or `const` keyword in raw pointer type", t.line, t.col + 1)
    }

    /// "missing type for `const` item" (or `static`), right after the name or its colon (the token before the current
    /// one, or the current `:`); the parse goes on as if the type were `_`
    #[cold]
    fn missing_item_type(&mut self, kw: &str) {
        let (k, len) = if self.is_punct(":") { (self.i, 1) } else { (self.i - 1, self.toks[self.i - 1].text.chars().count()) };
        if self.deferred.is_none() {
            let t = &self.toks[k];
            self.deferred = Some(format!("{}:{}: missing type for `{kw}` item", t.line, t.col as usize + len + 1));
        }
        if self.is_punct(":") {
            self.i += 1;
        }
    }

    /// E0128: the default of a type parameter names that parameter or one declared after it (`<T = U, U>`). The
    /// defaults: (the parameter's index, the tokens of its default)
    #[cold]
    fn forward_default(&self, g: &Generics, defaults: &[(usize, usize, usize)]) -> R<()> {
        for &(pi, s, e) in defaults {
            for k in s..e {
                let t = &self.toks[k];
                if t.kind == TokKind::Ident
                    && !(k > 0 && self.toks[k - 1].text == ":")
                    && g.params[pi..].iter().any(|p| matches!(p, GParam::Ty(n, ..) | GParam::Const(n, _) if *n == t.text))
                {
                    return Err(format!("{}:{}: generic parameter defaults cannot reference parameters before they are declared (E0128)", t.line, t.col + 1));
                }
            }
        }
        Ok(())
    }

    fn where_clause(&mut self, g: &mut Generics) -> R<()> {
        if !self.eat_ident("where") {
            return Ok(());
        }
        loop {
            if self.is_open("{") || self.is_punct(";") || self.is_punct("=") || self.at_end() {
                break;
            }
            if let Some(l) = self.lifetime() {
                self.expect_punct(":")?;
                let bs = self.bounds()?;
                g.preds.push((Ty::Path(None, Path { global: false, segs: vec![Seg { name: l, args: Vec::new(), fn_args: None, text: String::new() }], pos: Default::default() }), bs));
            } else {
                let mut binder = vec![];
                if self.eat_ident("for") {
                    self.expect_punct("<")?;
                    binder = self.generic_args()?;
                }
                // (`dyn A + 'static: B`: a trait object type takes its bounds)
                let st = self.i;
                let t = if self.is_ident("dyn") { self.ty()? } else { self.ty_no_bounds()? };
                if let Ty::Path(None, p) = &t {
                    constraint_check(&self.toks, p, st)?;
                }
                self.expect_punct(":")?;
                let at = self.i;
                let mut bs = self.bounds()?;
                for b in &bs {
                    if let Bound::Trait(_, p) = b {
                        self.hr_output_check(&binder, p, at, Some(&t))?;
                    }
                }
                // (`for<'a> T: 'a`: the bound is of a lifetime the predicate binds, not one of the item: T outlives
                // every region, `T: 'static`)
                for b in bs.iter_mut() {
                    if matches!(b, Bound::Lifetime(l) if binder.iter().any(|a| matches!(a, GArg::Lifetime(x) if x == l))) {
                        *b = Bound::Lifetime("'static".to_string());
                    }
                }
                g.preds.push((t, bs));
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        Ok(())
    }

    fn fields(&mut self) -> R<Fields> {
        if self.is_open("(") {
            self.i += 1;
            let mut v = Vec::new();
            while !self.is_close() {
                self.attrs()?;
                self.vis()?;
                let ft = self.ty()?;
                if ty_has_infer(&ft) {
                    return Err(self.err_plain("the placeholder `_` is not allowed within types on item signatures for structs (E0121)"));
                }
                v.push(ft);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_close(")")?;
            return Ok(Fields::Tuple(v));
        }
        if self.is_open("{") {
            self.i += 1;
            let mut v = Vec::new();
            while !self.is_close() {
                self.attrs()?;
                self.vis()?;
                let n = field_name(self.decl_ident()?);
                if v.iter().any(|(m, _): &(String, Ty)| *m == n) {
                    return Err(self.err(&format!("field `{n}` is already declared (E0124)")));
                }
                self.expect_punct(":")?;
                let ft = self.ty()?;
                if ty_has_infer(&ft) {
                    return Err(self.err_plain("the placeholder `_` is not allowed within types on item signatures for structs (E0121)"));
                }
                v.push((n, ft));
                if !self.eat_punct(",") {
                    // (no `,` before another field: rustc's error is right after the field, where the comma goes)
                    if !self.is_close() && self.i > 0 {
                        if let Some(nt) = self.tok(0) {
                            let found = if nt.kind == TokKind::Ident && reserved_kw(&nt.text, true) { format!("keyword `{}`", nt.text) } else { format!("`{}`", nt.text) };
                            let pt = &self.toks[self.i - 1];
                            return Err(format!("{}:{}: expected `,`, or `}}`, found {found}", pt.line, pt.col as usize + pt.text.chars().count() + 1));
                        }
                    }
                    break;
                }
            }
            self.expect_close("}")?;
            return Ok(Fields::Named(v));
        }
        Ok(Fields::Unit)
    }

    fn vis(&mut self) -> R<bool> {
        if !self.eat_ident("pub") {
            return Ok(false);
        }
        // (`pub(crate)` / `pub(in m)`; in `struct S(pub (u8, u8))` the group is the field's type)
        if self.is_open("(") && ["crate", "self", "super", "in"].iter().any(|k| self.peek_ident(1, k)) {
            let _ = self.group_text()?;
        }
        Ok(true)
    }

    fn fn_sig(&mut self, quals: Vec<String>) -> R<FnSig> {
        let name = self.decl_ident()?;
        let mut g = self.generics()?;
        if !self.gdefaults.is_empty() {
            self.misplaced_default(&g, false);
        }
        self.expect_open("(")?;
        let mut recv = None;
        let mut recv_ty: Option<Ty> = None;
        let mut params = Vec::new();
        let mut first = true;
        while !self.is_close() {
            self.attrs()?;
            if first {
                // How self is taken
                let save = self.i;
                let mut r = String::new();
                let mut by_ref: Option<Option<String>> = None;
                if self.is_punct("&") {
                    self.i += 1;
                    r.push('&');
                    let l = self.lifetime();
                    if let Some(l) = &l {
                        r.push_str(l);
                        r.push(' ');
                    }
                    by_ref = Some(l);
                }
                let m = self.eat_ident("mut");
                if m {
                    r.push_str("mut ");
                }
                if self.eat_ident("self") {
                    r.push_str("self");
                    let self_path = Ty::Path(None, Path { global: false, segs: vec![Seg { name: "Self".to_string(), args: Vec::new(), fn_args: None, text: "Self".to_string() }], pos: Default::default() });
                    let mut rt = match by_ref {
                        Some(l) => Ty::Ref(l, m, Box::new(self_path)),
                        None => self_path,
                    };
                    if self.eat_punct(":") {
                        let t = self.ty()?;
                        r.push_str(&format!(": {}", ty_text(&t)));
                        rt = t;
                    }
                    recv = Some(r);
                    recv_ty = Some(rt);
                    first = false;
                    if !self.eat_punct(",") {
                        break;
                    }
                    continue;
                }
                self.i = save;
            }
            first = false;
            // (C variadics `extern "C" { fn printf(f: *const u8, ...); }`: the rest is not checked)
            if self.punct_at(0, "...") {
                self.i += 3;
                break;
            }
            let save = self.i;
            let nd = std::mem::replace(&mut self.no_dup_check, true);
            let p = self.pat();
            self.no_dup_check = nd;
            let p = match p {
                Ok(p) if self.is_punct(":") && !self.punct_at(0, "::") => {
                    self.no_double_binding(&p)?;
                    p
                }
                // (an anonymous parameter of a 2015 trait method, `fn f(u8, u8);`: read as a pattern first, so that
                // `fn f(&'a str)` is still an error)
                Err(e) => return Err(e),
                r => {
                    let at = self.i;
                    self.i = save;
                    match self.ty() {
                        Ok(t) if self.is_punct(",") || self.is_close() => {
                            params.push((Pat::Wild, t));
                            if !self.eat_punct(",") {
                                break;
                            }
                            continue;
                        }
                        _ => {
                            self.i = at;
                            r?;
                            return Err(self.err("expected `:`"));
                        }
                    }
                }
            };
            // (E0130: a foreign fn's parameters are names, `_` or plain)
            if self.in_extern && !matches!(&p, Pat::Wild | Pat::Ident(false, false, _, None)) && !matches!(&p, Pat::Path(q) if q.segs.len() == 1) {
                let t = &self.toks[save];
                return Err(format!("{}:{}: patterns aren't allowed in foreign function declarations (E0130)", t.line, t.col + 1));
            }
            self.expect_punct(":")?;
            let t = if self.punct_at(0, "...") {
                self.i += 3;
                Ty::Infer
            } else {
                let t = self.ty()?;
                if ty_has_infer(&t) {
                    return Err(self.err_plain(PLACEHOLDER_FN));
                }
                t
            };
            params.push((p, t));
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_close(")")?;
        let ret = if self.eat_punct("->") { Some(self.ty()?) } else { None };
        if ret.as_ref().is_some_and(ty_has_infer) || recv_ty.as_ref().is_some_and(ty_has_infer) {
            return Err(self.err_plain(PLACEHOLDER_FN));
        }
        self.where_clause(&mut g)?;
        Ok(FnSig { name: name, generics: g, recv: recv, recv_ty: recv_ty, params: params, ret: ret, quals: quals })
    }

    fn items_in_braces(&mut self) -> R<Vec<Item>> {
        self.expect_open("{")?;
        let mut v = Vec::new();
        while !self.is_close() {
            if self.at_end() {
                return Err(self.err("unclosed `{`"));
            }
            if self.is_punct("#") && self.peek_is(1, "!") {
                let _ = self.attr()?;
                continue;
            }
            v.push(self.item()?);
        }
        self.expect_close("}")?;
        Ok(v)
    }

    pub fn item(&mut self) -> R<Item> {
        let attrs = self.attrs()?;
        let line = self.tok(0).map(|t| t.line).unwrap_or(0);
        let vis = self.vis()?;
        self.init_text = String::new();
        let kind = self.item_kind()?;
        let init_text = std::mem::take(&mut self.init_text);
        Ok(Item { attrs: attrs, vis: vis, kind: kind, line: line, init_text: init_text })
    }

    fn item_kind(&mut self) -> R<ItemKind> {
        let mut quals = Vec::new();
        loop {
            if self.is_ident("const") && (self.peek_ident(1, "fn") || self.peek_ident(1, "unsafe") || self.peek_ident(1, "extern")) {
                quals.push("const".to_string());
                self.i += 1;
            } else if self.is_ident("safe") && (self.peek_ident(1, "fn") || self.peek_ident(1, "static")) {
                // (`unsafe extern { safe fn f(); }`)
                self.i += 1;
            } else if self.is_ident("async") && !self.peek_is(1, "{") {
                quals.push("async".to_string());
                self.i += 1;
            } else if self.is_ident("unsafe") && !self.peek_is(1, "{") {
                quals.push("unsafe".to_string());
                self.i += 1;
            } else if self.is_ident("extern") && !self.peek_ident(1, "crate") && !(self.tok(1).is_some_and(|t| t.kind == TokKind::Lit) && self.tok(2).is_some_and(|t| t.kind == TokKind::Open)) && !self.peek_is(1, "{") {
                self.i += 1;
                let abi = if self.tok(0).is_some_and(|t| t.kind == TokKind::Lit) {
                    let s = self.toks[self.i].text.clone();
                    self.i += 1;
                    s
                } else {
                    String::new()
                };
                quals.push(format!("extern {abi}"));
            } else {
                break;
            }
        }
        let Some(t) = self.tok(0) else { return Err(self.err("expected an item")) };
        let kw = t.text.clone();
        if t.kind != TokKind::Ident {
            return Err(self.err("expected an item"));
        }
        match kw.as_str() {
            "fn" => {
                self.i += 1;
                let sig = self.fn_sig(quals)?;
                if self.eat_punct(";") {
                    return Ok(ItemKind::Fn(sig, None));
                }
                // (E0415: a name bound by two parameters of a fn with a body; a declaration (a trait's, an extern
                // one) may repeat one)
                if sig.params.len() > 1 && !sig.params.iter().all(|(p, _)| matches!(p, Pat::Wild)) {
                    let mut seen = crate::fxhash::FxSet::default();
                    if let Some(n) = sig.params.iter().find_map(|(p, _)| double_binding(p, &mut seen)) {
                        return Err(self.err(&format!("identifier `{n}` is bound more than once in this parameter list (E0415)")));
                    }
                }
                let st = self.i;
                let saved = std::mem::replace(&mut self.in_async, sig.quals.iter().any(|q| q == "async"));
                let saved_c = std::mem::replace(&mut self.in_const, false);
                let b = self.block();
                self.in_async = saved;
                self.in_const = saved_c;
                let b = b?;
                // (a `const fn`'s body text: constants may call it)
                if sig.quals.iter().any(|q| q == "const") {
                    self.init_text = self.tokens_text(st, self.i);
                }
                Ok(ItemKind::Fn(sig, Some(b)))
            }
            "struct" | "union" => {
                self.i += 1;
                let n = self.decl_ident()?;
                let mut g = self.generics()?;
                if !self.gdefaults.is_empty() {
                    self.misplaced_default(&g, true);
                }
                self.where_clause(&mut g)?;
                let f = self.fields()?;
                self.where_clause(&mut g)?;
                self.eat_punct(";");
                if kw == "union" {
                    return Ok(ItemKind::Union(n, g, f));
                }
                Ok(ItemKind::Struct(n, g, f))
            }
            "enum" => {
                self.i += 1;
                let n = self.decl_ident()?;
                let mut g = self.generics()?;
                if !self.gdefaults.is_empty() {
                    self.misplaced_default(&g, true);
                }
                self.where_clause(&mut g)?;
                self.expect_open("{")?;
                let mut vs = Vec::new();
                while !self.is_close() {
                    self.attrs()?;
                    let vat = self.i;
                    let vn = self.decl_ident()?;
                    // (E0428: two variants of one name)
                    if vs.iter().any(|v: &(String, Fields, Option<Expr>)| v.0 == vn) {
                        let t = &self.toks[vat];
                        return Err(format!("{}:{}: the name `{vn}` is defined multiple times (E0428)", t.line, t.col + 1));
                    }
                    let f = self.fields()?;
                    let d = if self.eat_punct("=") { Some(self.const_expr()?) } else { None };
                    vs.push((vn, f, d));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_close("}")?;
                Ok(ItemKind::Enum(n, g, vs))
            }
            "impl" => {
                self.i += 1;
                let mut g = if self.is_punct("<") && !self.punct_at(1, ">") { self.generics()? } else { Generics { params: Vec::new(), preds: Vec::new() } };
                if !self.gdefaults.is_empty() {
                    self.misplaced_default(&g, false);
                }
                // (`impl<> T<> for S<>`: empty generics; also `impl< >`, as a macro's `impl< $($t,)* >` gives them for no `$t`)
                if self.is_punct("<") && self.punct_at(1, ">") {
                    self.i += 2;
                }
                let neg = self.eat_punct("!");
                // (`impl dyn Tr + 'a { .. }` / `impl X for dyn Tr + Send`: the self type takes bounds)
                let first = if self.is_ident("dyn") { self.ty()? } else { self.ty_no_bounds()? };
                let (tr, self_ty) = if self.eat_ident("for") {
                    let p = match first {
                        Ty::Path(None, p) => p,
                        _ => return Err(self.err("expected a trait path")),
                    };
                    (Some((neg, p)), if self.is_ident("dyn") { self.ty()? } else { self.ty_no_bounds()? })
                } else {
                    (None, first)
                };
                self.where_clause(&mut g)?;
                let items = self.items_in_braces()?;
                Ok(ItemKind::Impl(g, tr, self_ty, items))
            }
            "trait" => {
                self.i += 1;
                let n = self.decl_ident()?;
                let mut g = self.generics()?;
                if !self.gdefaults.is_empty() {
                    self.misplaced_default(&g, true);
                }
                let sup = if self.eat_punct(":") { self.bounds()? } else { Vec::new() };
                self.where_clause(&mut g)?;
                let items = self.items_in_braces()?;
                Ok(ItemKind::Trait(n, g, sup, items))
            }
            "use" => {
                self.i += 1;
                let mut s = String::new();
                let mut prev_word = false;
                // (the path before each open `{`, and since it: an import of `self` of the crate root, `self` or `super`
                // has no name (E0431, rustc 1.100's message, at the `self`))
                let (mut prefixes, mut path): (Vec<String>, String) = (vec![], String::new());
                while !self.is_punct(";") {
                    let t = self.tok(0).ok_or_else(|| self.err("unterminated use"))?;
                    let leaf = t.kind == TokKind::Ident && t.text == "self" && path.is_empty() && (self.peek_is(1, "}") || self.peek_is(1, ",") || self.peek_is(1, ";"));
                    if leaf {
                        // (`use self;`, `use {self};`, `use crate::{self};`, `use super::{self};`; not `use ::{self};`, which is
                        // another error, nor a module's `self`)
                        let outer = prefixes.last().map_or("", String::as_str);
                        if !outer.starts_with("::") && outer.trim_end_matches("::").split("::").all(|w| matches!(w, "" | "crate" | "self" | "super")) {
                            return Err(self.err_plain("imports need to be explicitly named"));
                        }
                    }
                    match t.text.as_str() {
                        "{" => {
                            prefixes.push(format!("{}{}", prefixes.last().map_or("", String::as_str), path));
                            path.clear();
                        }
                        "}" => {
                            prefixes.pop();
                            path.clear();
                        }
                        "," => path.clear(),
                        _ => path.push_str(&t.text),
                    }
                    let t = self.tok(0).ok_or_else(|| self.err("unterminated use"))?;
                    // (a name does not come right after a name / `}`: `use a::{b} struct X` is missing a `;`)
                    let word = t.kind == TokKind::Ident || (t.kind == TokKind::Close);
                    // (`use foo::{bar} as baz;` / `use foo::* as baz;`: a group or a glob has no name to rename)
                    if t.text == "as" && prefixes.is_empty() && (s.ends_with('}') || s.ends_with('*')) {
                        return Err(self.err_plain("expected `;`, found keyword `as`"));
                    }
                    if prev_word && t.kind == TokKind::Ident && t.text != "as" && !s.ends_with("as") {
                        return Err(self.err("expected `;`"));
                    }
                    // (a lone `:` for `::`, `use std:fs::File;`; and in a braced list a token that cannot follow a
                    // name, `use x::{A. B};`: rustc's messages, at the token)
                    if t.kind == TokKind::Punct && prev_word {
                        let pair = t.text == ":" && t.joint && self.punct_at(1, ":");
                        if t.text == ":" && !pair {
                            return Err(self.err_plain("expected `::`, found `:`"));
                        }
                        if !prefixes.is_empty() && !pair && !matches!(t.text.as_str(), "," | "}" | "{" | "*" | ":") {
                            return Err(self.err_plain(&format!("expected one of `,`, `::`, `as`, or `}}`, found `{}`", t.text)));
                        }
                    }
                    prev_word = word;
                    s.push_str(&t.text);
                    self.i += 1;
                }
                self.i += 1;
                Ok(ItemKind::Use(s))
            }
            "const" => {
                self.i += 1;
                let n = if self.eat_ident("_") { "_".to_string() } else { self.ident()? };
                // (no type before the value, `const FOO = 1;` / `const FOO: = 1;`: rustc's error after the name or the
                // colon, reported after the parse as its type check reports it)
                if self.is_punct("=") || (self.is_punct(":") && self.punct_at(1, "=")) {
                    self.missing_item_type("const");
                }
                let t = if self.eat_punct(":") { Some(self.ty()?) } else { None };
                if t.as_ref().is_some_and(ty_has_infer) {
                    return Err(self.err_plain("the placeholder `_` is not allowed within types on item signatures for constants (E0121)"));
                }
                let e = if self.eat_punct("=") {
                    let st = self.i;
                    let e = self.const_expr()?;
                    self.init_text = self.tokens_text(st, self.i);
                    Some(e)
                } else {
                    None
                };
                self.expect_punct(";")?;
                Ok(ItemKind::Const(n, t, e))
            }
            "static" => {
                self.i += 1;
                let m = self.eat_ident("mut");
                let n = self.decl_ident()?;
                let missing = self.is_punct("=") || (self.is_punct(":") && self.punct_at(1, "="));
                let t = if missing {
                    self.missing_item_type("static");
                    Ty::Infer
                } else {
                    self.expect_punct(":")?;
                    self.ty()?
                };
                if !missing && ty_has_infer(&t) {
                    return Err(self.err_plain("the placeholder `_` is not allowed within types on item signatures for statics (E0121)"));
                }
                // (`static X: T;` in an extern block: a symbol of the program it links with, no initializer)
                if self.is_punct(";") && !self.in_extern {
                    return Err(self.err("free static item without body"));
                }
                if self.eat_punct(";") {
                    self.init_text = EXTERN_STATIC.to_string();
                    let seg = Seg { name: EXTERN_STATIC.to_string(), args: vec![], fn_args: None, text: EXTERN_STATIC.to_string() };
                    return Ok(ItemKind::Static(m, n, t, Expr::Path(None, Path { global: false, segs: vec![seg], pos: Default::default() })));
                }
                self.expect_punct("=")?;
                let st = self.i;
                let e = self.const_expr()?;
                self.init_text = self.tokens_text(st, self.i);
                self.expect_punct(";")?;
                Ok(ItemKind::Static(m, n, t, e))
            }
            "type" => {
                self.i += 1;
                let n = self.decl_ident()?;
                let mut g = self.generics()?;
                let bs = if self.eat_punct(":") { self.bounds()? } else { Vec::new() };
                self.where_clause(&mut g)?;
                let t = if self.eat_punct("=") { Some(self.ty()?) } else { None };
                // (not the parser's own `type __B = dyn ..;`, which reads a bound's text)
                if n != "__B" && t.as_ref().is_some_and(ty_has_infer) {
                    return Err(self.err_plain("the placeholder `_` is not allowed within types on item signatures for type aliases (E0121)"));
                }
                // (a generic associated type's where clause after the value: `type Item<'a> = &'a T where T: 'a;`)
                self.where_clause(&mut g)?;
                self.expect_punct(";")?;
                Ok(ItemKind::Type(n, g, bs, t))
            }
            "mod" => {
                self.i += 1;
                let n = self.decl_ident()?;
                if self.eat_punct(";") {
                    return Ok(ItemKind::Mod(n, None));
                }
                Ok(ItemKind::Mod(n, Some(self.items_in_braces()?)))
            }
            "extern" => {
                self.i += 1;
                if self.eat_ident("crate") {
                    let n = self.ident()?;
                    if self.eat_ident("as") {
                        let _ = self.ident()?;
                    }
                    self.expect_punct(";")?;
                    return Ok(ItemKind::ExternCrate(n));
                }
                if self.tok(0).is_some_and(|t| t.kind == TokKind::Lit) {
                    self.i += 1;
                }
                let outer = std::mem::replace(&mut self.in_extern, true);
                let items = self.items_in_braces();
                self.in_extern = outer;
                Ok(ItemKind::Extern(items?))
            }
            "macro_rules" => {
                self.i += 1;
                self.expect_punct("!")?;
                let n = self.ident()?;
                let g = self.group_text()?;
                self.eat_punct(";");
                Ok(ItemKind::MacroRules(n, g))
            }
            _ => {
                if quals.iter().any(|q| q == "unsafe") && self.is_open("{") {
                    return Err(self.err("expected an item"));
                }
                // `name!(..);` (a macro in item position)
                let p = self.path(false)?;
                if !self.eat_punct("!") {
                    return Err(self.err("expected an item"));
                }
                let g = self.group_text()?;
                self.eat_punct(";");
                Ok(ItemKind::Macro(p, g))
            }
        }
    }
}

// ---------------------------------------------------------------- Display (for checking)

/// E0586: `1..=` / `0..= =` / `X...`, an inclusive range with no end. at: the `..=`'s first token
#[cold]
fn no_end_err(toks: &[Tok], at: usize) -> String {
    match toks.get(at) {
        Some(t) => format!("{}:{}: inclusive range with no end (E0586)", t.line, t.col + 1),
        None => "inclusive range with no end (E0586)".to_string(),
    }
}

/// E0229: `<I as Foo<A = Bar>>::A`, `where T<Bogus = Foo>: A`, an associated item's constraint where a path names a
/// type, or a trait it is not a bound of. st: the path's first token
fn constraint_check(toks: &[Tok], p: &Path, st: usize) -> R<()> {
    if p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Bind(..)))) {
        return Err(constraint_err(toks, st));
    }
    Ok(())
}

#[cold]
fn constraint_err(toks: &[Tok], st: usize) -> String {
    match toks.get(st) {
        Some(t) => format!("{}:{}: associated item constraints are not allowed here (E0229)", t.line, t.col + 1),
        None => "associated item constraints are not allowed here (E0229)".to_string(),
    }
}

pub fn path_text(p: &Path) -> String {
    let mut s = String::new();
    if p.global {
        s.push_str("::");
    }
    for (k, seg) in p.segs.iter().enumerate() {
        if k > 0 {
            s.push_str("::");
        }
        s.push_str(&seg.name);
        if !seg.args.is_empty() {
            s.push('<');
            for (j, a) in seg.args.iter().enumerate() {
                if j > 0 {
                    s.push_str(", ");
                }
                match a {
                    GArg::Ty(t) => s.push_str(&ty_text(t)),
                    GArg::Lifetime(l) => s.push_str(l),
                    GArg::Bind(n, t) => s.push_str(&format!("{n} = {}", ty_text(t))),
                    GArg::Const(_) => s.push_str("{const}"),
                }
            }
            s.push('>');
        }
        if let Some((ps, r)) = &seg.fn_args {
            let v: Vec<String> = ps.iter().map(ty_text).collect();
            s.push_str(&format!("({})", v.join(", ")));
            if let Some(r) = r {
                s.push_str(&format!(" -> {}", ty_text(r)));
            }
        }
    }
    s
}

fn bounds_text(bs: &[Bound]) -> String {
    let v: Vec<String> = bs
        .iter()
        .map(|b| match b {
            Bound::Trait(q, p) => format!("{}{}", if *q { "?" } else { "" }, path_text(p)),
            Bound::Lifetime(l) => l.clone(),
        })
        .collect();
    v.join(" + ")
}

pub fn ty_text(t: &Ty) -> String {
    match t {
        Ty::Path(None, p) => path_text(p),
        Ty::Path(Some(q), p) => format!("<{} as {}>", ty_text(q), path_text(p)),
        Ty::Ref(l, m, i) => format!("&{}{}{}", l.as_ref().map(|l| format!("{l} ")).unwrap_or_default(), if *m { "mut " } else { "" }, ty_text(i)),
        Ty::Ptr(m, i) => format!("*{} {}", if *m { "mut" } else { "const" }, ty_text(i)),
        Ty::Slice(i) => format!("[{}]", ty_text(i)),
        Ty::Array(i, _) => format!("[{}; N]", ty_text(i)),
        Ty::Tuple(v) => {
            let v: Vec<String> = v.iter().map(ty_text).collect();
            format!("({})", v.join(", "))
        }
        Ty::Fn(ps, r) => {
            let v: Vec<String> = ps.iter().map(ty_text).collect();
            format!("fn({}) -> {}", v.join(", "), ty_text(r))
        }
        Ty::Impl(bs) => format!("impl {}", bounds_text(bs)),
        Ty::Dyn(bs) => format!("dyn {}", bounds_text(bs)),
        Ty::Never => "!".to_string(),
        Ty::Infer => "_".to_string(),
        Ty::Macro(g) => format!("macro!{g}"),
    }
}

/// The number of syntax tree nodes (per kind). Checked against the counts made with syn
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Counts {
    pub fns: u32,
    pub structs: u32,
    pub enums: u32,
    pub impls: u32,
    pub traits: u32,
    pub calls: u32,
    pub method_calls: u32,
    pub closures: u32,
    pub matches: u32,
    pub ifs: u32,
    pub loops: u32,
    pub lets: u32,
    pub macros: u32,
    pub struct_lits: u32,
    pub binaries: u32,
    pub paths: u32,
    pub lits: u32,
    pub fields: u32,
    pub indexes: u32,
    pub refs: u32,
    pub casts: u32,
    pub returns: u32,
}

impl Counts {
    pub fn file(&mut self, f: &File) {
        for it in &f.items {
            self.item(it);
        }
    }
    fn item(&mut self, it: &Item) {
        match &it.kind {
            ItemKind::Fn(_, b) => {
                self.fns += 1;
                if let Some(b) = b {
                    self.block(b);
                }
            }
            ItemKind::Struct(..) => self.structs += 1,
            ItemKind::Enum(_, _, vs) => {
                self.enums += 1;
                for v in vs {
                    if let Some(e) = &v.2 {
                        self.expr(e);
                    }
                }
            }
            ItemKind::Impl(_, _, _, items) => {
                self.impls += 1;
                for i in items {
                    self.item(i);
                }
            }
            ItemKind::Trait(_, _, _, items) => {
                self.traits += 1;
                for i in items {
                    self.item(i);
                }
            }
            ItemKind::Const(_, _, Some(e)) | ItemKind::Static(_, _, _, e) => self.expr(e),
            ItemKind::Mod(_, Some(items)) | ItemKind::Extern(items) => {
                for i in items {
                    self.item(i);
                }
            }
            ItemKind::Macro(..) | ItemKind::MacroRules(..) => self.macros += 1,
            _ => {}
        }
    }
    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            match s {
                Stmt::Let(_, _, init, els, ..) => {
                    self.lets += 1;
                    if let Some(e) = init {
                        self.expr(e);
                    }
                    if let Some(b) = els {
                        self.block(b);
                    }
                }
                Stmt::Expr(e, _) => self.expr(e),
                Stmt::Item(it) => self.item(it),
            }
        }
    }
    fn exprs(&mut self, v: &[Expr]) {
        for e in v {
            self.expr(e);
        }
    }
    pub fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Lit(_, _) => self.lits += 1,
            Expr::Path(..) => self.paths += 1,
            Expr::Unary(_, x, _) | Expr::Try(x) | Expr::Paren(x) => self.expr(x),
            Expr::Ref(_, x) | Expr::RawRef(_, x) => {
                self.refs += 1;
                self.expr(x);
            }
            Expr::Binary(_, l, r, _) => {
                self.binaries += 1;
                self.expr(l);
                self.expr(r);
            }
            Expr::Assign(l, r) => {
                self.expr(l);
                self.expr(r);
            }
            // (syn 2 makes `a += b` a Binary too)
            Expr::AssignOp(_, l, r, _) => {
                self.binaries += 1;
                self.expr(l);
                self.expr(r);
            }
            Expr::Call(f, a) => {
                self.calls += 1;
                self.expr(f);
                self.exprs(a);
            }
            Expr::MethodCall(r, _, _, a, _) => {
                self.method_calls += 1;
                self.expr(r);
                self.exprs(a);
            }
            Expr::Field(x, _, _) => {
                self.fields += 1;
                self.expr(x);
            }
            Expr::Index(x, i, _) => {
                self.indexes += 1;
                self.expr(x);
                self.expr(i);
            }
            Expr::Range(a, b, _) => {
                if let Some(a) = a {
                    self.expr(a);
                }
                if let Some(b) = b {
                    self.expr(b);
                }
            }
            Expr::Cast(x, _) => {
                self.casts += 1;
                self.expr(x);
            }
            Expr::Block(_, _, b) | Expr::Const(b) => self.block(b),
            Expr::If(c, t, e) => {
                self.ifs += 1;
                self.expr(c);
                self.block(t);
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            Expr::Let(_, x) => self.expr(x),
            Expr::Match(x, arms) => {
                self.matches += 1;
                self.expr(x);
                for a in arms {
                    if let Some(g) = &a.guard {
                        self.expr(g);
                    }
                    self.expr(&a.body);
                }
            }
            Expr::While(_, c, b) => {
                self.loops += 1;
                self.expr(c);
                self.block(b);
            }
            Expr::Loop(_, b) => {
                self.loops += 1;
                self.block(b);
            }
            Expr::For(_, _, x, b) => {
                self.loops += 1;
                self.expr(x);
                self.block(b);
            }
            Expr::Break(_, v, _) | Expr::Return(v, _) => {
                if matches!(e, Expr::Return(..)) {
                    self.returns += 1;
                }
                if let Some(v) = v {
                    self.expr(v);
                }
            }
            Expr::Continue(..) => {}
            Expr::Closure(_, _, _, b, _) => {
                self.closures += 1;
                self.expr(b);
            }
            Expr::Tuple(v) | Expr::Array(v) => self.exprs(v),
            Expr::Repeat(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            Expr::Struct(_, fs, base) => {
                self.struct_lits += 1;
                for f in fs {
                    self.expr(&f.1);
                }
                if let Some(b) = base {
                    self.expr(b);
                }
            }
            Expr::Macro(..) => self.macros += 1,
        }
    }
}

/// A reserved keyword (not the 2018 ones, which an edition 2015 file may use as names). `with_paths` also counts
/// those a path may start with (`self` / `Self` / `crate` / `super`) and `true` / `false` / `mut` / `ref`. By length
/// first: a few comparisons per name, as the parser asks for every declared name
pub fn reserved_kw(s: &str, with_paths: bool) -> bool {
    let b = s.as_bytes();
    if b.len() < 2 || b.len() > 8 || !(b[0].is_ascii_lowercase() || b[0] == b'S') {
        return false;
    }
    let core = match b.len() {
        2 => matches!(s, "as" | "fn" | "if" | "in"),
        3 => matches!(s, "for" | "let" | "mod" | "pub" | "use"),
        4 => matches!(s, "else" | "enum" | "impl" | "loop" | "move" | "type"),
        5 => matches!(s, "break" | "const" | "match" | "trait" | "where" | "while"),
        6 => matches!(s, "extern" | "return" | "static" | "struct" | "unsafe"),
        8 => s == "continue",
        _ => false,
    };
    core || (with_paths && matches!(s, "self" | "Self" | "crate" | "super" | "true" | "false" | "mut" | "ref"))
}

/// A keyword reserved for the future in every edition (`let final = ();` is an error; `r#final` is a name)
pub fn unused_kw(s: &str) -> bool {
    match s.len() {
        2 => s == "do",
        3 => s == "box",
        4 => s == "priv",
        5 => matches!(s, "final" | "macro" | "yield"),
        6 => matches!(s, "become" | "typeof"),
        7 => matches!(s, "unsized" | "virtual"),
        8 => matches!(s, "abstract" | "override"),
        _ => false,
    }
}

/// The first name a pattern binds twice. A name binds when it starts in lowercase (an uppercase one may be a unit
/// variant or a constant); of an or-pattern, the first alternative's names
pub(crate) fn double_binding<'a>(p: &'a Pat, seen: &mut crate::fxhash::FxSet<&'a str>) -> Option<&'a str> {
    match p {
        Pat::Ident(_, _, n, sub) => {
            // (a name the module renaming made (`serde_core__de__ignored_any__IgnoredAny`): its last part says what it is)
            let base = n.rsplit("__").next().filter(|b| !b.is_empty()).unwrap_or(n);
            if base.starts_with(|c: char| c.is_lowercase() || c == '_') && n != "_" && !seen.insert(n.as_str()) {
                return Some(n);
            }
            sub.as_deref().and_then(|s| double_binding(s, seen))
        }
        Pat::Tuple(ps) | Pat::Slice(ps) | Pat::TupleStruct(_, ps) => ps.iter().find_map(|q| double_binding(q, seen)),
        Pat::Struct(_, fs, _) => fs.iter().find_map(|(_, q)| double_binding(q, seen)),
        Pat::Ref(_, q) => double_binding(q, seen),
        // (each alternative on its own: `(x, y) | (y, x)` is fine, `(x, y) | (y, y)` is not; the first one's names
        // stay bound for the rest of the pattern)
        Pat::Or(ps, _) => {
            let before = seen.clone();
            for q in ps.iter().skip(1) {
                let mut s2 = before.clone();
                if let Some(n) = double_binding(q, &mut s2) {
                    return Some(n);
                }
            }
            ps.first().and_then(|q| double_binding(q, seen))
        }
        _ => None,
    }
}

/// Whether a type written in an item's signature has a `_` placeholder (E0121)
fn ty_has_infer(t: &Ty) -> bool {
    let arg = |a: &GArg| match a {
        GArg::Ty(t) | GArg::Bind(_, t) => ty_has_infer(t),
        _ => false,
    };
    match t {
        Ty::Infer => true,
        // (`Fn(_)` as a path in 2015: its parenthesized arguments too)
        Ty::Path(q, p) => {
            q.as_deref().is_some_and(ty_has_infer)
                || p.segs.iter().any(|s| s.args.iter().any(arg) || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(ty_has_infer) || r.as_deref().is_some_and(ty_has_infer)))
        }
        Ty::Ref(_, _, i) | Ty::Ptr(_, i) | Ty::Slice(i) | Ty::Array(i, _) => ty_has_infer(i),
        Ty::Tuple(ts) => ts.iter().any(ty_has_infer),
        Ty::Fn(ps, r) => ps.iter().any(ty_has_infer) || ty_has_infer(r),
        // (`&dyn Fn(_)`: the arguments of a bound's trait)
        Ty::Dyn(bs) | Ty::Impl(bs) => bs.iter().any(|b| match b {
            Bound::Trait(_, p) => p.segs.iter().any(|s| {
                s.args.iter().any(arg) || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(ty_has_infer) || r.as_deref().is_some_and(ty_has_infer))
            }),
            _ => false,
        }),
        _ => false,
    }
}

const PLACEHOLDER_FN: &str = "the placeholder `_` is not allowed within types on item signatures for functions (E0121)";

/// A field's name: a macro body's name renamed for hygiene (`core__mh`, the body has a `let core`) names the field
/// `core`, as rustc's hygiene does not apply to fields (chacha20 0.10: `Outer { core }` in `impl_chacha_rng!`)
fn field_name(n: String) -> String {
    let mut s = n.as_str();
    while let Some(t) = s.strip_suffix("__mh") {
        s = t;
    }
    if s.len() == n.len() { n } else { s.to_string() }
}
