//! `macro_rules!` の展開 (ソースのテキストの段階で行う)。
//!
//! 1 ファイルの中の `macro_rules! name { (pattern) => { body }; .. }` を集め、`name!(..)` の呼び出しを
//! 展開したテキストで置き換えてから字句解析に渡す。定義は空白で消す。
//! - 展開は 1 行に収める (後ろの行番号を変えないため。エラーの列は展開の中ではずれる)
//! - 式の位置では `({ .. })` で包み、トップレベル (アイテムの位置) ではそのまま
//! - `$x:expr` などの断片は `( .. )` で包んで埋め込む (Rust の断片は不透明なので `$x * $x` の優先順位が変わらない)
//! - 断片の種類: expr / ident / ty / pat / path / literal / tt / block / stmt / lifetime / item。
//!   expr などはパターンの次の区切り (`,` / `;` / `=>` など) が出るまでのトークン列を取る
//! - 繰り返し `$( .. ) sep op` (op は `*` / `+` / `?`)。展開の中の呼び出しも (深さ 64 まで) 展開する

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
type HashMap<K, V> = crate::fxhash::FxMap<K, V>;
use std::str::FromStr;

/// パターン / 本体のトークン (区切りの付いた繰り返しは Rep)
#[derive(Clone, Debug)]
enum Pt {
    Tok(TokenTree),
    Group(Delimiter, Vec<Pt>),
    Var(String, String),
    Rep(Vec<Pt>, Option<TokenTree>, char),
}

#[derive(Clone, Debug)]
enum Bind {
    One(Vec<TokenTree>, bool),
    Many(Vec<Bind>),
}

struct Rule {
    pat: Vec<Pt>,
    body: Vec<Pt>,
}

/// rustc's fragment specifiers
const FRAGS: &[&str] = &["block", "expr", "expr_2021", "ident", "item", "lifetime", "literal", "meta", "pat", "pat_param", "path", "stmt", "tt", "ty", "vis"];

thread_local! {
    /// The first fragment specifier of a `macro_rules!` matcher that is none of rustc's (`$x:id`), met while the
    /// definitions are read: the driver reports it, "invalid fragment specifier"
    pub static BAD_FRAGMENT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// A matcher's fragment that a token rustc does not allow after it follows (`$e:expr +`): the fragment as
    /// written, `$e:expr`, and rustc's message; the driver finds its place in the source
    pub static BAD_FOLLOW: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
    /// Whether a `pat` fragment matches `a | b` (edition 2021 on): `|` cannot follow one then
    pub static PAT_WITH_OR: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

thread_local! {
    /// Whether a `macro_rules!` definition was read that rustc rejects (no rule, a matcher outside delimiters, a rule
    /// cut short): macro_def_err finds it in the source then
    pub static BAD_DEF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// rustc's error for the first `macro_rules!` definition of src it rejects when it reads the definitions, used or not:
/// one delimited by parentheses or brackets with no `;` after it, one with no rule, a matcher that is no delimited
/// group, a rule without its `=>` or its body
#[cold]
pub fn macro_def_err(src: &str) -> Option<String> {
    let b = src.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let (close, _) = brackets(b, &toks);
    let sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && b[toks[k].1] == c;
    let open = |k: usize| (sym(k, b'(') || sym(k, b'[') || sym(k, b'{')) && close[k] != usize::MAX;
    let err = |k: usize, m: &str| {
        let off = toks[k].1;
        let line = 1 + b[..off].iter().filter(|&&c| c == b'\n').count();
        let col = src[..off].chars().rev().take_while(|&c| c != '\n').count() + 1;
        Some(format!("{line}:{col}: {m}"))
    };
    let mut i = 0;
    while i + 3 < toks.len() {
        if !(toks[i].0 == 0 && &b[toks[i].1..toks[i].2] == b"macro_rules" && sym(i + 1, b'!') && toks[i + 2].0 == 0 && open(i + 3)) {
            i += 1;
            continue;
        }
        let (o, c) = (i + 3, close[i + 3]);
        if !sym(o, b'{') && !sym(c + 1, b';') {
            return err(o, "macros that expand to items must be delimited with braces or followed by a semicolon");
        }
        if o + 1 == c {
            return err(i, "macros must contain at least one rule");
        }
        let mut j = o + 1;
        while j < c {
            // (a rule's attribute, `#[doc = ".."]`)
            if sym(j, b'#') && sym(j + 1, b'[') && open(j + 1) {
                j = close[j + 1] + 1;
                continue;
            }
            // (an attribute's or a derive's rule, `attr(args) (input) => ..`: E0658 without its feature, at the
            // definition; with it, its matcher follows the arguments)
            let w = &b[toks[j].1..toks[j].2];
            if toks[j].0 == 0 && (w == b"attr" || w == b"derive") && open(j + 1) {
                let (feat, what) = if w == b"attr" { ("macro_attr", "attributes") } else { ("macro_derive", "derives") };
                if !src.contains(feat) {
                    return err(i, &format!("`macro_rules!` {what} are unstable (E0658)"));
                }
                j = close[j + 1] + 1;
            }
            if !open(j) {
                return err(j, "invalid macro matcher; matchers must be contained in balanced delimiters");
            }
            let m = close[j];
            // (the matcher's `$name:kind` / `$( .. ) sep op`, as bad_matcher reads them)
            let kleene = |k: usize| sym(k, b'*') || sym(k, b'+') || sym(k, b'?');
            let tx = |k: usize| if k < toks.len() { &b[toks[k].1..toks[k].2] } else { &b[0..0] };
            let kind_at = |k: usize| k + 1 < toks.len() && sym(k, b':') && toks[k + 1].0 == 0;
            // (the operator of the repetition whose group closes at e: right after it, after a one-token separator, or
            // after `::`. A delimiter is no separator: in `($($b:tt))*` the `)` closes the outer group, and the `*` is
            // not the repetition's)
            let delim = |k: usize| [b'(', b')', b'[', b']', b'{', b'}'].iter().any(|&c| sym(k, c));
            let op_at = |e: usize| {
                if kleene(e + 1) {
                    Some(e + 1)
                } else if kleene(e + 2) && !delim(e + 1) {
                    Some(e + 2)
                } else if sym(e + 1, b':') && sym(e + 2, b':') && kleene(e + 3) {
                    Some(e + 3)
                } else {
                    None
                }
            };
            let empty_inner = |mut k: usize, to: usize| {
                while k < to {
                    if sym(k, b'$') && k + 1 < toks.len() && toks[k + 1].0 == 0 && kind_at(k + 2) && tx(k + 3) == b"vis" {
                        k += 4;
                        continue;
                    }
                    if !(sym(k, b'$') && open(k + 1)) {
                        return false;
                    }
                    let Some(o) = op_at(close[k + 1]) else { return false };
                    if !(sym(o, b'*') || sym(o, b'?')) {
                        return false;
                    }
                    k = o + 1;
                }
                true
            };
            let mut seen: Vec<&[u8]> = vec![];
            for q in j + 1..m {
                if !sym(q, b'$') || q + 1 >= toks.len() {
                    continue;
                }
                if toks[q + 1].0 == 0 {
                    let n = tx(q + 1);
                    if kind_at(q + 2) {
                        if seen.contains(&n) {
                            return err(q, "duplicate matcher binding");
                        }
                        seen.push(n);
                    } else if n != b"crate" {
                        return err(q, "missing fragment specifier");
                    }
                } else if sym(q + 1, b'(') && open(q + 1) {
                    let e = close[q + 1];
                    // (at the `(` of `$(`, as rustc)
                    let Some(o) = op_at(e) else { return err(q + 1, "expected one of: `*`, `+`, or `?`") };
                    if o == e + 1 && empty_inner(q + 2, e) {
                        return err(q + 1, "repetition matches empty token tree");
                    }
                }
            }
            j = m + 1;
            if j >= c {
                return err(m, "expected `=>`, found end of macro arguments");
            }
            if !(sym(j, b'=') && sym(j + 1, b'>') && toks[j].2 == toks[j + 1].1) {
                return None;
            }
            j += 2;
            if j >= c {
                return err(j - 1, "macro definition ended unexpectedly");
            }
            if !open(j) {
                return None;
            }
            // (the body: `$[..]` is no meta-variable expression (those are `${..}`), rustc's "expected `(` or `{`,
            // found `[`" at the `[`)
            if let Some(q) = (j + 1..close[j]).find(|&q| sym(q, b'$') && tx(q + 1) == b"[" && toks[q].2 == toks[q + 1].1) {
                return err(q + 1, "expected `(` or `{`, found `[`");
            }
            j = close[j] + 1;
            if sym(j, b';') {
                j += 1;
            } else if j < c {
                return None;
            }
        }
        i = c + 1;
    }
    None
}

thread_local! {
    /// A `$name` of a rule's body that its matcher does not bind, kept in the expansion (an inner `macro_rules!`'s, else
    /// rustc's "cannot find macro parameter"): its name, and whether it is in a repetition of no variable bound there
    pub static UNBOUND: std::cell::RefCell<Option<(String, bool)>> = const { std::cell::RefCell::new(None) };
}

#[cold]
fn unbound(n: &str, rep: bool) {
    UNBOUND.with(|c| {
        if c.borrow().is_none() {
            *c.borrow_mut() = Some((n.to_string(), rep));
        }
    });
}

/// rustc's error for a rule's `$name` its matcher does not bind, when it is left in the expansion (`expanded`): at its
/// first use in a rule's body in src (not a matcher's `$name:frag`), "cannot find macro parameter `$name` in this
/// scope"; or, in a repetition (`rep`), "attempted to repeat an expression containing no syntax variables matched as
/// repeating at this depth" at the repetition's `$`
#[cold]
pub fn unbound_err(expanded: &str, src: &str, name: &str, rep: bool) -> Option<String> {
    let has = |s: &str| -> Option<(Vec<(u8, usize, usize)>, Vec<usize>)> {
        let b = s.as_bytes();
        let toks = crate::lexer::tokenize(b);
        let at: Vec<usize> = (1..toks.len())
            .filter(|&k| toks[k - 1].0 == 1 && b[toks[k - 1].1] == b'$' && toks[k].0 == 0 && &b[toks[k].1..toks[k].2] == name.as_bytes())
            .filter(|&k| !(k + 1 < toks.len() && toks[k + 1].0 == 1 && b[toks[k + 1].1] == b':'))
            .collect();
        (!at.is_empty()).then_some((toks, at))
    };
    // (most expansions keep no `$`: one byte search first)
    if !expanded.contains('$') {
        return None;
    }
    has(expanded)?;
    let (toks, at) = has(src)?;
    let b = src.as_bytes();
    let mut k = at[0] - 1;
    if rep {
        // (the `$(` around it)
        let (close, _) = brackets(b, &toks);
        let o = (1..k).rev().find(|&j| toks[j - 1].0 == 1 && b[toks[j - 1].1] == b'$' && toks[j].0 == 1 && b[toks[j].1] == b'(' && close[j] > k)?;
        k = o - 1;
    }
    let off = toks[k].1;
    let line = 1 + b[..off].iter().filter(|&&c| c == b'\n').count();
    let col = src[..off].chars().rev().take_while(|&c| c != '\n').count() + 1;
    Some(if rep {
        format!("{line}:{col}: attempted to repeat an expression containing no syntax variables matched as repeating at this depth")
    } else {
        format!("{line}:{col}: cannot find macro parameter `${name}` in this scope")
    })
}

thread_local! {
    /// The name of a `#[macro_export]` macro defined twice (E0428), met while the definitions are read
    pub static DUP_EXPORT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// The names of the `#[macro_export]` macros the definitions read had (the driver resets it before an expansion)
    pub static EXPORTED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cold]
fn dup_export(n: &str) {
    DUP_EXPORT.with(|c| {
        if c.borrow().is_none() {
            *c.borrow_mut() = Some(n.to_string());
        }
    });
}

#[cold]
fn bad_fragment(k: &str) {
    BAD_FRAGMENT.with(|c| {
        if c.borrow().is_none() {
            *c.borrow_mut() = Some(k.to_string());
        }
    });
}

fn parse_pt(ts: TokenStream, is_pat: bool) -> Vec<Pt> {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            // (a doc comment in a matcher is ignored, as rustc does)
            TokenTree::Punct(p)
                if is_pat
                    && p.as_char() == '#'
                    && matches!(toks.get(i + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket
                        && matches!(g.stream().into_iter().next(), Some(TokenTree::Ident(d)) if d == "doc")) =>
            {
                i += 2;
            }
            TokenTree::Punct(p) if p.as_char() == '$' && i + 1 < toks.len() => match &toks[i + 1] {
                TokenTree::Ident(id) => {
                    // `$x:expr` (パターン) / `$x` (本体)
                    if is_pat
                        && i + 3 < toks.len()
                        && matches!(&toks[i + 2], TokenTree::Punct(c) if c.as_char() == ':')
                    {
                        if let TokenTree::Ident(k) = &toks[i + 3] {
                            let k = k.to_string();
                            if !FRAGS.contains(&k.as_str()) {
                                bad_fragment(&k);
                            }
                            out.push(Pt::Var(id.to_string(), k));
                            i += 4;
                            continue;
                        }
                    }
                    out.push(Pt::Var(id.to_string(), String::new()));
                    i += 2;
                }
                TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis => {
                    let inner = parse_pt(g.stream(), is_pat);
                    // `$( .. ) sep op` / `$( .. ) op`
                    let mut j = i + 2;
                    let mut sep = None;
                    let mut op = '\0';
                    if let Some(TokenTree::Punct(p)) = toks.get(j) {
                        if matches!(p.as_char(), '*' | '+' | '?') {
                            op = p.as_char();
                            j += 1;
                        } else {
                            sep = Some(toks[j].clone());
                            j += 1;
                            // (`::` as the separator (`$($s:ident)::+`): the joint first `:` stands for the pair)
                            if p.spacing() == Spacing::Joint && matches!(toks.get(j), Some(TokenTree::Punct(q)) if q.as_char() == p.as_char()) {
                                j += 1;
                            } else if let TokenTree::Punct(p) = &toks[j - 1] {
                                // (a lone punct separator is never joint, so a joint one always means the pair)
                                sep = Some(TokenTree::Punct(proc_macro2::Punct::new(p.as_char(), Spacing::Alone)));
                            }
                            if let Some(TokenTree::Punct(q)) = toks.get(j) {
                                if matches!(q.as_char(), '*' | '+' | '?') {
                                    op = q.as_char();
                                    j += 1;
                                }
                            }
                        }
                    } else if let Some(t) = toks.get(j) {
                        sep = Some(t.clone());
                        j += 1;
                        if let Some(TokenTree::Punct(q)) = toks.get(j) {
                            op = q.as_char();
                            j += 1;
                        }
                    }
                    // (`$($b:tt)` with no `*` / `+` / `?` after it: rustc's error in a matcher, as `*` here)
                    if !matches!(op, '*' | '+' | '?') {
                        if is_pat {
                            BAD_DEF.with(|c| c.set(true));
                        }
                        op = '*';
                    }
                    out.push(Pt::Rep(inner, sep, op));
                    i = j;
                }
                _ => {
                    out.push(Pt::Tok(toks[i].clone()));
                    i += 1;
                }
            },
            TokenTree::Group(g) => {
                out.push(Pt::Group(g.delimiter(), parse_pt(g.stream(), is_pat)));
                i += 1;
            }
            t => {
                out.push(Pt::Tok(t.clone()));
                i += 1;
            }
        }
    }
    out
}

fn same_tok(a: &TokenTree, b: &TokenTree) -> bool {
    match (a, b) {
        // (a rule's literal name matches the name renamed for hygiene, `closure__mh`, as rustc matches names)
        (TokenTree::Ident(x), TokenTree::Ident(y)) => {
            x == y || {
                let (x, y) = (x.to_string(), y.to_string());
                let base = |s: &str| -> String {
                    let mut s = s;
                    while let Some(t) = s.strip_suffix("__mh") {
                        s = t;
                    }
                    s.to_string()
                };
                (x.ends_with("__mh") || y.ends_with("__mh")) && base(&x) == base(&y)
            }
        }
        (TokenTree::Punct(x), TokenTree::Punct(y)) => x.as_char() == y.as_char(),
        (TokenTree::Literal(x), TokenTree::Literal(y)) => x.to_string() == y.to_string(),
        // (目印としてのグループ: 括弧の種類が同じなら)
        (TokenTree::Group(x), TokenTree::Group(y)) => x.delimiter() == y.delimiter(),
        _ => false,
    }
}

/// パターンの位置 k から後ろで最初に現れる具体的なトークン (断片の終わりの目印)
fn next_lit(pat: &[Pt], k: usize, follow: Option<&TokenTree>) -> Option<TokenTree> {
    for p in &pat[k..] {
        match p {
            Pt::Tok(t) => return Some(t.clone()),
            // (`$t:ty { .. }`: 次が括弧のグループなら、同じ種類の括弧で断片が終わる)
            Pt::Group(d, _) if *d != Delimiter::None => {
                return Some(TokenTree::Group(proc_macro2::Group::new(*d, TokenStream::new())));
            }
            Pt::Group(..) => return None,
            Pt::Rep(_, Some(s), _) => return Some(s.clone()),
            _ => return None,
        }
    }
    follow.cloned()
}

/// 式の中でオペランドにならない (後ろに続きがある) キーワード
fn is_expr_keyword(s: &str) -> bool {
    matches!(
        s,
        "as" | "async" | "box" | "break" | "const" | "continue" | "crate" | "dyn" | "else" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match" | "move" | "mut" | "ref" | "return" | "static" | "unsafe" | "while" | "where" | "yield"
    )
}

/// 1 つの断片を取る。取った数 (と、式として包むか)
fn take_frag(toks: &[TokenTree], kind: &str, stop: Option<&TokenTree>) -> Option<(usize, bool)> {
    match kind {
        "ident" | "lifetime" | "literal" | "tt" => {
            let first = toks.first()?;
            // (the kind decides which rule matches: `($i:ident)` does not take `1`, `($l:literal)` not `x`)
            match (kind, first) {
                ("ident", TokenTree::Ident(i)) if i != "_" => {}
                ("literal", TokenTree::Literal(_)) | ("literal", TokenTree::Punct(_)) => {}
                ("literal", TokenTree::Ident(i)) if i == "true" || i == "false" => return Some((1, false)),
                ("lifetime", TokenTree::Punct(p)) if p.as_char() == '\'' => {}
                ("tt", _) => {}
                _ => return None,
            }
            // (負のリテラル `-1` は 2 トークン)
            if kind == "literal" {
                if let (TokenTree::Punct(p), Some(TokenTree::Literal(_))) = (&toks[0], toks.get(1)) {
                    if p.as_char() == '-' {
                        return Some((2, false));
                    }
                }
                if !matches!(first, TokenTree::Literal(_)) {
                    return None;
                }
            }
            if kind == "lifetime" {
                return Some((2.min(toks.len()), false));
            }
            // (a lifetime is one token tree (`$(<$l:tt>)*` of `<'de>`), though lexed as `'` and its name)
            if kind == "tt" && matches!((first, toks.get(1)), (TokenTree::Punct(p), Some(TokenTree::Ident(_))) if p.as_char() == '\'' && p.spacing() == Spacing::Joint) {
                return Some((2, false));
            }
            Some((1, false))
        }
        // (`pub` / `pub(crate)` / `pub(in path)` or nothing)
        "vis" => match toks.first() {
            Some(TokenTree::Ident(i)) if i == "pub" => match toks.get(1) {
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                    let restr = matches!(g.stream().into_iter().next(), Some(TokenTree::Ident(k)) if k == "crate" || k == "self" || k == "super" || k == "in");
                    Some((if restr { 2 } else { 1 }, false))
                }
                _ => Some((1, false)),
            },
            _ => Some((0, false)),
        },
        "block" => match toks.first() {
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => Some((1, false)),
            _ => None,
        },
        // (one item: to its `;`, or to its `{ .. }` (fn, struct, enum, impl, trait, mod, a macro call); `const` /
        //  `static` / `use` / `type` / `extern crate` to their `;` only (`const X: S = S { .. };`). A group is one token,
        //  so `[u8; 3]` does not end it. `$($it:item)*` takes one item each: libc 0.2's cfg_if puts its cfg on each)
        "item" => {
            let mut n = 0;
            while matches!(toks.get(n), Some(TokenTree::Punct(p)) if p.as_char() == '#')
                && matches!(toks.get(n + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket)
            {
                n += 2;
            }
            let mut k = n;
            let mut semi = false;
            while let Some(t) = toks.get(k) {
                match t {
                    TokenTree::Ident(i) => match i.to_string().as_str() {
                        "pub" => {
                            k += 1;
                            if matches!(toks.get(k), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis) {
                                k += 1;
                            }
                        }
                        "unsafe" | "async" | "extern" | "default" | "safe" => k += 1,
                        "const" => {
                            semi = !matches!(toks.get(k + 1), Some(TokenTree::Ident(j)) if matches!(j.to_string().as_str(), "fn" | "unsafe" | "async" | "extern"));
                            break;
                        }
                        "static" | "use" | "type" => {
                            semi = true;
                            break;
                        }
                        "crate" => {
                            semi = k > n;
                            break;
                        }
                        _ => break,
                    },
                    // (`extern "C"`)
                    TokenTree::Literal(_) => k += 1,
                    _ => break,
                }
            }
            let mut m = n;
            // (inside generic arguments a brace is a const argument, not the body: `struct S<const N: usize = { 1 }>;`.
            //  The `>` of `->` closes nothing)
            let mut angle = 0u32;
            while m < toks.len() {
                match &toks[m] {
                    TokenTree::Punct(p) if p.as_char() == '<' => angle += 1,
                    TokenTree::Punct(p) if p.as_char() == '>' && angle > 0
                        && !(m > 0 && matches!(&toks[m - 1], TokenTree::Punct(q) if q.as_char() == '-' && q.spacing() == Spacing::Joint)) =>
                    {
                        angle -= 1
                    }
                    TokenTree::Punct(p) if p.as_char() == ';' && angle == 0 => return Some((m + 1, false)),
                    TokenTree::Group(g) if !semi && angle == 0 && g.delimiter() == Delimiter::Brace => return Some((m + 1, false)),
                    _ => {}
                }
                m += 1;
            }
            if m == 0 { None } else { Some((m, false)) }
        }
        _ => {
            // expr / ty / pat / path / stmt / item: 次の区切りまで (グループの中は見ない)
            let mut n = 0;
            // (式: オペランドの後にオペランドが続いたら式は終わっている: `$($len:expr)+` の `1 2 3`)
            let mut after_operand = false;
            // (a type / path keeps its generic arguments: `HashMap<K, V>` does not end at the `,`)
            let angles = matches!(kind, "ty" | "path");
            let mut angle: u32 = 0;
            // (a type: after a whole one (a name, a group, closed generic arguments), another name, `&`, `*` or `[..]`
            // starts the next: `$($t:ty)*` of `u8 u16 String`. A modifier (`dyn`, `mut`, `fn`, ..) and a lifetime's
            // name go on)
            let mut ty_end = false;
            let mut quote = false;
            // (the type so far ends in a name: `Fn(u8)`'s parentheses go on it, `Vec<u8> (bool, char)`'s start the next)
            let mut named = false;
            // (`m!` before its group: a type macro, `m!(u8)` / `m![u8]`)
            let mut mac = false;
            // (`for` before `<`, and generic arguments that are its binder: `for<'a> fn(&'a u8)` goes on after the `>`)
            let mut for_kw = false;
            let mut binder = false;
            // (a lifetime after `+` is a bound and can end the type: `dyn Display + 'static`; after `&` it goes on)
            let mut plus = false;
            let mut lt_bound = false;
            while n < toks.len() {
                if angle > 0 {
                    if let TokenTree::Punct(p) = &toks[n] {
                        match p.as_char() {
                            '<' => angle += 1,
                            // (`->` in `Fn() -> T` is not a closing angle)
                            '>' if !(n > 0 && matches!(&toks[n - 1], TokenTree::Punct(q) if q.as_char() == '-' && q.spacing() == Spacing::Joint)) => {
                                angle -= 1;
                                ty_end = angle == 0 && !binder;
                                named = false;
                            }
                            ';' => break,
                            _ => {}
                        }
                    }
                    n += 1;
                    continue;
                }
                if kind == "ty" && stop.is_none() {
                    match &toks[n] {
                        TokenTree::Ident(i) => {
                            let w = i.to_string();
                            if ty_end && !quote && w != "as" {
                                break;
                            }
                            ty_end = if quote { lt_bound } else { !matches!(w.as_str(), "dyn" | "impl" | "mut" | "const" | "fn" | "unsafe" | "extern" | "for" | "as" | "async") };
                            named = ty_end && !quote;
                            plus = false;
                            mac = false;
                            for_kw = w == "for";
                            quote = false;
                        }
                        TokenTree::Group(g) => {
                            if ty_end && !mac && (g.delimiter() != Delimiter::Parenthesis || !named) {
                                break;
                            }
                            ty_end = true;
                            named = false;
                            mac = false;
                            for_kw = false;
                            plus = false;
                        }
                        TokenTree::Punct(p) => {
                            if ty_end && matches!(p.as_char(), '&' | '*') {
                                break;
                            }
                            quote = p.as_char() == '\'';
                            if quote {
                                lt_bound = plus;
                            }
                            plus = p.as_char() == '+';
                            mac = p.as_char() == '!' && named;
                            ty_end = p.as_char() == '!';
                            named = false;
                            for_kw = for_kw && p.as_char() == '<';
                        }
                        TokenTree::Literal(_) => {
                            if ty_end {
                                break;
                            }
                        }
                    }
                }
                if let Some(s) = stop {
                    if same_tok(&toks[n], s) {
                        break;
                    }
                }
                // (an expression's turbofish `f::<A, B>` / qualified path `<T as Tr>::f`)
                if matches!(&toks[n], TokenTree::Punct(p) if p.as_char() == '<')
                    && (angles || (kind == "expr" && (n == 0 || matches!(&toks[n - 1], TokenTree::Punct(q) if q.as_char() == ':'))))
                {
                    angle = 1;
                    binder = std::mem::take(&mut for_kw);
                    n += 1;
                    continue;
                }
                if kind == "expr" {
                    // (an attribute on the expression, `#[allow(x)] 0`: not an operand)
                    if matches!(&toks[n], TokenTree::Punct(p) if p.as_char() == '#')
                        && matches!(toks.get(n + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket)
                    {
                        n += 2;
                        continue;
                    }
                    let operand_start = match &toks[n] {
                        TokenTree::Literal(_) => true,
                        TokenTree::Ident(i) => !is_expr_keyword(&i.to_string()),
                        _ => false,
                    };
                    if after_operand && operand_start {
                        break;
                    }
                    after_operand = match &toks[n] {
                        TokenTree::Literal(_) | TokenTree::Group(_) => true,
                        TokenTree::Ident(i) => !is_expr_keyword(&i.to_string()),
                        _ => false,
                    };
                }
                // 式は `;` で、型は `,` で必ず終わる
                if let TokenTree::Punct(p) = &toks[n] {
                    if p.as_char() == ';' && kind != "item" {
                        break;
                    }
                    if p.as_char() == ',' && stop.is_none() && matches!(kind, "expr" | "ty" | "pat" | "path") {
                        break;
                    }
                    // `=>` は式の中に現れない
                    if p.as_char() == '=' && p.spacing() == Spacing::Joint {
                        if let Some(TokenTree::Punct(q)) = toks.get(n + 1) {
                            if q.as_char() == '>' {
                                break;
                            }
                        }
                    }
                }
                n += 1;
            }
            if n == 0 {
                return None;
            }
            // (1 つのトークン (リテラル・名前・括弧) なら包まない: `[u8; $len]` の長さ)
            Some((n, kind == "expr" && n > 1))
        }
    }
}

/// パターン pat を toks 全体に合わせる
/// How far a rule's leading literal tokens reach into a call's tokens: its tokens and delimited groups (not looked
/// into), `$x:tt`; it stops at a fragment of another kind or a repetition, whose length it does not know
fn literal_reach(pat: &[Pt], toks: &[TokenTree]) -> usize {
    let mut i = 0;
    for p in pat {
        match (p, toks.get(i)) {
            (Pt::Tok(t), Some(x)) if same_tok(t, x) => i += 1,
            (Pt::Group(d, _), Some(TokenTree::Group(g))) if g.delimiter() == *d => i += 1,
            (Pt::Var(_, k), Some(_)) if k == "tt" => i += 1,
            _ => break,
        }
    }
    i
}

/// "no rules expected `X`" at the token of a call's arguments (text[a..b]) past the furthest any rule's literal
/// tokens reach, when one reaches past the first: None when none does, or the arguments do not tokenize
#[cold]
#[inline(never)]
fn no_rules_expected(text: &str, a: usize, b: usize, rules: Option<&Vec<Rule>>) -> Option<String> {
    let args: Vec<TokenTree> = TokenStream::from_str(text.get(a..b)?).ok()?.into_iter().collect();
    let best = rules?.iter().map(|r| literal_reach(&r.pat, &args)).max()?;
    let t = args.get(best).filter(|_| best > 0)?;
    let what = match t {
        TokenTree::Group(g) => match g.delimiter() {
            Delimiter::Parenthesis => "(".to_string(),
            Delimiter::Bracket => "[".to_string(),
            Delimiter::Brace => "{".to_string(),
            Delimiter::None => return None,
        },
        x => x.to_string(),
    };
    let lc = t.span().start();
    let base_line = text[..a].matches('\n').count() + 1;
    let base_col = a - text[..a].rfind('\n').map_or(0, |k| k + 1);
    let line = base_line + lc.line - 1;
    let col = if lc.line == 1 { base_col + lc.column } else { lc.column };
    Some(format!("{line}:{}: no rules expected `{what}`", col + 1))
}

fn match_pat(pat: &[Pt], toks: &[TokenTree], binds: &mut HashMap<String, Bind>) -> bool {
    match match_prefix(pat, toks, binds, None) {
        Some(n) => n == toks.len(),
        None => false,
    }
}

/// pat を toks の前から合わせ、使ったトークン数を返す
fn match_prefix(pat: &[Pt], toks: &[TokenTree], binds: &mut HashMap<String, Bind>, follow: Option<&TokenTree>) -> Option<usize> {
    let mut i = 0;
    for (k, p) in pat.iter().enumerate() {
        match p {
            Pt::Tok(t) => {
                if i < toks.len() && same_tok(t, &toks[i]) {
                    i += 1;
                } else {
                    return None;
                }
            }
            Pt::Group(d, inner) => match toks.get(i) {
                Some(TokenTree::Group(g)) if g.delimiter() == *d => {
                    let sub: Vec<TokenTree> = g.stream().into_iter().collect();
                    if !match_pat(inner, &sub, binds) {
                        return None;
                    }
                    i += 1;
                }
                _ => return None,
            },
            Pt::Var(name, kind) => {
                let stop = next_lit(pat, k + 1, follow);
                let (n, wrap) = take_frag(&toks[i..], kind, stop.as_ref())?;
                binds.insert(name.clone(), Bind::One(toks[i..i + n].to_vec(), wrap));
                i += n;
            }
            Pt::Rep(inner, sep, op) => {
                let stop = next_lit(pat, k + 1, follow);
                let mut items: Vec<HashMap<String, Bind>> = vec![];
                // (where the last separator was: `$($x:ident),* ,` gives the trailing `,` back when no item follows it)
                let mut sep_at: Option<usize> = None;
                loop {
                    if i >= toks.len() {
                        break;
                    }
                    if let Some(s) = &stop {
                        if sep.as_ref().is_none_or(|x| !same_tok(x, s)) && same_tok(&toks[i], s) {
                            break;
                        }
                    }
                    let mut b = HashMap::default();
                    let Some(n) = match_prefix(inner, &toks[i..], &mut b, sep.as_ref().or(stop.as_ref())) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    i += n;
                    items.push(b);
                    sep_at = None;
                    if *op == '?' {
                        break;
                    }
                    match sep {
                        Some(s) => {
                            let pair = matches!(s, TokenTree::Punct(p) if p.spacing() == Spacing::Joint);
                            if i < toks.len() && same_tok(s, &toks[i]) && (!pair || toks.get(i + 1).is_some_and(|t| same_tok(s, t))) {
                                sep_at = Some(i);
                                i += if pair { 2 } else { 1 };
                            } else {
                                break;
                            }
                        }
                        None => {}
                    }
                }
                if let Some(j) = sep_at {
                    i = j;
                }
                if *op == '+' && items.is_empty() {
                    return None;
                }
                // 変数ごとに Many にまとめる
                let mut names = vec![];
                collect_vars(inner, &mut names);
                for n in names {
                    let v = items.iter().map(|b| b.get(&n).cloned().unwrap_or(Bind::Many(vec![]))).collect();
                    binds.insert(n, Bind::Many(v));
                }
            }
        }
    }
    Some(i)
}

fn collect_vars(p: &[Pt], out: &mut Vec<String>) {
    for x in p {
        match x {
            Pt::Var(n, _) => out.push(n.clone()),
            Pt::Group(_, i) | Pt::Rep(i, _, _) => collect_vars(i, out),
            Pt::Tok(_) => {}
        }
    }
}

/// A repetition's separator: a joint punct stands for the pair of it (`::`)
fn sep_text(t: &TokenTree, out: &mut String) {
    tt_text(t, out);
    if let TokenTree::Punct(p) = t {
        if p.spacing() == Spacing::Joint {
            out.push(p.as_char());
            out.push(' ');
        }
    }
}

fn tt_text(t: &TokenTree, out: &mut String) {
    match t {
        TokenTree::Group(g) => {
            let (o, c) = match g.delimiter() {
                Delimiter::Parenthesis => ("(", ")"),
                Delimiter::Brace => ("{", "}"),
                Delimiter::Bracket => ("[", "]"),
                Delimiter::None => ("", ""),
            };
            out.push_str(o);
            for x in g.stream() {
                tt_text(&x, out);
            }
            out.push_str(c);
            out.push(' ');
        }
        TokenTree::Punct(p) => {
            out.push(p.as_char());
            if p.spacing() == Spacing::Alone {
                out.push(' ');
            }
        }
        TokenTree::Ident(i) => {
            out.push_str(&i.to_string());
            out.push(' ');
        }
        TokenTree::Literal(l) => {
            // (1 行にするので、文字列の中の改行はエスケープに直す)
            let s = l.to_string();
            if s.starts_with('r') && s.contains('\n') {
                out.push_str(&format!("{:?}", s.trim_start_matches('r').trim_matches('#').trim_matches('"')));
            } else {
                out.push_str(&s.replace('\n', "\\n"));
            }
            out.push(' ');
        }
    }
}

/// Whether every attribute in an expansion is `#[cfg(..)]` / `#[cfg_attr(..)]` (cfg-if's): evaluated with what they are
/// on, they leave nothing behind, so the expansion may be spliced into a block as statements
fn only_cfg_attrs(body: &str) -> bool {
    body.match_indices('#').all(|(k, _)| {
        let r = body[k + 1..].trim_start();
        r.strip_prefix('[').is_some_and(|r| {
            let r = r.trim_start();
            r.starts_with("cfg(") || r.starts_with("cfg (") || r.starts_with("cfg_attr")
        })
    })
}

/// The names a macro body binds with `let` (not from its variables): hygiene keeps them apart from the caller's names
/// (`macro_rules! m { ($e:expr) => {{ let x = 9; $e }} }` with `m!(x)` uses the caller's x)
fn body_lets(body: &[Pt], out: &mut Vec<String>) {
    let mut i = 0;
    while i < body.len() {
        match &body[i] {
            Pt::Tok(TokenTree::Ident(k)) if k == "let" => {
                let mut j = i + 1;
                if matches!(body.get(j), Some(Pt::Tok(TokenTree::Ident(m))) if m == "mut") {
                    j += 1;
                }
                // (a plain binding only: `let x = ..` / `let x: T` / `let x;`, not `if let Some(..) =`, nor a path's `::`,
                // `if let DecoderResult::InputEmpty = r` (encoding_rs))
                let path = matches!((body.get(j + 1), body.get(j + 2)), (Some(Pt::Tok(TokenTree::Punct(a))), Some(Pt::Tok(TokenTree::Punct(b)))) if a.as_char() == ':' && b.as_char() == ':');
                let plain = !path && matches!(body.get(j + 1), Some(Pt::Tok(TokenTree::Punct(p))) if matches!(p.as_char(), '=' | ':' | ';'));
                if let (Some(Pt::Tok(TokenTree::Ident(n))), true) = (body.get(j), plain) {
                    let n = n.to_string();
                    if n != "_" && !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            // (a match arm binding just a name, `_x => ..`: at an arm's start, the group's first token or after a `,`,
            // a `|` or an arm's block. Not `*self => $i` in a macro call's arguments (num-traits' `float_to_int_unchecked!`))
            Pt::Tok(TokenTree::Ident(n))
                if matches!((body.get(i + 1), body.get(i + 2)), (Some(Pt::Tok(TokenTree::Punct(a))), Some(Pt::Tok(TokenTree::Punct(b)))) if a.as_char() == '=' && b.as_char() == '>')
                    && (i == 0
                        || matches!(&body[i - 1], Pt::Tok(TokenTree::Punct(c)) if c.as_char() == ',' || c.as_char() == '|')
                        || matches!(&body[i - 1], Pt::Group(Delimiter::Brace, _))) =>
            {
                let n = n.to_string();
                if !matches!(n.as_str(), "_" | "true" | "false" | "self" | "super" | "crate") && n.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') && !out.contains(&n) {
                    out.push(n);
                }
            }
            // (a label defined in the body, `'x: loop`: kept as "'x")
            Pt::Tok(TokenTree::Punct(q)) if q.as_char() == '\'' => {
                if let (Some(Pt::Tok(TokenTree::Ident(n))), Some(Pt::Tok(TokenTree::Punct(c)))) = (body.get(i + 1), body.get(i + 2)) {
                    let n = format!("'{n}");
                    if c.as_char() == ':' && !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            // (not in a module's body: its lets are in its fns', which see no caller's locals)
            Pt::Group(Delimiter::Brace, _) if is_mod_body(body, i) => {}
            Pt::Group(_, inner) | Pt::Rep(inner, _, _) => body_lets(inner, out),
            _ => {}
        }
        i += 1;
    }
}

/// body[k] is the body of a module `mod m { .. }` (a module file put in a macro's body, serde_core's `crate_root!`):
/// hygiene stays out of it, as its lets are in its fns' bodies, which see no caller's locals, and the renaming, by
/// name alone, would reach other uses of the name (`seq` of `forward_to_deserialize_any! { seq }`)
fn is_mod_body(body: &[Pt], k: usize) -> bool {
    k >= 2 && matches!(&body[k], Pt::Group(Delimiter::Brace, _)) && matches!(&body[k - 1], Pt::Tok(TokenTree::Ident(_))) && matches!(&body[k - 2], Pt::Tok(TokenTree::Ident(m)) if m == "mod")
}

/// transcribe with hygiene for the body's own `let` names
fn transcribe_h(body: &[Pt], binds: &HashMap<String, Bind>, out: &mut String) -> Result<(), String> {
    let mut lets = vec![];
    body_lets(body, &mut lets);
    transcribe(body, &Binds::Root(binds), out, &lets)
}

/// The bindings a transcription reads: the match's, and over them, for each repetition being transcribed, its
/// variables' bindings at that step (a layer of references: copying the whole map at each step was quadratic)
enum Binds<'a> {
    Root(&'a HashMap<String, Bind>),
    Layer(HashMap<&'a str, &'a Bind>, &'a Binds<'a>),
}

impl<'a> Binds<'a> {
    fn get(&self, name: &str) -> Option<&'a Bind> {
        match self {
            Binds::Root(m) => m.get(name),
            Binds::Layer(m, up) => m.get(name).copied().or_else(|| up.get(name)),
        }
    }
}

fn transcribe(body: &[Pt], binds: &Binds, out: &mut String, lets: &[String]) -> Result<(), String> {
    let punct = |k: usize, c: char| matches!(body.get(k), Some(Pt::Tok(TokenTree::Punct(q))) if q.as_char() == c);
    for (k, p) in body.iter().enumerate() {
        match p {
            // (a field or method of a let's name is not renamed: `r.core`, `x.seed()` (not the range `..core`). Field names
            //  in struct literals, patterns and definitions drop the hygiene suffix where they are parsed)
            Pt::Tok(TokenTree::Ident(i)) if lets.iter().any(|l| *i == l.as_str()) && k > 0 && punct(k - 1, '.') && !(k > 1 && punct(k - 2, '.')) => {
                tt_text(&TokenTree::Ident(i.clone()), out);
            }
            // (a label of the body's own: `'x` -> `'x__mh`)
            Pt::Tok(TokenTree::Ident(i))
                if k > 0
                    && matches!(&body[k - 1], Pt::Tok(TokenTree::Punct(q)) if q.as_char() == '\'')
                    && lets.iter().any(|l| l.strip_prefix('\'') == Some(i.to_string().as_str())) =>
            {
                out.push_str(&i.to_string());
                out.push_str("__mh ");
            }
            // (not a tt-muncher's marker, `closures!(@closure ..)` in a body with `let closure`: the marker is matched
            // against a rule's `@closure`, as rustc matches it by name. A binding's `@`, `whole @ x`, has a name
            // before it: its `x` is the body's own)
            // (nor a field or method of the name, `vector.len()` / `js.len` beside `let len`. A field's name in a struct
            // literal, `WasmSlice { len: n }`, is renamed as well: the parser drops the suffix of field names)
            Pt::Tok(TokenTree::Ident(i))
                if lets.iter().any(|l| *i == l.as_str())
                    && !(k > 0
                        && matches!(&body[k - 1], Pt::Tok(TokenTree::Punct(q)) if q.as_char() == '@')
                        && !(k > 1 && matches!(&body[k - 2], Pt::Tok(TokenTree::Ident(_)) | Pt::Var(..))))
                    && !(k > 0
                        && matches!(&body[k - 1], Pt::Tok(TokenTree::Punct(q)) if q.as_char() == '.')
                        && !(k > 1 && matches!(&body[k - 2], Pt::Tok(TokenTree::Punct(q)) if q.as_char() == '.'))) =>
            {
                out.push_str(&i.to_string());
                out.push_str("__mh ");
            }
            Pt::Tok(t) => tt_text(t, out),
            Pt::Group(d, inner) => {
                let (o, c) = match d {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Brace => ("{", "}"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::None => ("", ""),
                };
                out.push_str(o);
                // (a module's body in the expansion is no block: its names are items, not the body's `let`s)
                let inner_lets: &[String] = if is_mod_body(body, k) { &[] } else { lets };
                // (a struct literal's shorthand fields of the body's own names, `MutSlice { contents, js }`: the field
                // keeps its name, the value is the renamed local, `contents: contents__mh`. A type's name before the
                // braces, and a `,` among them: a block does not have that shape. Nor an item's body, `enum E { value,
                // other }`: its names are variants)
                let shorthand = *d == Delimiter::Brace
                    && k > 0
                    && matches!(&body[k - 1], Pt::Tok(TokenTree::Ident(n)) if n.to_string().starts_with(|c: char| c.is_ascii_uppercase()))
                    && !(k > 1 && matches!(&body[k - 2], Pt::Tok(TokenTree::Ident(kw)) if matches!(kw.to_string().as_str(), "enum" | "struct" | "union" | "trait" | "impl" | "mod")))
                    && inner.iter().any(|p| matches!(p, Pt::Tok(TokenTree::Punct(q)) if q.as_char() == ','));
                if shorthand && !inner_lets.is_empty() {
                    let mut fixed: Vec<Pt> = Vec::with_capacity(inner.len());
                    let sep = |p: Option<&Pt>| p.is_none_or(|p| matches!(p, Pt::Tok(TokenTree::Punct(q)) if q.as_char() == ','));
                    let mut parts: Vec<(usize, String)> = vec![];
                    for (j, p) in inner.iter().enumerate() {
                        if let Pt::Tok(TokenTree::Ident(n)) = p {
                            let n = n.to_string();
                            if inner_lets.iter().any(|l| *l == n) && (j == 0 || sep(inner.get(j - 1))) && sep(inner.get(j + 1)) {
                                parts.push((j, n));
                            }
                        }
                    }
                    if parts.is_empty() {
                        transcribe(inner, binds, out, inner_lets)?;
                    } else {
                        let mut last = 0;
                        for (j, n) in &parts {
                            fixed.clear();
                            fixed.extend_from_slice(&inner[last..*j]);
                            transcribe(&fixed, binds, out, inner_lets)?;
                            out.push_str(&format!("{n}: {n}__mh "));
                            last = j + 1;
                        }
                        fixed.clear();
                        fixed.extend_from_slice(&inner[last..]);
                        transcribe(&fixed, binds, out, inner_lets)?;
                    }
                } else {
                    transcribe(inner, binds, out, inner_lets)?;
                }
                out.push_str(c);
                out.push(' ');
            }
            Pt::Var(name, _) => match binds.get(name) {
                Some(Bind::One(ts, wrap)) => {
                    // (`stringify!($e)`: the text of the expression, without the parentheses that keep its precedence)
                    let wrap = &(*wrap && !{
                        let tail: String = out[out.len().saturating_sub(40)..].chars().filter(|c| !c.is_whitespace()).collect();
                        tail.ends_with("stringify!(")
                    });
                    if *wrap {
                        out.push('(');
                    }
                    for t in ts {
                        tt_text(t, out);
                    }
                    if *wrap {
                        out.push(')');
                    }
                    // (`$t:tt` の `-` の後に `>` が続く (`->`): Joint の記号の後には空白を入れない)
                    let joint = !*wrap && matches!(ts.last(), Some(TokenTree::Punct(p)) if p.spacing() == Spacing::Joint);
                    if !joint {
                        out.push(' ');
                    }
                }
                Some(Bind::Many(_)) => return Err(format!("macro variable `${name}` is still repeating here")),
                // (`$crate` など、束縛されていない `$x` はそのまま)
                // (`$crate::m::f` は `crate::m::f`: モジュールは平らにして最後のセグメントで引く)
                None => {
                    if name == "crate" {
                        out.push_str("crate");
                    } else {
                        unbound(name, false);
                        out.push('$');
                        out.push_str(name);
                    }
                    out.push(' ');
                }
            },
            Pt::Rep(inner, sep, op) => {
                let mut names = vec![];
                collect_vars(inner, &mut names);
                // (no variable of it bound here: a repetition of an inner `macro_rules!` that this body holds (a module
                // file put in a macro's body, serde_core's `crate_root!`): kept as written, for that macro)
                if names.iter().all(|v| binds.get(v).is_none()) {
                    if let Some(n) = names.first() {
                        unbound(n, true);
                    }
                    out.push_str("$(");
                    transcribe(inner, binds, out, lets)?;
                    out.push_str(") ");
                    if let Some(s) = sep {
                        sep_text(s, out);
                    }
                    out.push(*op);
                    out.push(' ');
                    continue;
                }
                let n = names
                    .iter()
                    .filter_map(|v| match binds.get(v) {
                        Some(Bind::Many(xs)) => Some(xs.len()),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                for k in 0..n {
                    if k > 0 {
                        if let Some(s) = sep {
                            sep_text(s, out);
                        }
                    }
                    let mut layer: HashMap<&str, &Bind> = HashMap::default();
                    for v in &names {
                        if let Some(Bind::Many(xs)) = binds.get(v) {
                            if let Some(x) = xs.get(k) {
                                layer.insert(v.as_str(), x);
                            }
                        }
                    }
                    transcribe(inner, &Binds::Layer(layer, binds), out, lets)?;
                }
            }
        }
    }
    Ok(())
}

#[cold]
fn bad_follow(e: (String, String)) {
    BAD_FOLLOW.with(|c| {
        if c.borrow().is_none() {
            *c.borrow_mut() = Some(e);
        }
    });
}

/// A token that can come first in a part of a matcher, as rustc's FIRST sets have it: a token's text (`=>` as one), a
/// fragment (its name and kind), or the end of a delimited group
#[derive(Clone)]
enum Ft {
    Tok(String),
    Frag(String, String),
    Close,
}

/// The tokens one of rustc's (`=>`, `>>`, `::`) is made of, written together
const GLUED: &[&str] = &["..=", "...", "<<=", ">>=", "=>", ">>", "<<", "::", "->", "<-", "==", "!=", "<=", ">=", "&&", "||", "..", "+=", "-=", "*=", "/=", "%=", "^=", "&=", "|="];

/// The first token of seq[i..] when seq[i] is no repetition
fn first_tok(seq: &[Pt], i: usize) -> Ft {
    match &seq[i] {
        Pt::Tok(TokenTree::Punct(p)) => {
            let mut s = p.as_char().to_string();
            let (mut k, mut sp) = (i, p.spacing());
            while sp == Spacing::Joint && s.len() < 3 {
                let Some(Pt::Tok(TokenTree::Punct(q))) = seq.get(k + 1) else { break };
                s.push(q.as_char());
                k += 1;
                sp = q.spacing();
            }
            while s.len() > 1 && !GLUED.contains(&s.as_str()) {
                s.pop();
            }
            Ft::Tok(s)
        }
        Pt::Tok(t) => Ft::Tok(t.to_string()),
        Pt::Group(d, _) => Ft::Tok(match d {
            Delimiter::Parenthesis => "(",
            Delimiter::Bracket => "[",
            Delimiter::Brace => "{",
            Delimiter::None => "",
        }.to_string()),
        Pt::Var(n, k) => Ft::Frag(n.clone(), k.clone()),
        Pt::Rep(..) => Ft::Close,
    }
}

/// FIRST of a matcher's sequence: its tokens, and whether it can match nothing (rustc's FirstSets)
fn first_of(seq: &[Pt]) -> (Vec<Ft>, bool) {
    let mut out = vec![];
    for i in 0..seq.len() {
        let Pt::Rep(inner, sep, op) = &seq[i] else {
            out.push(first_tok(seq, i));
            return (out, false);
        };
        let (f, empty) = first_of(inner);
        out.extend(f);
        if empty {
            if let Some(s) = sep {
                out.push(Ft::Tok(s.to_string()));
            }
        }
        if *op == '+' && !empty {
            return (out, false);
        }
    }
    (out, true)
}

/// Whether a fragment of kind k may be followed by t (rustc 1.100's is_in_follow; `vis` taken as followed by anything)
fn in_follow(t: &Ft, k: &str, pat_with_or: bool) -> bool {
    let tok = |s: &str| matches!(t, Ft::Tok(x) if x == s);
    match t {
        Ft::Close => return true,
        Ft::Frag(_, fk) if fk.is_empty() => return true,
        _ => {}
    }
    match k {
        "expr" | "expr_2021" | "stmt" => tok("=>") || tok(",") || tok(";"),
        "pat" | "pat_param" => tok("=>") || tok(",") || tok("=") || tok("if") || tok("in") || (tok("|") && !(k == "pat" && pat_with_or)),
        "path" | "ty" => ["=>", ",", "=", "|", ";", ":", ">", ">>", "[", "{", "as", "where"].iter().any(|s| tok(s)) || matches!(t, Ft::Frag(_, fk) if fk == "block"),
        _ => true,
    }
}

/// rustc's check_matcher: each fragment of seq against what can follow it (follow: what follows seq). Ok(the
/// fragments that can end seq, whether seq can match nothing), or Err((the fragment, rustc's message))
fn check_follow<'a>(seq: &'a [Pt], follow: &[Ft], pat_with_or: bool) -> Result<(Vec<(&'a str, &'a str)>, bool), (String, String)> {
    let mut last: Vec<(&str, &str)> = vec![];
    let mut empty = true;
    for i in 0..seq.len() {
        let (mut next, rest_empty) = first_of(&seq[i + 1..]);
        if rest_empty {
            next.extend(follow.iter().cloned());
        }
        match &seq[i] {
            Pt::Var(n, k) => {
                last = vec![(n.as_str(), k.as_str())];
                empty = false;
            }
            Pt::Tok(_) => {
                last.clear();
                empty = false;
            }
            Pt::Group(_, inner) => {
                check_follow(inner, &[Ft::Close], pat_with_or)?;
                last.clear();
                empty = false;
            }
            Pt::Rep(inner, sep, _) => {
                let mut mine = next.clone();
                if let Some(s) = sep {
                    mine.push(Ft::Tok(s.to_string()));
                }
                let (l, e) = check_follow(inner, &mine, pat_with_or)?;
                if e {
                    last.extend(l);
                } else {
                    last = l;
                    empty = false;
                }
            }
        }
        for &(n, k) in &last {
            if let Some(t) = next.iter().find(|t| !in_follow(t, k, pat_with_or)) {
                let shown = match t {
                    Ft::Tok(x) => x.clone(),
                    Ft::Frag(m, mk) => format!("${m}:{mk}"),
                    Ft::Close => String::new(),
                };
                return Err((format!("${n}:{k}"), format!("`${n}:{k}` is followed by `{shown}`, which is not allowed for `{k}` fragments")));
            }
        }
    }
    Ok((last, empty))
}

/// Where a fragment `$name:kind` rustc rejects what follows is (BAD_FOLLOW): `line:col: msg` at its first such
/// fragment in src
#[cold]
pub fn follow_err(src: &str, frag: &str, msg: &str) -> Option<String> {
    let (n, k) = frag.strip_prefix('$')?.split_once(':')?;
    let b = src.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let tx = |q: usize| &b[toks[q].1..toks[q].2];
    let q = (0..toks.len().saturating_sub(3)).find(|&q| tx(q) == b"$" && tx(q + 1) == n.as_bytes() && tx(q + 2) == b":" && tx(q + 3) == k.as_bytes())?;
    let off = toks[q].1;
    let line = 1 + b[..off].iter().filter(|&&c| c == b'\n').count();
    let col = src[..off].chars().rev().take_while(|&c| c != '\n').count() + 1;
    Some(format!("{line}:{col}: {msg}"))
}

/// Whether a matcher is one rustc rejects as it reads it: a name bound twice (seen: the names bound before), a
/// `$name` with no fragment specifier, a repetition with no separator that can match nothing (`$()*`, `$($v:vis)*`)
fn bad_matcher<'a>(pat: &'a [Pt], seen: &mut Vec<&'a str>) -> bool {
    pat.iter().any(|p| match p {
        Pt::Var(n, k) if k.is_empty() => n != "crate",
        Pt::Var(n, _) => seen.contains(&n.as_str()) || {
            seen.push(n);
            false
        },
        Pt::Rep(inner, None, _) if inner.iter().all(|p| matches!(p, Pt::Var(_, k) if k == "vis") || matches!(p, Pt::Rep(_, _, '*' | '?'))) => true,
        Pt::Group(_, inner) | Pt::Rep(inner, ..) => bad_matcher(inner, seen),
        _ => false,
    })
}

/// 規則の並び `(matcher) => {transcriber}; ..` (macro_rules! の `{ .. }` の中身のテキスト)
/// Whether a rule's body has a `$` right before a `[..]` group, in its groups too
fn dollar_bracket(ts: TokenStream) -> bool {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    toks.iter().enumerate().any(|(k, t)| match t {
        TokenTree::Punct(p) => p.as_char() == '$' && matches!(toks.get(k + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket),
        TokenTree::Group(g) => dollar_bracket(g.stream()),
        _ => false,
    })
}

fn parse_rules(inner: &str) -> Result<Vec<Rule>, String> {
    let ts = TokenStream::from_str(inner).map_err(|e| format!("1:1: {e}"))?;
    let body: Vec<TokenTree> = ts.into_iter().collect();
    // (no rule, or one rustc rejects: noted, for the driver to read where it is)
    if body.is_empty() {
        BAD_DEF.with(|c| c.set(true));
    }
    let mut rules = vec![];
    let mut j = 0;
    while j < body.len() {
        // (a rule's doc comment or attribute, `/// doc` before its matcher: rustc's unused_doc_comments warns of it)
        if matches!(&body[j], TokenTree::Punct(p) if p.as_char() == '#') && matches!(body.get(j + 1), Some(TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Bracket) {
            j += 2;
            continue;
        }
        let TokenTree::Group(m) = &body[j] else {
            BAD_DEF.with(|c| c.set(true));
            j += 1;
            continue;
        };
        let Some(TokenTree::Group(t)) = body.get(j + 3) else {
            BAD_DEF.with(|c| c.set(true));
            break;
        };
        let pat = parse_pt(m.stream(), true);
        // (`($a:ident, $a:ident)`: a name bound twice, rustc's "duplicate matcher binding"; `($i)`, `$()*`)
        if bad_matcher(&pat, &mut vec![]) {
            BAD_DEF.with(|c| c.set(true));
        }
        // (`($e:expr +)`: a token after a fragment that rustc does not allow there)
        if let Err(e) = check_follow(&pat, &[], PAT_WITH_OR.with(|c| c.get())) {
            bad_follow(e);
        }
        // (`$[..]` in the body: macro_def_err places rustc's error)
        if dollar_bracket(t.stream()) {
            BAD_DEF.with(|c| c.set(true));
        }
        rules.push(Rule { pat, body: parse_pt(t.stream(), false) });
        j += 4;
        if let Some(TokenTree::Punct(p)) = body.get(j) {
            if p.as_char() == ';' {
                j += 1;
            }
        }
    }
    Ok(rules)
}

/// lexer::tokenize のトークン列で、開き括弧ごとの閉じ括弧の番号 (開きでなければ usize::MAX) と、トークンごとの括弧の深さ
/// (for pm: each bracket's closer, and each token's depth)
pub(crate) fn brackets_of(s: &[u8], toks: &[(u8, usize, usize)]) -> (Vec<usize>, Vec<u32>) {
    brackets(s, toks)
}

fn brackets(s: &[u8], toks: &[(u8, usize, usize)]) -> (Vec<usize>, Vec<u32>) {
    let mut close = vec![usize::MAX; toks.len()];
    let mut depth = vec![0u32; toks.len()];
    let mut st: Vec<usize> = vec![];
    for (k, t) in toks.iter().enumerate() {
        depth[k] = st.len() as u32;
        if t.0 != 1 {
            continue;
        }
        match s[t.1] {
            b'(' | b'[' | b'{' => st.push(k),
            b')' | b']' | b'}' => {
                if let Some(o) = st.pop() {
                    close[o] = k;
                }
                depth[k] = st.len() as u32;
            }
            _ => {}
        }
    }
    (close, depth)
}

/// トークン i を直接包む括弧が `mod m {` / `impl .. {` / `trait .. {` の中括弧か (アイテムが並ぶ所)
fn item_scope(b: &[u8], toks: &[(u8, usize, usize)], close: &[usize], depth: &[u32], i: usize) -> bool {
    let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
    let mut o = i;
    loop {
        if o == 0 {
            return false;
        }
        o -= 1;
        if close[o] != usize::MAX && close[o] > i && depth[o] + 1 == depth[i] {
            break;
        }
    }
    if tk(o) != b"{" {
        return false;
    }
    let mut k = o;
    let mut n = 0;
    // (inside `<..>` of the head, `=` binds an associated type: `where I: Iterator<Item = T>`)
    let mut angle = 0i32;
    while k > 0 && n < 256 {
        k -= 1;
        n += 1;
        let t = tk(k);
        // (a braced const argument inside the head's `<..>`, `impl C<{ m!() * 3 }> {`: stepped over, back to its `{`)
        if toks[k].0 == 1 && t == b"}" && angle > 0 {
            let mut d = 0i32;
            while k > 0 {
                if toks[k].0 == 1 && tk(k) == b"}" {
                    d += 1;
                } else if toks[k].0 == 1 && tk(k) == b"{" {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                k -= 1;
            }
            continue;
        }
        if toks[k].0 == 1 && (t == b";" || t == b"{" || t == b"}") {
            return false;
        }
        // (`extern "C" { .. }`: foreign items. `extern "C" fn f() {` meets `fn` first, going back)
        // (an `impl` that starts an item: not `impl Trait` in a type, `fn f() -> impl Display { m!() * 3 }`)
        let impl_head = || k == 0 || (toks[k - 1].0 == 1 && matches!(tk(k - 1), b";" | b"{" | b"}" | b"]")) || (toks[k - 1].0 == 0 && matches!(tk(k - 1), b"unsafe" | b"default"));
        // (a brace inside the head's `<..>` is a const argument, an expression: `impl S<{ m!() * 3 }> {}`)
        if toks[k].0 == 0 && (t == b"mod" || (t == b"impl" && impl_head()) || t == b"trait" || t == b"extern") {
            return angle >= 0;
        }
        if toks[k].0 == 0 && (t == b"fn" || t == b"struct" || t == b"enum" || t == b"union" || t == b"let") {
            return false;
        }
        // (the `>` of `->` / `=>` is none)
        let arrow = k > 0 && toks[k - 1].0 == 1 && matches!(tk(k - 1), b"-" | b"=") && toks[k - 1].2 == toks[k].1;
        if toks[k].0 == 1 && t == b">" && !arrow {
            angle += 1;
        } else if toks[k].0 == 1 && t == b"<" {
            angle -= 1;
        } else if toks[k].0 == 1 && t == b"=" && angle <= 0 {
            return false;
        }
    }
    false
}

/// ソースの中の macro_rules! を展開する。なければ None。
/// ソース全体は速い lexer::tokenize で見て、proc_macro2 の字句解析は定義と呼び出しの括弧の中だけにする
/// (全体を proc_macro2 で読むと、大きなソース (zustc 自身) で遅い)。結果は expand_reference と同じ
/// The line that separates a crate's own text from the `macro_rules!` of its dependencies appended for its
/// expansion (session's workspace build)
pub const DEP_MACROS_MARKER: &str = "\n// __zustc_dep_macros__\n";

/// How deep macro expansion may go: the crate's `#![recursion_limit = "N"]`, else rustc's default, 128
pub fn recursion_limit(src: &str) -> usize {
    if !src.contains("recursion_limit") {
        return 128;
    }
    // (by tokens, so a comment or a string that spells the attribute changes nothing)
    let b = src.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
    for k in 0..toks.len().saturating_sub(6) {
        if tk(k) == b"#" && tk(k + 1) == b"!" && tk(k + 2) == b"[" && tk(k + 3) == b"recursion_limit" && tk(k + 4) == b"=" && toks[k + 5].0 == 2 {
            let lit = tk(k + 5);
            if let Some(n) = std::str::from_utf8(lit).ok().map(|l| l.trim_matches('"')).and_then(|l| l.trim().parse::<usize>().ok()) {
                return n;
            }
        }
    }
    128
}

pub fn expand(src: &str) -> Result<Option<String>, String> {
    expand_cfg(src, None)
}

/// ソースの中の macro_rules! の定義のテキスト (`macro_rules! m { .. }` を改行で並べたもの)。
/// ワークスペースで、依存するクレートのマクロを使う側の展開に渡す
pub fn macro_defs(src: &str) -> String {
    let mut out = String::new();
    if !src.contains("macro_rules!") {
        return out;
    }
    let b = src.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let (close, _) = brackets(b, &toks);
    let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
    let sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && b[toks[k].1] == c;
    let mut i = 0;
    while i + 3 < toks.len() {
        if toks[i].0 == 0 && tk(i) == b"macro_rules" && sym(i + 1, b'!') && toks[i + 2].0 == 0 && close[i + 3] != usize::MAX {
            let ge = close[i + 3];
            out.push_str(&src[toks[i].1..toks[ge].2]);
            out.push('\n');
            i = ge + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// expand と同じだが、展開した後のソースの `#[cfg(..)]` を周ごとに評価する (`#[cfg(p)] m! { .. }` を展開する前に)
/// The top-level items of a macro's expansion (`struct S;`, `fn f() {..}`, `impl ..`, `macro_rules! m {..}` ..) and the
/// other statements, as two texts; None when there is no item
fn split_items(body: &str) -> Option<(String, String)> {
    let b = body.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let n = toks.len();
    let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && b[toks[k].1] == c;
    const KW: &[&[u8]] = &[b"struct", b"enum", b"fn", b"impl", b"trait", b"mod", b"use", b"union", b"type", b"static", b"extern", b"macro_rules"];
    let (mut items, mut rest) = (String::new(), String::new());
    let mut any = false;
    let mut i = 0;
    while i < n {
        // (attributes, `pub` / `pub(crate)`, `unsafe` / `async` / `const` before the keyword)
        let st = i;
        let mut k = i;
        while sym(k, b'#') && sym(k + 1, b'[') {
            let mut d = 0;
            k += 1;
            while k < n {
                if sym(k, b'[') {
                    d += 1;
                } else if sym(k, b']') {
                    d -= 1;
                    if d == 0 {
                        k += 1;
                        break;
                    }
                }
                k += 1;
            }
        }
        if k < n && tk(k) == b"pub" {
            k += 1;
            if sym(k, b'(') {
                while k < n && !sym(k, b')') {
                    k += 1;
                }
                k += 1;
            }
        }
        while k < n && matches!(tk(k), b"unsafe" | b"async") {
            k += 1;
        }
        let const_fn = k + 1 < n && tk(k) == b"const" && tk(k + 1) == b"fn";
        if const_fn {
            k += 1;
        }
        let is_item = k < n && toks[k].0 == 0 && KW.contains(&tk(k));
        // (`static` / `type` / `use` end at the `;`; the others at it or at their first `{ .. }`)
        let semi_only = k < n && matches!(tk(k), b"static" | b"type" | b"use");
        // the statement's end
        let mut j = k.max(st);
        let mut d = 0i32;
        let mut end = n;
        while j < n {
            if sym(j, b'(') || sym(j, b'[') || sym(j, b'{') {
                d += 1;
            } else if sym(j, b')') || sym(j, b']') || sym(j, b'}') {
                d -= 1;
                if d == 0 && sym(j, b'}') && is_item && !semi_only {
                    end = if sym(j + 1, b';') { j + 2 } else { j + 1 };
                    break;
                }
            } else if d == 0 && sym(j, b';') {
                end = j + 1;
                break;
            }
            j += 1;
        }
        let a = toks[st].1;
        let e = if end < n { toks[end].1 } else { body.len() };
        if is_item {
            any = true;
            items.push_str(&body[a..e]);
            items.push(' ');
        } else {
            rest.push_str(&body[a..e]);
            rest.push(' ');
        }
        i = end;
    }
    any.then_some((items, rest))
}

/// 1. `macro_rules!` の定義を集めて消す (関数の本体や `mod m { .. }` の中の定義も)
/// `first` gets, for each name, the offset of its first definition (usize::MAX when one is `#[macro_export]`: a path
/// to it works anywhere in the crate)
fn collect_defs(text: &mut String, macros: &mut HashMap<String, Vec<Rule>>, mut first: Option<&mut HashMap<String, usize>>) -> Result<(), String> {
        let b = text.as_bytes();
        let toks = crate::lexer::tokenize(b);
        let (close, _) = brackets(b, &toks);
        let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
        let sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && b[toks[k].1] == c;
        let mut cut: Vec<(usize, usize)> = vec![];
        // (the `#[macro_export]` ones defined here: two of one name are E0428, as both are at the crate's root)
        let mut exported_here: Vec<String> = vec![];
        let mut i = 0;
        while i + 3 < toks.len() {
            let is_def = toks[i].0 == 0 && tk(i) == b"macro_rules" && sym(i + 1, b'!') && toks[i + 2].0 == 0 && close[i + 3] != usize::MAX;
            // (a macro call's arguments are its input, not items: `stringify!(macro_rules! m { .. })` is text, and quote's
            //  `__quote![ macro_rules! quote { .. } ]` defines `quote!` only when the call expands, in a later round)
            //  A std macro's arguments are not expanded by macro_rules, so a definition there is collected now:
            //  `println!("{}", { macro_rules! foo { .. } foo!(1) })`)
            if !is_def && toks[i].0 == 0 && tk(i) != b"macro_rules" && sym(i + 1, b'!') && close[i + 2] != usize::MAX
                && (sym(i + 2, b'(') || sym(i + 2, b'[') || sym(i + 2, b'{'))
                && (matches!(tk(i), b"stringify" | b"concat_idents" | b"log_syntax")
                    || !crate::lexer::is_std_macro(std::str::from_utf8(tk(i)).unwrap_or("")))
            {
                i = close[i + 2] + 1;
                continue;
            }
            if !is_def {
                i += 1;
                continue;
            }
            let ge = close[i + 3];
            let name = String::from_utf8_lossy(tk(i + 2)).into_owned();
            let rules = parse_rules(&text[toks[i + 3].2..toks[ge].1])?;
            macros.insert(name.clone(), rules);
            // `#[macro_export]` などの属性も消す (all of them: `#[macro_export] #[doc(hidden)] macro_rules! m`)
            let mut st = toks[i].1;
            let mut at = i;
            while at >= 3 && sym(at - 1, b']') {
                let mut o = at - 1;
                while o > 0 && close[o - 1] != at - 1 {
                    o -= 1;
                }
                if o >= 2 && sym(o - 1, b'[') && sym(o - 2, b'#') {
                    st = toks[o - 2].1;
                    at = o - 2;
                } else {
                    break;
                }
            }
            if st != toks[i].1 && text[st..toks[i].1].contains("macro_export") {
                if exported_here.contains(&name) {
                    dup_export(&name);
                }
                EXPORTED.with(|c| c.borrow_mut().push(name.clone()));
                exported_here.push(name.clone());
            }
            if let Some(f) = first.as_deref_mut() {
                let exported = st != toks[i].1 && text[st..toks[i].1].contains("macro_export");
                let at = if exported { usize::MAX } else { toks[i].1 };
                f.entry(name.clone()).and_modify(|o| *o = (*o).min(at)).or_insert(at);
            }
            let mut en = toks[ge].2;
            if sym(ge + 1, b';') {
                en = toks[ge + 1].2;
            }
            cut.push((st, en));
            i = ge + 1;
        }
        let mut bb = text.clone().into_bytes();
        for (a, e) in cut {
            for x in &mut bb[a..e] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
        }
        *text = String::from_utf8(bb).map_err(|e| e.to_string())?;
        Ok(())
}

/// Two `macro_rules!` of one name (serde_core's `seq_impl` in de/impls.rs and in ser/impls.rs): textual scope, a call
/// is the definition last above it. Each definition gets a name of its own (`seq_impl__zm1`) and each call the name of
/// the definition before it. Not for a `#[macro_export]` one, which a path names anywhere
fn rename_shadowed_macros(text: &str) -> Option<String> {
    let b = text.as_bytes();
    // (first a look at the bytes: most sources define no name twice, or twice the same (a generated module's copy),
    // and need no tokens for it. A definition's text: to the bracket that closes its body, brackets counted)
    {
        let def_text = |k: usize| -> &[u8] {
            let mut d = 0i32;
            let mut j = k;
            while j < b.len() {
                match b[j] {
                    b'{' | b'(' | b'[' => d += 1,
                    b'}' | b')' | b']' => {
                        d -= 1;
                        if d == 0 {
                            return &b[k..=j];
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            &b[k..]
        };
        let mut names: Vec<(&[u8], &[u8])> = vec![];
        let mut twice = false;
        for (k, _) in text.match_indices("macro_rules!") {
            let mut a = k + "macro_rules!".len();
            while a < b.len() && b[a].is_ascii_whitespace() {
                a += 1;
            }
            let mut e = a;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            let body = def_text(e);
            match names.iter().find(|(n, _)| *n == &b[a..e]) {
                Some((_, t)) if *t == body => {}
                Some(_) => {
                    twice = true;
                    break;
                }
                None => names.push((&b[a..e], body)),
            }
        }
        if !twice {
            return None;
        }
    }
    let toks = crate::lexer::tokenize(b);
    let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
    let sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && b[toks[k].1] == c;
    // (the input of `stringify!` / `concat_idents!` / `log_syntax!` is text, not code: no definition or call there,
    // as collect_defs reads it)
    let mut opaque = vec![false; toks.len()];
    for i in 0..toks.len().saturating_sub(2) {
        if toks[i].0 == 0 && matches!(tk(i), b"stringify" | b"concat_idents" | b"log_syntax") && sym(i + 1, b'!') && (sym(i + 2, b'(') || sym(i + 2, b'{') || sym(i + 2, b'[')) {
            let mut d = 0i32;
            let mut j = i + 2;
            while j < toks.len() {
                if sym(j, b'{') || sym(j, b'(') || sym(j, b'[') {
                    d += 1;
                } else if sym(j, b'}') || sym(j, b')') || sym(j, b']') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                opaque[j] = true;
                j += 1;
            }
        }
    }
    // (name -> the tokens of its definitions' names, in order; exported: any of them is `#[macro_export]`)
    let mut defs: HashMap<&[u8], (Vec<usize>, bool)> = HashMap::default();
    for i in 0..toks.len().saturating_sub(3) {
        if toks[i].0 == 0 && !opaque[i] && tk(i) == b"macro_rules" && sym(i + 1, b'!') && toks[i + 2].0 == 0 {
            let exported = text[..toks[i].1].trim_end().ends_with(']') && {
                let before = &text[..toks[i].1];
                before.rfind("#[").is_some_and(|a| before[a..].contains("macro_export"))
            };
            let e = defs.entry(tk(i + 2)).or_default();
            e.0.push(i + 2);
            e.1 |= exported;
        }
    }
    // (a name a `use` imports (`pub(crate) use m;`, a path-based binding) keeps its definitions' names: the
    // import names them)
    let mut used: Vec<&[u8]> = vec![];
    let mut i = 0;
    while i < toks.len() {
        if toks[i].0 == 0 && tk(i) == b"use" && !opaque[i] {
            let mut j = i + 1;
            while j < toks.len() && !sym(j, b';') {
                if toks[j].0 == 0 {
                    used.push(tk(j));
                }
                j += 1;
            }
            i = j;
        }
        i += 1;
    }
    let mut dup: HashMap<&[u8], &Vec<usize>> =
        defs.iter().filter(|(k, (v, ex))| v.len() > 1 && !ex && !used.contains(k)).map(|(k, (v, _))| (*k, v)).collect();
    if dup.is_empty() {
        return None;
    }
    // (the token ranges of the `macro_rules!` bodies: a call in one is resolved where that macro is called, not known
    // here; the definition nearest it is taken (`tuple_impls!`'s `tuple_impl_body!` beside the one of its module))
    let mut in_body = vec![false; toks.len()];
    for i in 0..toks.len().saturating_sub(3) {
        if toks[i].0 == 0 && tk(i) == b"macro_rules" && sym(i + 1, b'!') && toks[i + 2].0 == 0 && (sym(i + 3, b'{') || sym(i + 3, b'(')) {
            let mut d = 0i32;
            let mut j = i + 3;
            while j < toks.len() {
                if sym(j, b'{') || sym(j, b'(') || sym(j, b'[') {
                    d += 1;
                } else if sym(j, b'}') || sym(j, b')') || sym(j, b']') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                j += 1;
            }
            for x in &mut in_body[i + 4..j.min(toks.len())] {
                *x = true;
            }
        }
    }
    // (a definition's textual scope: to the end of the block or module around it, and past a `#[macro_use] mod`'s end
    // to its parent's. Outside every definition's scope a call keeps its name: "cannot find macro" as before)
    let mut open: Vec<usize> = vec![];
    let mut encl = vec![usize::MAX; toks.len()];
    let mut close_of = vec![usize::MAX; toks.len()];
    for k in 0..toks.len() {
        encl[k] = open.last().copied().unwrap_or(usize::MAX);
        if sym(k, b'{') {
            open.push(k);
        } else if sym(k, b'}') {
            if let Some(o) = open.pop() {
                close_of[o] = k;
            }
        }
    }
    let scope_end = |d: usize| -> usize {
        let mut o = encl[d];
        loop {
            if o == usize::MAX {
                return toks.len();
            }
            // (`#[macro_use] mod m {`: the scope goes on in the parent)
            let is_mod = o >= 2 && toks[o - 1].0 == 0 && toks[o - 2].0 == 0 && tk(o - 2) == b"mod";
            if is_mod && text[..toks[o - 2].1].trim_end().ends_with(']') && {
                let before = &text[..toks[o - 2].1];
                before.rfind("#[").is_some_and(|a| before[a..].contains("macro_use"))
            } {
                o = encl[o];
                continue;
            }
            return if close_of[o] == usize::MAX { toks.len() } else { close_of[o] };
        }
    };
    // (a name defined twice in one scope (`macro_rules! m` redefined further down) is one binding shadowed in place:
    // it keeps its name, as before. Renamed are the names that different modules / blocks each define)
    dup.retain(|_, ds| {
        let mut scopes: Vec<usize> = ds.iter().map(|&d| encl[d]).collect();
        scopes.sort_unstable();
        scopes.windows(2).all(|w| w[0] != w[1])
    });
    if dup.is_empty() {
        return None;
    }
    let ends: HashMap<usize, usize> = dup.values().flat_map(|v| v.iter().map(|&d| (d, scope_end(d)))).collect();
    // (a call has its brackets after the `!`: `m != 0` is a comparison; not a path's last segment `a::name!`)
    let is_call = |k: usize| sym(k + 1, b'!') && (sym(k + 2, b'(') || sym(k + 2, b'[') || sym(k + 2, b'{')) && !(k >= 1 && sym(k - 1, b':'));
    // (the block / module a definition is in contains k, up to the definition's scope's end)
    let in_region = |d: usize, k: usize| (encl[d] == usize::MAX || encl[d] < k) && k < ends[&d];
    // (a call in a macro body that no definition's block contains (`invoke!` at the top calling the `pick!` that
    // `mod a` and `mod b` each define) is resolved where `invoke!` is called, which this pass does not know: the
    // name keeps its definitions' names, as before)
    let mut unknown: Vec<&[u8]> = vec![];
    for k in 0..toks.len() {
        if in_body[k] && toks[k].0 == 0 && !opaque[k] && is_call(k) {
            if let Some(ds) = dup.get(tk(k)) {
                if !ds.iter().any(|&d| in_region(d, k)) && !unknown.contains(&tk(k)) {
                    unknown.push(tk(k));
                }
            }
        }
    }
    dup.retain(|n, _| !unknown.contains(n));
    if dup.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut last = 0;
    for k in 0..toks.len() {
        if toks[k].0 != 0 || opaque[k] {
            continue;
        }
        let Some(ds) = dup.get(tk(k)) else { continue };
        // (a definition's name, or a call `name!`, not a path's last segment `a::name!`)
        let is_def = ds.contains(&k);
        let is_call = is_call(k);
        if !is_def && !is_call {
            continue;
        }
        let n = if is_def {
            ds.iter().position(|&d| d == k)
        } else if in_body[k] {
            (0..ds.len()).filter(|&n| in_region(ds[n], k)).min_by_key(|&n| ds[n].abs_diff(k))
        } else {
            ds.iter().rposition(|&d| d <= k && k < ends[&d])
        };
        let Some(n) = n else { continue };
        out.push_str(&text[last..toks[k].1]);
        out.push_str(&format!("{}__zm{}", String::from_utf8_lossy(tk(k)), n));
        last = toks[k].2;
    }
    out.push_str(&text[last..]);
    Some(out)
}

pub fn expand_cfg(src: &str, cfg: Option<&crate::cfg::CfgSet>) -> Result<Option<String>, String> {
    expand_with(src, cfg, &[])
}

/// A name that can be a segment of a path (`crate`, `self`, `super`, `Self` and identifiers, no other keyword): a
/// macro call's path stops at `return ::pm::m!()`'s `return`
fn path_seg(t: &[u8]) -> bool {
    !matches!(
        t,
        b"return" | b"break" | b"continue" | b"in" | b"as" | b"let" | b"mut" | b"ref" | b"move" | b"if" | b"else" | b"match" | b"while" | b"for"
            | b"loop" | b"fn" | b"pub" | b"unsafe" | b"async" | b"await" | b"dyn" | b"impl" | b"where" | b"use" | b"const" | b"static" | b"struct"
            | b"enum" | b"type" | b"mod" | b"trait" | b"box" | b"yield" | b"extern"
    )
}

/// The outermost attribute and derive uses of the text, expanded (crate::pm::expand_items) and put in its place with
/// as many lines as they had; the definitions' places (textual scope) move with the text. Whether one ran
fn pm_items(text: &mut String, pms: &[crate::pm::Lib], env: &[(String, String)], cfg: Option<&crate::cfg::CfgSet>, first: &mut HashMap<String, usize>) -> Result<bool, String> {
    let preps = crate::pm::expand_items(text, pms, env)?;
    if preps.is_empty() {
        return Ok(false);
    }
    for (a, e, rep) in preps.into_iter().rev() {
        let nl = text[a..e].bytes().filter(|&c| c == b'\n').count();
        let mut r = rep;
        for _ in 0..nl {
            r.push('\n');
        }
        let (old_len, new_len) = (e - a, r.len());
        for v in first.values_mut() {
            if *v != usize::MAX && *v >= e {
                *v = *v + new_len - old_len;
            }
        }
        text.replace_range(a..e, &r);
    }
    // (their cfgs now: a macro call in an item they give under a cfg that is off does not run. The strip keeps the
    // text's length and lines)
    if let Some(t) = cfg.and_then(|c| crate::cfg::strip(text, c)) {
        *text = t;
    }
    Ok(true)
}

/// expand_cfg with the proc macros of pms too (crate::pm): at the start of each round the outermost attribute and
/// derive uses, and a function-like call where a `macro_rules!` call would be (a name no `macro_rules!` has)
pub fn expand_with(src: &str, cfg: Option<&crate::cfg::CfgSet>, pms: &[crate::pm::Lib]) -> Result<Option<String>, String> {
    // (one scan, as before, for a source with no macro at all)
    if pms.is_empty() && !src.contains("macro_rules") {
        return Ok(None);
    }
    match join_macro_rules_bang(src) {
        Some(t) => Ok(Some(expand_joined(&t, cfg, pms)?.unwrap_or(t))),
        None => expand_joined(src, cfg, pms),
    }
}

/// `macro_rules !` with whitespace before the `!`, which rustc accepts, written `macro_rules!` with that whitespace
/// after the `!`: the text keeps its length and lines, and every position past the `!`. None when there is none
pub fn join_macro_rules_bang(src: &str) -> Option<String> {
    let b = src.as_bytes();
    // (the text's candidates first; the tokens, only when there is one, keep a string's or a comment's text as is)
    let mut cands: Vec<(usize, usize)> = vec![];
    for (k, _) in src.match_indices("macro_rules") {
        let a = k + "macro_rules".len();
        let mut e = a;
        while e < b.len() && b[e].is_ascii_whitespace() {
            e += 1;
        }
        if e > a && e < b.len() && b[e] == b'!' {
            cands.push((k, e));
        }
    }
    if cands.is_empty() {
        return None;
    }
    let toks = crate::lexer::tokenize(b);
    let mut out: Option<Vec<u8>> = None;
    for w in toks.windows(2) {
        let ((k0, a, _), (k1, e, _)) = (w[0], w[1]);
        if k0 == 0 && k1 == 1 && b[e] == b'!' && &b[a..e.min(a + 11)] == b"macro_rules" && cands.iter().any(|&(k, ce)| k == a && ce == e) {
            let a = a + "macro_rules".len();
            let o = out.get_or_insert_with(|| b.to_vec());
            o[a] = b'!';
            o[a + 1..=e].copy_from_slice(&b[a..e]);
        }
    }
    out.map(|o| String::from_utf8(o).unwrap_or_else(|_| src.to_string()))
}

fn expand_joined(src: &str, cfg: Option<&crate::cfg::CfgSet>, pms: &[crate::pm::Lib]) -> Result<Option<String>, String> {
    if !src.contains("macro_rules!") && pms.is_empty() {
        return Ok(None);
    }
    let mut text = rename_shadowed_macros(src).unwrap_or_else(|| src.to_string());
    let mut macros: HashMap<String, Vec<Rule>> = HashMap::default();
    let mut first: HashMap<String, usize> = HashMap::default();
    let env: &[(String, String)] = cfg.map_or(&[], |c| &c.env[..]);
    // (the outermost attribute uses before the definitions are read: an attribute macro gets its item whole, a
    // `macro_rules!` in it too. Those in its output are read with the others)
    // (each pass runs the outermost of an item's attributes: again while one runs, `#[a] #[pm::rules] fn ..`)
    if !pms.is_empty() {
        for _ in 0..64 {
            if !pm_items(&mut text, pms, env, cfg, &mut first)? {
                break;
            }
        }
    }
    collect_defs(&mut text, &mut macros, Some(&mut first))?;
    // (a dependency's macros, after the marker, are in scope from the top: `#[macro_use] extern crate x;` or a
    //  `use` brought them in, and the dependency was checked on its own)
    if let Some(mpos) = text.find(DEP_MACROS_MARKER) {
        for d in first.values_mut() {
            if *d != usize::MAX && *d > mpos {
                *d = usize::MAX;
            }
        }
    }
    // (and so are the macros of a `#[macro_use] mod m { .. }`: srcmap appends a file's modules at its end, keeping
    //  that attribute, so `#[macro_use] mod macros;` at the top of lib.rs puts libc's `cfg_if!` below its uses)
    if text.contains("macro_use") {
        let b = text.as_bytes();
        let toks = crate::lexer::tokenize(b);
        let (close, _) = brackets(b, &toks);
        let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
        let mut spans: Vec<(usize, usize)> = vec![];
        for k in 0..toks.len().saturating_sub(4) {
            if tk(k) == b"#" && tk(k + 1) == b"[" && tk(k + 2) == b"macro_use" && tk(k + 3) == b"]" {
                let mut j = k + 4;
                while j < toks.len() && matches!(tk(j), b"pub") {
                    j += 1;
                }
                if j + 2 < toks.len() && tk(j) == b"mod" && toks[j + 1].0 == 0 && tk(j + 2) == b"{" && close[j + 2] != usize::MAX {
                    spans.push((toks[j + 2].1, toks[close[j + 2]].2));
                }
            }
        }
        for d in first.values_mut() {
            if spans.iter().any(|&(a, e)| *d >= a && *d < e) {
                *d = usize::MAX;
            }
        }
    }
    if macros.is_empty() && pms.is_empty() {
        return Ok(Some(text));
    }
    // 2. 呼び出しを展開する (いちばん外側の呼び出しだけ。展開したものの中の呼び出しは次の周)
    let limit = text.len().saturating_mul(64).max(16 << 20);
    let depth_limit = recursion_limit(&text);
    // (one more round than the limit: the last one only finds that nothing is left to expand)
    for round in 0..=depth_limit {
        // (definitions an expansion produced: `macro_rules! foo { () => { macro_rules! bar { .. } } }`)
        if round > 0 && text.contains("macro_rules") {
            collect_defs(&mut text, &mut macros, None)?;
        }
        // (proc macros: the outermost attribute and derive uses. What they give is read again next round; the
        // `macro_rules!` they give are definitions now, not calls to run: `pm::fail!()` in an unused one does not run)
        let mut pm_done = !pms.is_empty() && pm_items(&mut text, pms, env, cfg, &mut first)?;
        if pm_done && text.contains("macro_rules") {
            collect_defs(&mut text, &mut macros, None)?;
        }
        let b = text.as_bytes();
        let toks = crate::lexer::tokenize(b);
        let (close, depth) = brackets(b, &toks);
        let pm_scope = (!pms.is_empty()).then(|| crate::pm::Scope::new(pms, b, &toks));
        // (the end of a `stringify!(..)`'s arguments: a proc macro called in them does not run, they are text)
        let mut inert_until = 0usize;
        let tk = |k: usize| -> &[u8] { &b[toks[k].1..toks[k].2] };
        let sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && b[toks[k].1] == c;
        let mut reps: Vec<(usize, usize, String)> = vec![];
        let mut i = 0;
        let n = toks.len();
        while i + 2 < n {
            // (most tokens are no call: on to the next `name!(..)` in one tight loop)
            match (i..n - 2).find(|&j| toks[j].0 == 0 && toks[j + 1].0 == 1 && b[toks[j + 1].1] == b'!' && close[j + 2] != usize::MAX) {
                Some(j) => i = j,
                None => break,
            }
            // (the builtin's: `stringify!`, `std::stringify!` / `core::stringify!` and their absolute `::std::` form; not
            // a library's macro of that name, `re::stringify!` / `re::std::stringify!`, which can be another macro
            // re-exported under it, nor a `macro_rules! stringify` of the crate's own)
            let colons = |k: usize| k >= 2 && sym(k - 1, b':') && sym(k - 2, b':');
            // (a label or a lifetime is no path segment: `break 'done ::std::stringify!(..)`. The tokens keep only its
            // name; the quote is the byte before it)
            let label = |k: usize| toks[k].1 > 0 && b[toks[k].1 - 1] == b'\'';
            if tk(i) == b"stringify"
                && !macros.contains_key("stringify")
                && (!colons(i)
                    || (i >= 3
                        && toks[i - 3].0 == 0
                        && matches!(tk(i - 3), b"std" | b"core")
                        && !(colons(i - 3) && i >= 6 && toks[i - 6].0 == 0 && path_seg(tk(i - 6)) && !label(i - 6))))
            {
                inert_until = inert_until.max(close[i + 2]);
            }
            let mut rules = macros.get(std::str::from_utf8(tk(i)).unwrap_or(""));
            // (in `stringify!(..)`: a `macro_rules!` call is kept as written, as rustc does: `stringify!(m!())` is
            // "m!()")
            if i < inert_until {
                rules = None;
            }
            // (a proc macro's function-like call: `name!(..)`, `path::name!(..)`, a name no `macro_rules!` has. A path
            // from a proc-macro crate's name, or a library's that re-exports its macro (`pm::answer!()`,
            // `re::answer!()`), is that crate's even beside a `macro_rules! answer`)
            let pm_bang = match &pm_scope {
                Some(sc) if tk(i) != b"macro_rules" => {
                    let mut segs = vec![std::str::from_utf8(tk(i)).unwrap_or("")];
                    let mut k = i;
                    while k >= 3 && toks[k - 3].0 == 0 && path_seg(tk(k - 3)) && sym(k - 2, b':') && sym(k - 1, b':') {
                        segs.insert(0, std::str::from_utf8(tk(k - 3)).unwrap_or(""));
                        k -= 3;
                    }
                    let own = sc.names_crate(&segs);
                    if i < inert_until {
                        // (in `stringify!(..)`: kept as written, also beside a `macro_rules!` of its name)
                        if own {
                            rules = None;
                        }
                        None
                    } else if rules.is_none() || own {
                        sc.resolve(&segs, crate::pm::Kind::Bang).inspect(|_| rules = None)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if rules.is_none() && pm_bang.is_none() {
                i += 1;
                continue;
            }
            // (textual scope: a `macro_rules!` macro is usable only after its definition; E0433-like
            // "cannot find macro" for `m!()` above the only `macro_rules! m` (not `crate::m!` / exported ones))
            if rules.is_some() && round == 0 && !(i >= 2 && sym(i - 1, b':')) {
                let nm = std::str::from_utf8(tk(i)).unwrap_or("");
                // (`use m;` after the definition makes it a path-scoped item, usable above too)
                let later = first.get(nm).is_some_and(|&d| d != usize::MAX && toks[i].1 < d)
                    && !(0..toks.len().saturating_sub(1)).any(|k| toks[k].0 == 0 && tk(k) == b"use" && {
                        let mut j = k + 1;
                        while j < toks.len() && !sym(j, b';') && !(toks[j].0 == 0 && tk(j) == nm.as_bytes()) {
                            j += 1;
                        }
                        j < toks.len() && !sym(j, b';')
                    });
                // (`pub m!(..);` in an item list)
                let vis = i >= 1 && toks[i - 1].0 == 0 && tk(i - 1) == b"pub";
                if later || vis {
                    let at = toks[if vis { i - 1 } else { i }].1;
                    let line = b[..at].iter().filter(|&&c| c == b'\n').count() + 1;
                    let ls = b[..at].iter().rposition(|&c| c == b'\n').map(|k| k + 1).unwrap_or(0);
                    let col = text[ls..at].chars().count();
                    if vis {
                        return Err(format!("{line}:{}: can't qualify macro invocation with `pub`", col + 1));
                    }
                    return Err(format!("{line}:{}: cannot find macro `{nm}` in this scope", col + 1));
                }
            }
            let ge = close[i + 2];
            // (`line:col` of the call, for an error: counted only then, as it reads the text up to the call)
            let at = toks[i].1;
            let pos = || {
                let line = b[..at].iter().filter(|&&c| c == b'\n').count() + 1;
                let ls = b[..at].iter().rposition(|&c| c == b'\n').map(|k| k + 1).unwrap_or(0);
                (line, text[ls..at].chars().count())
            };
            let body: Option<String> = match rules {
                Some(rules) => {
                    let ts = TokenStream::from_str(&text[toks[i + 2].2..toks[ge].1]).map_err(|e| format!("1:1: {e}"))?;
                    let args: Vec<TokenTree> = ts.into_iter().collect();
                    let mut found = None;
                    for r in rules {
                        let mut binds = HashMap::default();
                        if match_pat(&r.pat, &args, &mut binds) {
                            let mut body = String::new();
                            transcribe_h(&r.body, &binds, &mut body)?;
                            found = Some(body);
                            break;
                        }
                    }
                    found
                }
                None => {
                    let (lib, name) = pm_bang.expect("a proc macro's call");
                    pm_done = true;
                    Some(crate::pm::expand(lib, &name, None, &text[toks[i + 2].2..toks[ge].1], env).map_err(|e| {
                        let (line, col) = pos();
                        format!("{line}:{}: {e}", col + 1)
                    })?)
                }
            };
            let Some(body) = body else {
                // (rustc's "no rules expected `!`", at the furthest token a rule's leading literal tokens reach: the
                // `//!` a `(#[$m:meta])` rule cannot take)
                if let Some(e) = no_rules_expected(&text, toks[i + 2].2, toks[ge].1, macros.get(std::str::from_utf8(tk(i)).unwrap_or(""))) {
                    return Err(e);
                }
                let (line, col) = pos();
                return Err(format!("{line}:{}: no rules of `{}!` matched this call", col + 1, String::from_utf8_lossy(tk(i))));
            };
            // 式の位置は `({ .. })`、トップレベルと `mod` / `impl` / `trait` の中はアイテムとしてそのまま
            // (`const C: T = m!();` at the top or in an impl is an expression)
            // (`crate::m!(..);` / `::m!(..);` / `pm::bind!();`: where its path starts (not at a keyword before a
            // leading `::`, `return ::pm::m!()`))
            let mut ps = i;
            while ps >= 3 && toks[ps - 3].0 == 0 && path_seg(tk(ps - 3)) && sym(ps - 2, b':') && sym(ps - 1, b':') {
                ps -= 3;
            }
            if ps >= 2 && sym(ps - 2, b':') && sym(ps - 1, b':') && !(ps >= 3 && toks[ps - 3].0 == 0 && path_seg(tk(ps - 3))) {
                ps -= 2;
            }
            // (`const N: i32 = pm::sum!() * 3;`: after `=` from the path's start)
            let top = (depth[i] == 0 || item_scope(b, &toks, &close, &depth, i)) && !(ps > 0 && sym(ps - 1, b'='));
            // (a call in statement position: the items it expands to are items of the enclosing block, not
            // of the `({ .. })` around the rest: `fn main() { m!(); S }` with `struct S;` from m)
            let stmt_pos = ps == 0 || sym(ps - 1, b';') || sym(ps - 1, b'{') || sym(ps - 1, b'}');
            // (at the top, a call is an item only where an item starts: after an attribute too. Elsewhere it is
            // in an item's expression or type, `const N: u32 = 0 + m!(u8);`, and keeps the `;` after it; after
            // a binary operator its expansion is one operand, as rustc's is)
            let item_start = stmt_pos || sym(ps - 1, b']');
            let operand = top && !item_start && matches!(tk(ps - 1), b"+" | b"-" | b"*" | b"/" | b"%" | b"^" | b"|" | b"&" | b"!");
            // (not with attributes: a `#[cfg(false)]` item removed later would leave the attributes before it)
            // (`m! { .. }` in statement position is a statement by itself, whatever follows: `defer! { .. }
            // let x = ..;`)
            //  Attributes that are only `#[cfg(..)]` / `#[cfg_attr(..)]` (cfg-if's expansions) are evaluated with what they
            //  are on, so they leave nothing behind)
            let spliced = !top && stmt_pos && (sym(ge + 1, b';') || sym(ge + 1, b'}') || sym(ge, b'}')) && only_cfg_attrs(&body);
            // (the call's own `;` goes when the expansion ends a statement itself: no stray `;` after attributes)
            let drop_semi = spliced && sym(ge + 1, b';') && body.trim_end().ends_with(';');
            let rep = if operand {
                format!("({body})")
            } else if top {
                body
            } else if spliced {
                // (followed by `;` or the block's end: the statements go into the block as they are; the
                // expansion's own `let`s are already renamed for hygiene, so they do not capture names)
                body
            } else if stmt_pos {
                match split_items(&body) {
                    Some((items, rest)) => format!("{items} ({{ {rest} }})"),
                    None => format!("({{ {body} }})"),
                }
            } else {
                format!("({{ {body} }})")
            };
            // (`crate::m!(..)` / `self::m!(..)`: 前のパスも消す)
            let mut a = toks[i].1;
            let mut k = i;
            while k >= 3 && toks[k - 3].0 == 0 && path_seg(tk(k - 3)) && sym(k - 2, b':') && sym(k - 1, b':') {
                a = toks[k - 3].1;
                k -= 3;
            }
            // (`::m!(..)`)
            if k >= 2 && sym(k - 2, b':') && sym(k - 1, b':') && toks[k - 1].1 == toks[k - 2].1 + 1 && !(k >= 3 && toks[k - 3].0 == 0 && path_seg(tk(k - 3))) {
                a = toks[k - 2].1;
            }
            // (トップレベルの `name!(..);` の `;` も取る)
            let mut e = toks[ge].2;
            if ((top && item_start) || drop_semi) && sym(ge + 1, b';') {
                e = toks[ge + 1].2;
            }
            reps.push((a, e, rep));
            i = ge + 1;
        }
        if reps.is_empty() && !pm_done {
            return Ok(Some(text));
        }
        // 後ろから置き換える (1 行に収めるので、置き換えた範囲の改行は残す)
        reps.sort_by_key(|r| std::cmp::Reverse(r.0));
        // (展開したものの中にマクロの名前が無ければ、もう一周しなくてよい。proc macro の出力は次の周でも読む)
        // (also when it names a proc macro: a `macro_rules!` that gives `pm::answer!()` or a `#[wasm_bindgen]` item)
        let names_pm = |t: &str| {
            pms.iter().any(|l| l.macros.iter().any(|(m, _)| t.contains(m.as_str())) || l.via.iter().any(|(_, a, _)| a != "*" && t.contains(a.as_str())))
                || pm_scope.as_ref().is_some_and(|sc| sc.aliases().any(|a| t.contains(a)))
        };
        let again = pm_done || reps.iter().any(|r| r.2.contains("macro_rules") || macros.keys().any(|m| r.2.contains(m.as_str())) || names_pm(&r.2));
        let has_cfg = cfg.is_some() && (pm_done || reps.iter().any(|r| r.2.contains("cfg")));
        let has_select = !has_cfg && reps.iter().any(|r| r.2.contains("cfg_select"));
        // (`env!($k)` in a template: its name is known only now)
        let has_env = pm_done || reps.iter().any(|r| r.2.contains("env"));
        for (a, e, rep) in reps {
            let nl = text[a..e].bytes().filter(|&c| c == b'\n').count();
            let mut r = rep;
            for _ in 0..nl {
                r.push('\n');
            }
            text.replace_range(a..e, &r);
        }
        // (a macro whose expansion grows each round (`m!($({ x } $t)*)` calling itself) would fill the memory long
        // before the round limit: stop at 64 times the source, as an error)
        if text.len() > limit {
            return Err("1:1: macro expansion is too large (a recursive macro that grows)".into());
        }
        if has_cfg {
            if let Some(t) = crate::cfg::strip(&text, cfg.unwrap()) {
                text = t;
            }
        } else if has_select {
            // (`cfg_select!` an expansion produced, without a configuration given: the default one's)
            if let Some(t) = crate::cfg::expand_select(&text, &crate::cfg::CfgSet { debug_assertions: true, ..Default::default() }) {
                text = t;
            }
        }
        if has_env {
            let none = crate::cfg::CfgSet::default();
            if let Some(t) = expand_env(&text, cfg.unwrap_or(&none)) {
                text = t;
            }
            // (`env!` values can grow it too)
            if text.len() > limit {
                return Err("1:1: macro expansion is too large (a recursive macro that grows)".into());
            }
        }
        if !again {
            return Ok(Some(text));
        }
    }
    Err(format!("1:1: macro expansion is too deep (recursion limit {depth_limit})"))
}

/// 前の版の expand (ソース全体を proc_macro2 で読む)。expand と結果が同じかを確かめるため
#[doc(hidden)]
pub fn expand_reference(src: &str) -> Result<Option<String>, String> {
    if !src.contains("macro_rules!") {
        return Ok(None);
    }
    // (macro names two textual scopes define, as expand_cfg renames them)
    let mut text = rename_shadowed_macros(src).unwrap_or_else(|| src.to_string());
    let mut macros: HashMap<String, Vec<Rule>> = HashMap::default();
    // 1. 定義を集めて消す (関数の本体や `mod m { .. }` の中の定義も)
    {
        let ts = TokenStream::from_str(&text).map_err(|e| format!("1:1: {e}"))?;
        let mut cut: Vec<(usize, usize)> = vec![];
        fn collect(ts: TokenStream, text: &str, macros: &mut HashMap<String, Vec<Rule>>, cut: &mut Vec<(usize, usize)>) {
            let toks: Vec<TokenTree> = ts.into_iter().collect();
            let mut i = 0;
            while i < toks.len() {
                let is_def = i + 3 < toks.len()
                    && matches!(&toks[i], TokenTree::Ident(x) if x == "macro_rules")
                    && matches!(&toks[i + 1], TokenTree::Punct(p) if p.as_char() == '!');
                if !is_def {
                    // (中に macro_rules の無い括弧は辿らない。`stringify!(..)` などの入力は文字: as expand_cfg's collect_defs)
                    let opaque = i >= 2
                        && matches!(&toks[i - 2], TokenTree::Ident(x) if x == "stringify" || x == "concat_idents" || x == "log_syntax")
                        && matches!(&toks[i - 1], TokenTree::Punct(p) if p.as_char() == '!');
                    if let (TokenTree::Group(g), false) = (&toks[i], opaque) {
                        let r = g.span().byte_range();
                        let inner = if r.start <= r.end && r.end <= text.len() { &text[r.start..r.end] } else { "" };
                        if inner.is_empty() || inner.contains("macro_rules") {
                            collect(g.stream(), text, macros, cut);
                        }
                    }
                    i += 1;
                    continue;
                }
                let (TokenTree::Ident(name), TokenTree::Group(g)) = (&toks[i + 2], &toks[i + 3]) else {
                    i += 1;
                    continue;
                };
                let mut rules = vec![];
                let body: Vec<TokenTree> = g.stream().into_iter().collect();
                let mut j = 0;
                while j < body.len() {
                    // (matcher) => {transcriber} ;
                    let TokenTree::Group(m) = &body[j] else { j += 1; continue };
                    let Some(TokenTree::Group(t)) = body.get(j + 3) else { break };
                    rules.push(Rule { pat: parse_pt(m.stream(), true), body: parse_pt(t.stream(), false) });
                    j += 4;
                    if let Some(TokenTree::Punct(p)) = body.get(j) {
                        if p.as_char() == ';' {
                            j += 1;
                        }
                    }
                }
                macros.insert(name.to_string(), rules);
                // `#[macro_export]` などの属性も消す
                let mut st = toks[i].span().byte_range().start;
                if i >= 2 {
                    if let (TokenTree::Punct(h), TokenTree::Group(a)) = (&toks[i - 2], &toks[i - 1]) {
                        if h.as_char() == '#' && a.delimiter() == Delimiter::Bracket {
                            st = h.span().byte_range().start;
                        }
                    }
                }
                let mut en = g.span().byte_range().end;
                if let Some(TokenTree::Punct(p)) = toks.get(i + 4) {
                    if p.as_char() == ';' {
                        en = p.span().byte_range().end;
                    }
                }
                cut.push((st, en));
                i += 4;
            }
        }
        collect(ts, &text, &mut macros, &mut cut);
        let mut b = text.into_bytes();
        for (a, e) in cut {
            for x in &mut b[a..e] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
        }
        text = String::from_utf8(b).map_err(|e| e.to_string())?;
    }
    if macros.is_empty() {
        return Ok(Some(text));
    }
    // 2. 呼び出しを展開する (展開したものの中の呼び出しも)
    let limit = text.len().saturating_mul(64).max(16 << 20);
    let depth_limit = recursion_limit(&text);
    for _round in 0..=depth_limit {
        let ts = TokenStream::from_str(&text).map_err(|e| format!("1:1: {e}"))?;
        let mut reps: Vec<(usize, usize, String)> = vec![];
        find_calls(ts, &macros, &text, 0, false, true, &mut reps)?;
        if reps.is_empty() {
            return Ok(Some(text));
        }
        // 後ろから置き換える (1 行に収めるので、置き換えた範囲の改行は残す)
        reps.sort_by_key(|r| std::cmp::Reverse(r.0));
        // (展開したものの中にマクロの名前が無ければ、もう一周しなくてよい)
        let again = reps.iter().any(|r| r.2.contains("macro_rules") || macros.keys().any(|m| r.2.contains(m.as_str())));
        for (a, e, rep) in reps {
            let nl = text[a..e].bytes().filter(|&c| c == b'\n').count();
            let mut r = rep;
            for _ in 0..nl {
                r.push('\n');
            }
            text.replace_range(a..e, &r);
        }
        // (a macro whose expansion grows each round (`m!($({ x } $t)*)` calling itself) would fill the memory long
        // before the round limit: stop at 64 times the source, as an error)
        if text.len() > limit {
            return Err("1:1: macro expansion is too large (a recursive macro that grows)".into());
        }
        if !again {
            return Ok(Some(text));
        }
    }
    Err(format!("1:1: macro expansion is too deep (recursion limit {depth_limit})"))
}

/// `name!(..)` / `name![..]` / `name!{..}` の呼び出し (いちばん外側だけ。中は次の周で展開する)
fn find_calls(
    ts: TokenStream,
    macros: &HashMap<String, Vec<Rule>>,
    text: &str,
    depth: usize,
    brace: bool,
    // (a place of items: the top level, or a `mod m { .. }` body (expand_cfg's item_scope))
    item: bool,
    out: &mut Vec<(usize, usize, String)>,
) -> Result<(), String> {
    let toks: Vec<TokenTree> = ts.into_iter().collect();
    let mut i = 0;
    while i < toks.len() {
        if i + 2 < toks.len() {
            if let (TokenTree::Ident(name), TokenTree::Punct(p), TokenTree::Group(g)) = (&toks[i], &toks[i + 1], &toks[i + 2]) {
                if p.as_char() == '!' {
                    if let Some(rules) = macros.get(&name.to_string()) {
                        let args: Vec<TokenTree> = g.stream().into_iter().collect();
                        let mut done = false;
                        for r in rules {
                            let mut binds = HashMap::default();
                            if match_pat(&r.pat, &args, &mut binds) {
                                let mut body = String::new();
                                transcribe_h(&r.body, &binds, &mut body)?;
                                // 式の位置は `({ .. })`、トップレベルはアイテムとしてそのまま
                                // (as expand_cfg: a call in statement position in a block puts its statements into the
                                // block (followed by `;` or the block's end), or its items before the `({ .. })`)
                                // (`crate::m!(..);`: where its path starts)
                                let mut ps = i;
                                while ps >= 3
                                    && matches!((&toks[ps - 3], &toks[ps - 2], &toks[ps - 1]), (TokenTree::Ident(seg), TokenTree::Punct(c1), TokenTree::Punct(c2)) if c1.as_char() == ':' && c2.as_char() == ':' && path_seg(seg.to_string().as_bytes()))
                                {
                                    ps -= 3;
                                }
                                // (`::m!(..)`, and not a keyword before it: `return ::pm::m!()`)
                                let colons = |k: usize| {
                                    k >= 2
                                        && matches!((&toks[k - 2], &toks[k - 1]), (TokenTree::Punct(c1), TokenTree::Punct(c2)) if c1.as_char() == ':' && c2.as_char() == ':' && c1.spacing() == proc_macro2::Spacing::Joint)
                                        && !(k >= 3 && matches!(&toks[k - 3], TokenTree::Ident(seg) if path_seg(seg.to_string().as_bytes())))
                                };
                                if colons(ps) {
                                    ps -= 2;
                                }
                                // (`const N: i32 = m!() * 3;`: after `=` an expression, also at the top)
                                // (a `mod m { .. }` body is a place of items too, as expand_cfg's item_scope reads it)
                                let top = (depth == 0 || item) && !(ps > 0 && matches!(&toks[ps - 1], TokenTree::Punct(q) if q.as_char() == '='));
                                let stmt_pos = brace
                                    && (ps == 0
                                        || matches!(&toks[ps - 1], TokenTree::Punct(q) if q.as_char() == ';')
                                        || matches!(&toks[ps - 1], TokenTree::Group(pg) if pg.delimiter() == proc_macro2::Delimiter::Brace));
                                let next_semi = matches!(toks.get(i + 3), Some(TokenTree::Punct(q)) if q.as_char() == ';');
                                // (`m! { .. }` is a statement by itself, whatever follows, as in expand_cfg)
                                let braced = matches!(toks.get(i + 2), Some(TokenTree::Group(cg)) if cg.delimiter() == proc_macro2::Delimiter::Brace);
                                let spliced = depth > 0 && stmt_pos && (next_semi || braced || toks.get(i + 3).is_none()) && only_cfg_attrs(&body);
                                let drop_semi = spliced && next_semi && body.trim_end().ends_with(';');
                                // (as expand_cfg: at the top, an item only where an item starts (after an attribute
                                // too); after a binary operator, one operand that keeps the `;` after it)
                                let item_start = ps == 0
                                    || matches!(&toks[ps - 1], TokenTree::Punct(q) if q.as_char() == ';')
                                    || matches!(&toks[ps - 1], TokenTree::Group(pg) if matches!(pg.delimiter(), proc_macro2::Delimiter::Brace | proc_macro2::Delimiter::Bracket));
                                let operand = top
                                    && !item_start
                                    && matches!(&toks[ps - 1], TokenTree::Punct(q) if matches!(q.as_char(), '+' | '-' | '*' | '/' | '%' | '^' | '|' | '&' | '!'));
                                let rep = if operand {
                                    format!("({body})")
                                } else if top || spliced {
                                    body
                                } else if stmt_pos {
                                    match split_items(&body) {
                                        Some((items, rest)) => format!("{items} ({{ {rest} }})"),
                                        None => format!("({{ {body} }})"),
                                    }
                                } else {
                                    format!("({{ {body} }})")
                                };
                                let mut a = name.span().byte_range().start;
                                // (`crate::m!(..)` / `self::m!(..)`: 前のパスも消す)
                                let mut k = i;
                                while k >= 3 {
                                    match (&toks[k - 3], &toks[k - 2], &toks[k - 1]) {
                                        (TokenTree::Ident(seg), TokenTree::Punct(c1), TokenTree::Punct(c2))
                                            if c1.as_char() == ':' && c2.as_char() == ':' && path_seg(seg.to_string().as_bytes()) =>
                                        {
                                            a = seg.span().byte_range().start;
                                            k -= 3;
                                        }
                                        _ => break,
                                    }
                                }
                                if colons(k) {
                                    a = toks[k - 2].span().byte_range().start;
                                }
                                let mut e = g.span().byte_range().end;
                                // (トップレベルの `name!(..);` の `;` も取る)
                                if (top && item_start) || drop_semi {
                                    if let Some(TokenTree::Punct(s)) = toks.get(i + 3) {
                                        if s.as_char() == ';' {
                                            e = s.span().byte_range().end;
                                        }
                                    }
                                }
                                out.push((a, e, rep));
                                done = true;
                                break;
                            }
                        }
                        if !done {
                            let lc = name.span().start();
                            return Err(format!("{}:{}: no rules of `{name}!` matched this call", lc.line, lc.column + 1));
                        }
                        i += 3;
                        continue;
                    }
                }
            }
        }
        // (中にマクロの名前が無い括弧は辿らない: 括弧ごとに中身を写すと、深く入れ子になったソースで遅い)
        // (nor the arguments of the builtin `stringify!(..)`: they are its text, not calls, as in expand_with. The
        // builtin's path only, as there: `stringify!`, `std::` / `core::stringify!`, `::std::stringify!`)
        let punct = |k: usize, c: char| matches!(&toks[k], TokenTree::Punct(p) if p.as_char() == c);
        let colons = |k: usize| k >= 2 && punct(k - 1, ':') && punct(k - 2, ':');
        let inert = i >= 2
            && matches!(&toks[i - 2], TokenTree::Ident(x) if x == "stringify")
            && punct(i - 1, '!')
            && !macros.contains_key("stringify")
            && {
                let s = i - 2;
                !colons(s)
                    || (s >= 3
                        && matches!(&toks[s - 3], TokenTree::Ident(x) if x == "std" || x == "core")
                        && !(colons(s - 3)
                            && s >= 6
                            && matches!(&toks[s - 6], TokenTree::Ident(x) if path_seg(x.to_string().as_bytes()))
                            && !(s >= 7 && punct(s - 7, '\''))))
            };
        if let (TokenTree::Group(g), false) = (&toks[i], inert) {
            let r = g.span().byte_range();
            let inner = if r.start <= r.end && r.end <= text.len() { &text[r.start..r.end] } else { "" };
            if inner.is_empty() || macros.keys().any(|m| inner.contains(m.as_str())) {
                let brace = g.delimiter() == proc_macro2::Delimiter::Brace;
                // (a place of items, as expand_cfg's item_scope reads it: the body of a `mod`, `impl`, `trait` or
                // `extern` block, going back over its head to the item before it; not a fn's or a struct's body)
                let module = brace && {
                    let mut k = i;
                    let mut found = false;
                    // (a brace inside the head's `<..>` is a const argument, an expression: `impl S<{ m!() * 3 }> {}`)
                    let mut angle = 0i32;
                    while k > 0 {
                        k -= 1;
                        match &toks[k] {
                            TokenTree::Punct(q) if q.as_char() == '>' && !(k > 0 && matches!(&toks[k - 1], TokenTree::Punct(a) if matches!(a.as_char(), '-' | '=') && a.spacing() == proc_macro2::Spacing::Joint)) => angle += 1,
                            TokenTree::Punct(q) if q.as_char() == '<' => angle -= 1,
                            TokenTree::Punct(q) if q.as_char() == ';' => break,
                            // (a braced const argument inside the head's `<..>`: stepped over)
                            TokenTree::Group(pg) if pg.delimiter() == proc_macro2::Delimiter::Brace && angle > 0 => {}
                            TokenTree::Group(pg) if pg.delimiter() == proc_macro2::Delimiter::Brace => break,
                            TokenTree::Ident(w) if matches!(w.to_string().as_str(), "fn" | "struct" | "enum" | "union" | "let") => break,
                            // (an `impl` that starts an item, as item_scope: not `-> impl Display`)
                            TokenTree::Ident(w) if w == "impl" && !(k == 0
                                || matches!(&toks[k - 1], TokenTree::Punct(q) if q.as_char() == ';')
                                || matches!(&toks[k - 1], TokenTree::Group(pg) if matches!(pg.delimiter(), proc_macro2::Delimiter::Brace | proc_macro2::Delimiter::Bracket))
                                || matches!(&toks[k - 1], TokenTree::Ident(u) if u == "unsafe" || u == "default")) => {}
                            TokenTree::Ident(w) if matches!(w.to_string().as_str(), "mod" | "impl" | "trait" | "extern") => {
                                found = angle >= 0;
                                break;
                            }
                            _ => {}
                        }
                    }
                    found
                };
                find_calls(g.stream(), macros, text, depth + 1, brace, module, out)?;
            }
        }
        i += 1;
    }
    Ok(())
}

/// Whether src may call `env!` / `option_env!` (also written `env !` or `env /* .. */ !`): the one look a file gets
/// before expand_env
pub fn may_use_env(src: &str) -> bool {
    if !src.contains("env") {
        return false;
    }
    let b = src.as_bytes();
    src.match_indices("env").any(|(k, _)| {
        let mut j = k + 3;
        loop {
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if b[j..].starts_with(b"/*") {
                j = src[j + 2..].find("*/").map_or(b.len(), |e| j + 2 + e + 2);
            } else if b[j..].starts_with(b"//") {
                j = src[j..].find('\n').map_or(b.len(), |e| j + e);
            } else {
                break;
            }
        }
        b.get(j) == Some(&b'!')
    })
}

/// `env!("NAME")` / `env!("NAME", "message")` and `option_env!("NAME")` with the crate's compile-time environment
/// (`CfgSet::env`, then zustc's own): `env!` becomes the value as a string literal, `option_env!` `Some("value")`.
/// A name that is not set is left as written: `option_env!` is then `None` (acg), and `env!` the error that names it.
/// Found by tokens, so the text of string literals and comments is not read. The value goes on one line (newlines
/// escaped), followed by the newlines the invocation spanned, so the lines after it keep their numbers. None: nothing
/// to expand (the caller looks for `env!` first)
pub fn expand_env(src: &str, cfg: &crate::cfg::CfgSet) -> Option<String> {
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    let is_lit = |k: usize| toks.get(k).is_some_and(|x| x.0 == 2);
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    let mut changed = false;
    let mut i = 0;
    while i < toks.len() {
        // (`stringify!(env!("X"))` is the text of the tokens, not their value)
        if t(i) == b"stringify" && t(i + 1) == b"!" && matches!(t(i + 2), b"(" | b"[" | b"{") {
            let mut depth = 0usize;
            let mut j = i + 2;
            while j < toks.len() {
                if toks[j].0 == 1 {
                    match t(j) {
                        b"(" | b"[" | b"{" => depth += 1,
                        b")" | b"]" | b"}" => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            i = j + 1;
            continue;
        }
        let opt = t(i) == b"option_env";
        if !(toks[i].0 == 0 && (opt || t(i) == b"env") && t(i + 1) == b"!" && t(i + 2) == b"(" && is_lit(i + 3)) {
            i += 1;
            continue;
        }
        // (the arguments: `"NAME"`, `"NAME",`, and for env! `"NAME", "message"`)
        let close = if t(i + 4) == b")" {
            i + 4
        } else if t(i + 4) == b"," && t(i + 5) == b")" {
            i + 5
        } else if !opt && t(i + 4) == b"," && is_lit(i + 5) && t(i + 6) == b")" {
            i + 6
        } else if !opt && t(i + 4) == b"," && is_lit(i + 5) && t(i + 6) == b"," && t(i + 7) == b")" {
            i + 7
        } else {
            i += 1;
            continue;
        };
        let name = match t(i + 3).strip_prefix(b"\"").and_then(|r| r.strip_suffix(b"\"")) {
            Some(n) if !n.is_empty() && !n.contains(&b'\\') => String::from_utf8_lossy(n).into_owned(),
            _ => {
                i = close + 1;
                continue;
            }
        };
        // (`std::env!`, `::core::option_env!`: the path goes too. Another path (`custom::env!`, `a::std::env!`) names
        // a macro of its own: left as it is)
        let back = |k: usize, d: usize| if k >= d { t(k - d) } else { b"" };
        let mut st = i;
        if back(st, 1) == b":" && back(st, 2) == b":" {
            if !matches!(back(st, 3), b"std" | b"core") {
                i = close + 1;
                continue;
            }
            st -= 3;
            if back(st, 1) == b":" && back(st, 2) == b":" {
                // (`a::std::env!`, `<T>::std::env!`: a path of its own; `return ::std::env!(..)`: std's)
                let kw = matches!(back(st, 3), b"return" | b"break" | b"in" | b"if" | b"else" | b"match" | b"while" | b"move" | b"mut" | b"let" | b"yield" | b"unsafe" | b"async");
                if st >= 3 && !kw && (toks[st - 3].0 == 0 || t(st - 3) == b">") {
                    i = close + 1;
                    continue;
                }
                st -= 2;
            }
        }
        let Some(v) = cfg.env_var(&name) else {
            i = close + 1;
            continue;
        };
        let (a, b) = (toks[st].1, toks[close].2);
        out.push_str(&src[at..a]);
        let lit = str_literal(&v);
        if opt {
            out.push_str("Some(");
            out.push_str(&lit);
            out.push(')');
        } else {
            out.push_str(&lit);
        }
        for _ in 0..src[a..b].matches('\n').count() {
            out.push('\n');
        }
        at = b;
        changed = true;
        i = close + 1;
    }
    if !changed {
        return None;
    }
    out.push_str(&src[at..]);
    Some(out)
}

/// A string literal (on one line) whose value is s
fn str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `include_str!("path")` / `include_bytes!("path")` (path は文字列リテラル、`dir` からの相対) を、ファイルの中身の
/// リテラル (改行などはエスケープして 1 行に収める: 後ろの行番号を変えない) に置き換える。無ければ None
pub fn expand_includes(src: &str, dir: &std::path::Path) -> Result<Option<String>, String> {
    // (one look at the source for all three macros)
    if !src.contains("include") {
        return Ok(None);
    }
    let code = expand_include_code(src, dir, 0)?;
    let src = code.as_deref().unwrap_or(src);
    if !src.contains("include_str!(") && !src.contains("include_bytes!(") {
        return Ok(code);
    }
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    let mut changed = false;
    loop {
        let a = rest.find("include_str!(");
        let b = rest.find("include_bytes!(");
        let (i, bytes) = match (a, b) {
            (Some(a), Some(b)) => if a < b { (a, false) } else { (b, true) },
            (Some(a), None) => (a, false),
            (None, Some(b)) => (b, true),
            (None, None) => break,
        };
        let head = if bytes { "include_bytes!(" } else { "include_str!(" };
        // (コメントの中と、文字列の中 (`"include_str!("`) のものは展開しない)
        let abs = src.len() - rest.len() + i;
        let line_start = src[..abs].rfind('\n').map(|k| k + 1).unwrap_or(0);
        let in_comment = src[line_start..abs].contains("//")
            || src[..abs].ends_with(|c: char| c == '"' || c == '_' || c.is_alphanumeric());
        let after = &rest[i + head.len()..];
        let lit = after.trim_start();
        let parsed = (!in_comment && lit.starts_with('"'))
            .then(|| lit[1..].find('"').map(|e| (&lit[1..1 + e], &lit[1 + e + 1..])))
            .flatten()
            .and_then(|(p, tail)| {
                let t = tail.trim_start();
                t.starts_with(')').then(|| (p, src.len() - (t.len() - 1)))
            });
        let Some((p, end_abs)) = parsed.filter(|(p, _)| !p.contains('\\')) else {
            out.push_str(&rest[..i + head.len()]);
            rest = &rest[i + head.len()..];
            continue;
        };
        let path = dir.join(p);
        let data = std::fs::read(&path).map_err(|e| format!("couldn't read `{}`: {e}", path.display()))?;
        out.push_str(&rest[..i]);
        if bytes {
            out.push_str("b\"");
        } else {
            std::str::from_utf8(&data).map_err(|_| format!("`{}` wasn't a utf-8 file", path.display()))?;
            out.push('"');
        }
        if bytes {
            for &c in &data {
                match c {
                    b'"' => out.push_str("\\\""),
                    b'\\' => out.push_str("\\\\"),
                    0x20..=0x7e => out.push(c as char),
                    _ => out.push_str(&format!("\\x{c:02x}")),
                }
            }
        } else {
            for ch in std::str::from_utf8(&data).unwrap().chars() {
                match ch {
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
                    c => out.push(c),
                }
            }
        }
        out.push('"');
        let consumed = end_abs - (src.len() - rest.len());
        rest = &rest[consumed..];
        changed = true;
    }
    out.push_str(rest);
    Ok(if changed { Some(out) } else { code })
}

/// The path of `include!("a")` / `include!(concat!("a", "b"))`: (path, the rest after the closing paren)
fn include_path(after: &str) -> Option<(String, &str)> {
    let mut t = after.trim_start();
    let concat = t.starts_with("concat!(");
    if concat {
        t = t["concat!(".len()..].trim_start();
    }
    let mut path = String::new();
    loop {
        let r = t.strip_prefix('"')?;
        let e = r.find('"')?;
        if r[..e].contains('\\') {
            return None;
        }
        path.push_str(&r[..e]);
        t = r[e + 1..].trim_start();
        match t.strip_prefix(',') {
            Some(x) if concat => t = x.trim_start(),
            _ => break,
        }
    }
    if concat {
        t = t.strip_prefix(')')?.trim_start();
    }
    Some((path, t.strip_prefix(')')?))
}

/// `include!("file.rs")`: the file's code in its place, its own includes expanded relative to it. As a statement /
/// item (`include!(..);`) the code goes in as is, as an expression in parentheses
fn expand_include_code(src: &str, dir: &std::path::Path, depth: u32) -> Result<Option<String>, String> {
    if !src.contains("include!(") {
        return Ok(None);
    }
    if depth > 32 {
        return Err("include! nests too deep".into());
    }
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    let mut changed = false;
    while let Some(k) = src[at..].find("include!(") {
        let i = at + k;
        let line_start = src[..i].rfind('\n').map(|k| k + 1).unwrap_or(0);
        let skip = src[line_start..i].contains("//") || src[..i].ends_with(|c: char| c == '"' || c == '_' || c == ':' || c.is_alphanumeric());
        let parsed = (!skip).then(|| include_path(&src[i + "include!(".len()..])).flatten();
        let Some((p, tail)) = parsed else {
            out.push_str(&src[at..i + "include!(".len()]);
            at = i + "include!(".len();
            continue;
        };
        let path = dir.join(&p);
        let code = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read `{}`: {e}", path.display()))?;
        // (a `#!` line that is not an inner attribute is a shebang: dropped where items go (outside any braces),
        // kept, as rustc does, in an expression or a statement)
        let top = src[..i].bytes().fold(0i32, |d, b| d + (b == b'{') as i32 - (b == b'}') as i32) == 0;
        let code = match code.strip_prefix("#!") {
            Some(r) if top && !r.trim_start().starts_with('[') => r.split_once('\n').map(|x| x.1).unwrap_or("").to_string(),
            _ => code,
        };
        let sub = dir.join(&p);
        let code = match expand_include_code(&code, sub.parent().unwrap_or(dir), depth + 1)? {
            Some(c) => c,
            None => code,
        };
        // (`mod m;` in a file included from another directory: m's file is beside the included file)
        let rel = std::path::Path::new(&p).parent().filter(|r| !r.as_os_str().is_empty());
        let code = match rel {
            Some(rel) if code.contains("mod ") && !code.contains("#[path") => code
                .lines()
                .map(|l| {
                    let t = l.trim_start();
                    let decl = t.strip_prefix("pub ").unwrap_or(t).strip_prefix("mod ").and_then(|r| r.trim_end().strip_suffix(';'));
                    match decl.map(str::trim).filter(|n| !n.is_empty() && n.chars().all(|c| c == '_' || c.is_alphanumeric())) {
                        Some(n) => {
                            let file = if sub.parent().unwrap_or(dir).join(format!("{n}.rs")).exists() { format!("{n}.rs") } else { format!("{n}/mod.rs") };
                            format!("#[path = \"{}\"] {l}", rel.join(file).display())
                        }
                        None => l.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => code,
        };
        let end = src.len() - tail.len();
        let stmt = tail.trim_start().starts_with(';');
        out.push_str(&src[at..i]);
        if stmt {
            out.push_str(&code);
            out.push('\n');
            at = end + tail.find(';').unwrap() + 1;
        } else {
            out.push('(');
            out.push_str(code.trim());
            out.push(')');
            at = end;
        }
        changed = true;
    }
    out.push_str(&src[at..]);
    Ok(changed.then_some(out))
}

#[cfg(test)]
mod env_tests {
    use super::*;

    #[test]
    fn env_macros_and_their_paths() {
        let c = crate::cfg::CfgSet { env: vec![("LABEL".into(), "v".into())], ..Default::default() };
        let x = |s: &str| expand_env(s, &c).unwrap_or_else(|| s.to_string());
        assert_eq!(x("f(env!(\"LABEL\"), ::std::env!(\"LABEL\"), core::option_env!(\"LABEL\"));"), "f(\"v\", \"v\", Some(\"v\"));");
        // (another path names a macro of its own)
        assert_eq!(x("custom::env!(\"LABEL\"); a::std::env!(\"LABEL\");"), "custom::env!(\"LABEL\"); a::std::env!(\"LABEL\");");
        assert_eq!(x("env ! (\"LABEL\")"), "\"v\"");
        assert_eq!(x("return ::std::env!(\"LABEL\");"), "return \"v\";");
        assert_eq!(x("stringify!(env!(\"LABEL\")); env!(\"LABEL\")"), "stringify!(env!(\"LABEL\")); \"v\"");
        assert!(may_use_env("let a = env ! (\"X\");") && may_use_env("option_env!(\"X\")"));
        assert!(!may_use_env("use std::env; fn environment() {}"));
        assert!(may_use_env("env /* a comment */ !(\"X\")") && may_use_env("env // why\n !(\"X\")"));
        assert_eq!(x("env /* c */ !(\"LABEL\")"), "\"v\"");
    }

    /// A dependency's macros come after DEP_MACROS_MARKER, at the end of the text: they are in scope from the top
    /// (`#[macro_use] extern crate cfg_if;` as getrandom 0.4 has), unlike the crate's own `macro_rules!`, which is
    /// usable only below its definition
    #[test]
    fn dependency_macros_are_in_scope_above_their_text() {
        let own = "fn main() { m!(); }\nmacro_rules! m { () => {}; }\n";
        assert!(expand(own).is_err());
        let dep = format!("fn main() {{ m!(); }}\n{DEP_MACROS_MARKER}macro_rules! m {{ () => {{}}; }}\n");
        assert!(expand(&dep).is_ok(), "{:?}", expand(&dep).err());
    }

    /// A macro invoked in an `extern` block expands to items there (foreign fns), not to an expression: libc 0.2
    /// calls `cfg_if!` inside `extern "C" { .. }`, and cfg_if calls itself again behind `#[cfg(..)]` (`ci!` here has
    /// the same shape). An `extern "C" fn` body is still a block of statements
    #[test]
    fn macro_in_extern_block_is_items() {
        let src = "macro_rules! decl { ($n:ident) => { pub fn $n(x: i32) -> i32; }; }\nmacro_rules! ci { ($(#[$m:meta] $i:item)*) => { $( #[$m] ci! { @id $i } )* }; (@id $($t:tt)*) => { $($t)* }; }\nunsafe extern \"C\" {\n    ci! { #[cfg(unix)] pub fn abs(x: i32) -> i32; #[cfg(unix)] pub fn labs(x: i32) -> i32; }\n}\nmacro_rules! decl2 { () => { 1 }; }\nextern \"C\" fn g() -> i32 { decl2!() }\n";
        let out = expand(src).unwrap().unwrap();
        let norm = out.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!norm.contains("({"), "{out}");
        assert!(norm.contains("pub fn abs (x : i32 ) -> i32 ;") && norm.contains("pub fn labs (x : i32 ) -> i32 ;"), "{out}");
        assert!(norm.contains("extern \"C\" fn g() -> i32 { 1 }"), "{out}");
    }

    /// `$($it:item)*` takes each item on its own: two `const` items are two fragments, so `$(#[$m] $it)*` puts the
    /// attribute on both (libc 0.2's `cfg_if!` applies each branch's cfg this way; taking both consts as one item
    /// left the second without its cfg, a duplicate definition)
    #[test]
    fn item_fragments_split_at_each_item() {
        let src = "macro_rules! ap { ($m:meta, $($it:item)*) => { $(#[$m] $it)* }; }\nap! { cfg(any()), pub const A: i32 = 0; pub const B: i32 = 1; pub static S: u8 = 2; pub fn f() {} }\nfn main() {}\n";
        let c = crate::cfg::CfgSet { debug_assertions: true, ..Default::default() };
        let out = expand_cfg(src, Some(&c)).unwrap().unwrap();
        for n in ["A", "B", "S", "f"] {
            assert!(!out.split(|c: char| !c.is_alphanumeric()).any(|w| w == n), "{n} left: {out}");
        }
    }

    /// A `macro_rules!` inside another macro's arguments is that macro's input, not a definition yet: quote 1.0
    /// wraps its `quote!` in `__quote![ #[macro_export] macro_rules! quote { .. } ];`. Taking it out of the call
    /// left the call empty ("no rules matched"); it is defined when the call's expansion puts it in place
    #[test]
    fn macro_rules_in_call_arguments() {
        let src = "macro_rules! wrap { ($it:item) => { $it }; }\nwrap![ #[macro_export] macro_rules! q { () => { 0 }; ($t:tt) => { $t }; } ];\nfn main() { let _a = q!(); let _b = q!(5); }\n";
        let out = expand(src);
        assert!(out.is_ok(), "{out:?}");
        let norm = out.unwrap().unwrap().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(norm.contains("let _a = ({ 0 })") && norm.contains("let _b = ({ 5 })"), "{norm}");
    }

    /// ..but a std macro's arguments (`println!("{}", { macro_rules! foo { .. } foo!("rust") })`, tests/ui
    /// macro-nested_definition_issue-31946) are not expanded by macro_rules: a definition there is collected
    #[test]
    fn macro_rules_in_std_macro_arguments() {
        let src = "fn main() {\n    println!(\"{}\", {\n        macro_rules! foo { ($n:expr) => { $n + 1 } }\n        foo!(41)\n    });\n}\n";
        let out = expand(src);
        assert!(out.is_ok(), "{out:?}");
        let norm = out.unwrap().unwrap().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!norm.contains("foo!"), "{norm}");
    }

    /// A definition goes with all its attributes: `#[macro_export] #[doc(hidden)] macro_rules! m { .. }` (bitflags
    /// 2) left `#[macro_export]` with no item after it ("unterminated item")
    #[test]
    fn macro_definition_with_several_attributes() {
        let src = "#[macro_export]\n#[doc(hidden)]\nmacro_rules! m { () => { 1 }; }\nfn main() { let _x = m!(); }\n";
        let out = expand(src).unwrap().unwrap();
        assert!(!out.contains("#["), "{out}");
        assert!(crate::parse::parse_file(&out).is_ok(), "{out}");
    }

    /// `$i:item` does not end at a brace inside generic arguments: `struct S<const N: usize = { 1 }>;`
    #[test]
    fn item_fragment_with_braces_in_generics() {
        let src = "macro_rules! keep { ($i:item) => { $i }; }\nkeep! { struct S<const N: usize = { 1 }>; }\nfn main() {}\n";
        let out = expand(src);
        assert!(out.is_ok(), "{out:?}");
        let flat: String = out.unwrap().unwrap().chars().filter(|c| !c.is_whitespace()).collect();
        assert!(flat.contains("structS<constN:usize={1}>;"), "{flat}");
    }

    /// `#![recursion_limit]` is read from the crate's attribute, not from a comment or a string
    #[test]
    fn recursion_limit_from_the_attribute_only() {
        assert_eq!(super::recursion_limit("// #![recursion_limit = \"0\"]\nfn main() {}"), 128);
        assert_eq!(super::recursion_limit("const S: &str = \"#![recursion_limit = \\\"3\\\"]\";"), 128);
        assert_eq!(super::recursion_limit("#![recursion_limit = \"300\"]\nfn main() {}"), 300);
        assert_eq!(super::recursion_limit("#![ recursion_limit = \"64\" ]"), 64);
    }

    /// A limit counts expansion depth: with `#![recursion_limit = "1"]` a call whose expansion needs no more
    /// expansion completes, even though the expansion's text names the macro (`"m"`)
    #[test]
    fn recursion_limit_one_completes() {
        let src = "#![recursion_limit = \"1\"]\nmacro_rules! m { () => { \"m\" }; }\nfn main() { let _ = m!(); }\n";
        assert!(expand(src).is_ok(), "{:?}", expand(src).err());
        assert!(expand_reference(src).is_ok());
    }

    /// A braced call in statement position whose expansion carries only `#[cfg(..)]` attributes (cfg-if's
    /// `cfg_if! { if #[cfg(..)] { let tokens = ..; } else { .. } }`, chacha20 0.10) puts its statements into the
    /// block, so a `let` it declares is seen by what follows (`Self { tokens }`)
    #[test]
    fn statement_cfg_if_declares_into_the_block() {
        let src = "macro_rules! pick { (if #[cfg($m:meta)] { $($a:tt)* } else { $($b:tt)* }) => { #[cfg($m)] pick! { @id $($a)* } #[cfg(not($m))] pick! { @id $($b)* } }; (@id $($t:tt)*) => { $($t)* }; }\nfn f() -> u8 {\n    pick! {\n        if #[cfg(unix)] { let tokens = 1; } else { let tokens = 2; }\n    }\n    tokens\n}\nfn main() {}\n";
        let c = crate::cfg::CfgSet { debug_assertions: true, ..Default::default() };
        let out = expand_cfg(src, Some(&c)).unwrap().unwrap();
        let norm = out.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!norm.contains("({"), "{norm}");
        assert!(crate::parse::parse_file(&out).is_ok(), "{norm}");
    }

    /// A body's own `let core` is renamed for hygiene, but not a field or method of that name after `.`:
    /// `self.core.core` (chacha20 0.10's `impl_chacha_rng!`). A parameter of the name is renamed with it
    #[test]
    fn hygiene_keeps_fields_of_a_let_name() {
        let src = "macro_rules! imp { () => { fn h(core: u8) -> u8 { core } fn f(r: &mut R) -> u8 { let core = 1; r.core.core = core; let x = core..core; r.core() } } }\nimp!();\nfn main() {}\n";
        let out = expand_cfg(src, Some(&crate::cfg::CfgSet::default())).unwrap().unwrap();
        let norm: String = out.split_whitespace().collect();
        assert!(norm.contains("r.core.core=core__mh"), "{norm}");
        assert!(norm.contains("core__mh..core__mh"), "{norm}");
        assert!(norm.contains("r.core()"), "{norm}");
        assert!(norm.contains("fnh(core__mh:u8)->u8{core__mh}"), "{norm}");
    }

    /// `stringify!(m!())` is the call's text in both expanders (#199), through the builtin's paths; a library's
    /// `re::stringify!` / `re::std::stringify!` can be another macro re-exported under that name, so the calls in its
    /// arguments expand, as before
    #[test]
    fn stringify_keeps_calls_as_text() {
        let both = |call: &str| {
            let src = format!("macro_rules! m {{ () => {{ 7 }}; }}\nfn main() {{ let _ = {call}; }}\n");
            let (a, b) = (expand(&src).unwrap().unwrap(), expand_reference(&src).unwrap().unwrap());
            assert_eq!(a, b, "{call}: the two expanders");
            a
        };
        for call in ["stringify!(m!())", "std::stringify!(m!())", "core::stringify!(m![])", "::std::stringify! { m!() }"] {
            assert!(both(call).contains(call), "{call}");
        }
        // (a label before the absolute path is no path segment: `break 'done ::std::stringify!(..)`)
        let call = "'done: loop { break 'done ::std::stringify!(m!()) }";
        assert!(both(call).contains("::std::stringify!(m!())"), "{call}");
        for call in ["re::stringify!(m!())", "re::std::stringify!(m!())", "::re::core::stringify!(m!())"] {
            assert!(!both(call).contains("m!()"), "{call}");
        }
    }

    /// The depth of recursive expansion is limited as in rustc: 128 by default, or the crate's
    /// `#![recursion_limit = "N"]` (libc 0.2's `cfg_if!` chains and `s!` items go deeper than 64)
    #[test]
    fn recursion_limit() {
        let src = |n: usize, attr: &str| {
            format!(
                "{attr}macro_rules! deep {{ () => {{ 0 }}; ($x:tt $($rest:tt)*) => {{ 1 + deep!($($rest)*) }}; }}\nfn main() {{ let _n = deep!({}); }}\n",
                "a ".repeat(n)
            )
        };
        for (n, attr, ok) in [(100, "", true), (150, "", false), (200, "#![recursion_limit = \"300\"]\n", true)] {
            let s = src(n, attr);
            assert_eq!(expand(&s).is_ok(), ok, "fast, depth {n} {attr:?}: {:?}", expand(&s).err());
            assert_eq!(expand_reference(&s).is_ok(), ok, "reference, depth {n} {attr:?}");
        }
    }
}
