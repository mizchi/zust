//! Preprocessing (expanding thread_local!, expanding macros, flattening `mod`, moving nested items, rewriting constructors), done incrementally
//! by remembering it per chunk across edits (on LSP didChange, the parts that did not change are not rebuilt).
//!
//! - Chunk: a range of lines cut at item boundaries (inside `mod` / `impl` / `trait` too). A boundary is after `}` / `;` / an opening `{`,
//!   before a token that begins a line (with only whitespace before it).
//! - Every preprocessing step rewrites token by token using what is collected from the whole file (macro definitions, element counts of tuple structs and variants, the module tree and `use`s,
//!   names of nested items), and does not change the number of lines (moved items are appended at the end).
//!   What is collected depends only on item declarations, so the same things are collected from a "skeleton" with the expressions inside fn bodies blanked out (nested items, `use`s,
//!   `macro_rules!`, calls of user macros that may make an item, and brackets and `;` are kept; the number of lines is the same), and the fn signatures too (`fn name() {`:
//!   no name, module, macro or arity is collected from one).
//! - If the whole skeleton is the same as last time, unchanged chunks use the previous results, and only the changed chunks are preprocessed in a source where the rest is turned into
//!   skeleton, and cut out by lines. If the skeleton changed (a declaration changed), the whole is preprocessed again.
//! - `ZUSTC_PREP_VERIFY=1` compares with a full preprocessing every time. `ZUSTC_NO_PREP_INC=1` does not use the incremental path.

use crate::fxhash::FxMap;
use crate::lexer;

pub type R<T> = Result<T, String>;

/// The result of preprocessing
pub struct Prep {
    pub text: String,
    /// When nested items were moved to the end: (start line in the input (0-based), number of newlines) of each moved item
    pub moved: Option<Vec<(usize, usize)>>,
}

fn count_nl(b: &[u8]) -> usize {
    let mut n = 0;
    for &c in b {
        if c == b'\n' {
            n += 1;
        }
    }
    n
}

fn fnv(h: u64, b: &[u8]) -> u64 {
    let mut h = h;
    for &c in b {
        h = (h ^ c as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

fn mix(h: u64, x: u64) -> u64 {
    let h = (h ^ x).wrapping_mul(0x0100_0000_01b3);
    h ^ (h >> 29)
}

/// Preprocesses the whole (in with_prelude_in's original order)
pub fn prep_full(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>, cfg: Option<&crate::cfg::CfgSet>) -> R<Prep> {
    let tl = lexer::expand_thread_local(src);
    let s1: &str = match &tl {
        Some(t) => t.as_str(),
        None => src,
    };
    let expanded = crate::macros::expand_cfg(s1, cfg)?;
    let s2: &str = match &expanded {
        Some(t) => t.as_str(),
        None => s1,
    };
    if let Some(p) = std::env::var_os("ZUSTC_DUMP_PRE") {
        let _ = std::fs::write(p, s2);
    }
    let flat = lexer::flatten_modules_fix(s2, crates, fix);
    let s3: &str = match &flat {
        Some(t) => t.as_str(),
        None => s2,
    };
    let mut moved: Option<Vec<(usize, usize)>> = None;
    // (where the moved items start in the hoisted text: after the body, which renaming may have made shorter than s3)
    let mut appended_at = 0usize;
    let hoisted = match lexer::hoist_nested_items_parts(s3) {
        Some((body, parts)) => {
            if parts.is_empty() {
                Some(body)
            } else {
                // (Start lines of the moved ones: the body has the same sequence of lines as the input)
                let bb = body.as_bytes();
                let mut info = vec![];
                appended_at = body.len();
                let mut out = body.clone().into_bytes();
                let (mut at, mut line) = (0usize, 0usize);
                for (pos, t) in &parts {
                    line += count_nl(&bb[at..*pos]);
                    at = *pos;
                    info.push((line, count_nl(t.as_bytes())));
                    out.push(b'\n');
                    out.extend_from_slice(t.as_bytes());
                }
                out.push(b'\n');
                moved = Some(info);
                Some(String::from_utf8(out).map_err(|e| e.to_string())?)
            }
        }
        None => None,
    };
    // (a `mod` moved out of a function body is flattened like the others)
    let reflat = match &hoisted {
        // (in the moved items only, by bytes)
        Some(t) if moved.is_some() && t.as_bytes()[appended_at.min(t.len())..].windows(4).any(|w| w == b"mod ") => lexer::flatten_modules_fix(t, crates, fix),
        _ => None,
    };
    let hoisted = reflat.or(hoisted);
    let s4: &str = match &hoisted {
        Some(t) => t.as_str(),
        None => s3,
    };
    let cv = crate::ctors::rewrite_ctor_values(s4);
    let text = match cv {
        Some(t) => t,
        None => s4.to_string(),
    };
    Ok(Prep { text, moved })
}

/// The start of a source file without a byte order mark and a shebang line (`#!/usr/bin/env ..`, blanked to keep the
/// line numbers; `#![attr]` is not one). None when there is neither
fn file_start(src: &str) -> Option<String> {
    let bom = src.starts_with('\u{feff}');
    let s = if bom { &src[3..] } else { src };
    if !s.starts_with("#!") {
        return bom.then(|| s.to_string());
    }
    // (`#!` then whitespace / comments then `[` is an inner attribute)
    let r = s[2..].as_bytes();
    let mut i = 0;
    loop {
        while i < r.len() && r[i].is_ascii_whitespace() {
            i += 1;
        }
        if r[i..].starts_with(b"//") {
            while i < r.len() && r[i] != b'\n' {
                i += 1;
            }
        } else if r[i..].starts_with(b"/*") {
            match s[2 + i + 2..].find("*/") {
                Some(e) => i += 2 + e + 2,
                None => break,
            }
        } else {
            break;
        }
    }
    if r.get(i) == Some(&b'[') {
        return bom.then(|| s.to_string());
    }
    let nl = s.find('\n').unwrap_or(s.len());
    let mut out = String::with_capacity(s.len());
    out.push_str(&" ".repeat(nl));
    out.push_str(&s[nl..]);
    Some(out)
}

/// The result of a chunk: lines of the body (with newlines), items moved to the end (line within the chunk, text), column shifts (lines are counted from the start of the chunk,
/// 0-based)
struct ChunkOut {
    text: String,
    pieces: Vec<(usize, String)>,
    fix: Vec<(u32, u32, i32)>,
}

/// What a hoisted item starts with, on its first line: the line it was written on (from 1), `/*zust:line=12*/`, which
/// an error in it is reported at (a comment: no token, the item's hash stays). A line the program starts with such a
/// comment of its own is read as a mark too (the lines after it, to the next mark, as written at its line)
pub const HOIST_MARK: &str = "/*zust:line=";

fn hoist_mark(line0: usize) -> String {
    format!("{HOIST_MARK}{}*/", line0 + 1)
}

/// The full preprocessing's text, each hoisted item marked with the line it was written on
pub fn marked(p: Prep) -> String {
    let Some(info) = &p.moved else { return p.text };
    let b = p.text.as_bytes();
    let total_nl = b.iter().filter(|&&c| c == b'\n').count();
    let tail_nl: usize = info.iter().map(|&(_, nl)| nl + 1).sum::<usize>() + 1;
    // (the body has the input's lines; then `\n` and each piece, then a last `\n`)
    let Some(body_nl) = total_nl.checked_sub(tail_nl) else { return p.text };
    let mut nl = 0;
    let mut sep = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            if nl == body_nl {
                sep = i;
                break;
            }
            nl += 1;
        }
    }
    let mut out = String::with_capacity(p.text.len() + info.len() * 20);
    out.push_str(&p.text[..sep]);
    let mut at = sep;
    for &(line, pnl) in info {
        // (at the `\n` before the piece: the piece runs pnl lines more, to the next `\n`)
        out.push('\n');
        out.push_str(&hoist_mark(line));
        let start = at + 1;
        let mut e = start;
        let mut seen = 0;
        while e < b.len() && !(b[e] == b'\n' && seen == pnl) {
            if b[e] == b'\n' {
                seen += 1;
            }
            e += 1;
        }
        out.push_str(&p.text[start..e]);
        at = e;
    }
    out.push_str(&p.text[at..]);
    out
}

/// The hoisted items' marks of a preprocessed text: (its line, the line it was written on), in order. Empty without
pub fn hoist_marks(text: &str) -> Vec<(usize, usize)> {
    if !text.contains(HOIST_MARK) {
        return vec![];
    }
    text.split('\n')
        .enumerate()
        .filter_map(|(i, l)| Some((i + 1, l.strip_prefix(HOIST_MARK)?.split("*/").next()?.parse().ok()?)))
        .collect()
}

/// With hoist_marks: the line a line of a hoisted item was written on (None before every mark)
pub fn hoisted_at(marks: &[(usize, usize)], line: usize) -> Option<usize> {
    let k = marks.partition_point(|&(at, _)| at <= line);
    let &(at, orig) = marks.get(k.checked_sub(1)?)?;
    Some(orig + (line - at))
}

/// The line an error at `line` of the preprocessed text was written on, when it is in a hoisted item (HOIST_MARK)
pub fn hoisted_line(text: &str, line: usize) -> Option<usize> {
    if !text.contains(HOIST_MARK) {
        return None;
    }
    let mut best: Option<(usize, usize)> = None;
    let mut ln = 1;
    for (i, l) in text.split('\n').enumerate() {
        ln = i + 1;
        if ln > line {
            break;
        }
        if let Some(r) = l.strip_prefix(HOIST_MARK) {
            if let Some(n) = r.split("*/").next().and_then(|n| n.parse::<usize>().ok()) {
                best = Some((ln, n));
            }
        }
    }
    let _ = ln;
    best.map(|(at, orig)| orig + (line - at))
}

#[derive(Default)]
pub struct PrepMemo {
    /// Number of calls (the first one only preprocesses the whole: a build that preprocesses only once does not record chunks)
    calls: u32,
    /// crates and cfg
    key: u64,
    /// Hash of the whole skeleton
    fp: u64,
    /// Chunk hash → the macros defined in it
    names: FxMap<u64, Vec<MacroDef>>,
    /// Chunk hash → what is left of its delimiters once those that match are taken away (lexer::delims_left)
    delims: FxMap<u64, Option<Vec<u8>>>,
    /// (chunk hash, hash of the set of macro names) → (skeleton, its hash, whether it keeps something where the full
    /// preparation may read it otherwise (skeleton))
    skel: FxMap<(u64, u64), (String, u64, bool)>,
    /// The previous input and the chunk start positions (position, hash of what encloses it, levels)
    chunks: Option<(String, Vec<(usize, u64, Levels)>, bool)>,
    /// (chunk hash, hash of what encloses it (mod / impl / trait), index among identical ones) → result
    out: FxMap<(u64, u64, u32), ChunkOut>,
    /// Whether the last call reused chunks (the incremental path) rather than preprocessing the whole
    inc: bool,
    /// Keep the chunks from the first call on (keep_from_first)
    keep_first: bool,
}

impl PrepMemo {
    /// Whether the last call of `prep` with this memo reused the chunks it kept (the incremental path)
    pub fn was_incremental(&self) -> bool {
        self.inc
    }

    /// Keeps the chunks from the first call on, for a session whose document will be edited (zust-analyzer): its
    /// first edit can then reuse them. Otherwise the first call only preprocesses the whole, and a build that
    /// preprocesses once does not pay for keeping them (+20–40% of prep)
    pub fn keep_from_first(&mut self) {
        self.keep_first = true;
    }

    /// Forgets the chunks (not the number of calls, nor keep_from_first)
    fn forget(&mut self) {
        *self = PrepMemo { calls: self.calls, keep_first: self.keep_first, ..PrepMemo::default() };
    }
}

/// Chunk start position (byte), and the hash of what encloses it (the sequence of mod names / impl / trait)
type Levels = Vec<(i32, u64)>;

/// The chunk start positions (including from) of src[from..to] (from is a chunk boundary, levels the levels there), and the levels at the end.
/// If the end is not between items (or a comment / string may still be open), the levels at the end are None
fn chunk_range(src: &str, from: usize, to: usize, init: Levels) -> (Vec<(usize, u64, Levels)>, Option<Levels>) {
    let s = &src.as_bytes()[..to];
    let toks0 = lexer::tokenize(&s[from..]);
    let toks: Vec<(u8, usize, usize)> = toks0.iter().map(|t| (t.0, t.1 + from, t.2 + from)).collect();
    // (Levels: (depth of the braces inside, hash of what encloses))
    let mut levels: Levels = init;
    let first_ctx = levels[levels.len() - 1].1;
    let mut out: Vec<(usize, u64, Levels)> = vec![(from, first_ctx, levels.clone())];
    let mut depth = levels[levels.len() - 1].0;
    let mut pd = 0i32;
    // Whether we are between items, whether the previous token is `}` / `;` / a `{` that opens a level
    let mut at_b = true;
    let mut prev_ok = true;
    // The first word of the item (excluding pub / unsafe / async), the mod's name, whether fn was seen
    let mut head: Vec<u8> = vec![];
    let mut mod_name: Vec<u8> = vec![];
    let mut want_name = false;
    let mut saw_fn = false;
    let mut k = 0;
    while k < toks.len() {
        let (kind, a, b) = toks[k];
        let lvl = levels[levels.len() - 1];
        if at_b && prev_ok && pd == 0 && depth == lvl.0 && k > 0 {
            // (If only whitespace lies from the line start to this token, the line start is a boundary)
            let mut ls = a;
            while ls > 0 && s[ls - 1] != b'\n' {
                ls -= 1;
            }
            let mut blank = true;
            for &c in &s[ls..a] {
                if c != b' ' && c != b'\t' && c != b'\r' {
                    blank = false;
                }
            }
            if blank && ls > out[out.len() - 1].0 {
                out.push((ls, lvl.1, levels.clone()));
            }
        }
        if kind == 1 {
            let c = s[a];
            if c == b'{' {
                let hd: &[u8] = &head;
                let opens = pd == 0 && depth == lvl.0 && (hd == b"mod" || hd == b"impl" || hd == b"trait" || (hd == b"extern" && !saw_fn));
                depth += 1;
                if opens {
                    let mut h = mix(lvl.1, head.len() as u64);
                    h = fnv(h, &head);
                    if hd == b"mod" {
                        h = fnv(mix(h, 7), &mod_name);
                    }
                    levels.push((depth, h));
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else {
                    at_b = false;
                    prev_ok = false;
                }
            } else if c == b'}' {
                depth -= 1;
                if depth < lvl.0 && levels.len() > 1 {
                    levels.pop();
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else if depth == lvl.0 && pd == 0 {
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else {
                    prev_ok = false;
                }
            } else if c == b'(' || c == b'[' {
                pd += 1;
                at_b = false;
                prev_ok = false;
            } else if c == b')' || c == b']' {
                pd -= 1;
                prev_ok = false;
            } else if c == b';' {
                if depth == lvl.0 && pd == 0 {
                    at_b = true;
                    prev_ok = true;
                    head.clear();
                    saw_fn = false;
                } else {
                    prev_ok = false;
                }
            } else {
                at_b = false;
                prev_ok = false;
            }
        } else {
            if kind == 0 {
                let w = &s[a..b];
                if want_name {
                    mod_name = w.to_vec();
                    want_name = false;
                }
                if pd == 0 && depth == lvl.0 && head.is_empty() && w != b"pub" && w != b"unsafe" && w != b"async" {
                    head = w.to_vec();
                    if w == b"mod" {
                        want_name = true;
                    }
                }
                if w == b"fn" {
                    saw_fn = true;
                }
            }
            at_b = false;
            prev_ok = false;
        }
        k += 1;
    }
    let lvl = levels[levels.len() - 1];
    let last_end = match toks.last() {
        Some(t) => t.2,
        None => from,
    };
    let mut clean = at_b && prev_ok && pd == 0 && depth == lvl.0;
    // (After the last token, only whitespace and `//` comments)
    let g = &s[last_end..];
    let mut i = 0;
    while i < g.len() && clean {
        if g[i].is_ascii_whitespace() {
            i += 1;
        } else if g[i] == b'/' && i + 1 < g.len() && g[i + 1] == b'/' {
            while i < g.len() && g[i] != b'\n' {
                i += 1;
            }
        } else {
            clean = false;
        }
    }
    (out, if clean { Some(levels) } else { None })
}

/// Chunk start positions and the hashes of what encloses them (with a previous result, only re-cuts around the changed part)
/// The second is whether the brackets of the whole balance (the incremental path is not used for unbalanced sources)
fn chunk_starts(src: &str, memo: &mut Option<(String, Vec<(usize, u64, Levels)>, bool)>) -> (Vec<(usize, u64)>, bool) {
    let r = match memo.as_ref() {
        Some((old, st, bal)) => chunk_diff(old, st, src).map(|v| (v, *bal)),
        None => None,
    };
    let (st, bal) = match r {
        Some(v) => v,
        None => {
            let (v, end) = chunk_range(src, 0, src.len(), vec![(0, FNV0)]);
            let bal = end == Some(vec![(0, FNV0)]);
            (v, bal)
        }
    };
    let out = st.iter().map(|x| (x.0, x.1)).collect();
    *memo = Some((src.to_string(), st, bal));
    (out, bal)
}

fn chunk_diff(old: &str, st: &[(usize, u64, Levels)], new: &str) -> Option<Vec<(usize, u64, Levels)>> {
    let (p, n) = (old.as_bytes(), new.as_bytes());
    if p == n {
        return Some(st.to_vec());
    }
    let lim = p.len().min(n.len());
    let mut pre = 0;
    while pre < lim && p[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < lim - pre && p[p.len() - 1 - suf] == n[n.len() - 1 - suf] {
        suf += 1;
    }
    // Front: the chunks whose next boundary is before the changed part
    let mut j = 0;
    while j + 1 < st.len() && st[j + 1].0 <= pre {
        j += 1;
    }
    let a = st[j].0;
    // Back: the first boundary that starts after the changed part
    let chg_end = p.len() - suf;
    let m = st.iter().position(|x| x.0 > chg_end && x.0 > a);
    let (b_old, tail) = match m {
        Some(m) => (st[m].0, m),
        None => (p.len(), st.len()),
    };
    let b_new = (b_old + n.len()).checked_sub(p.len())?;
    if b_new < a {
        return None;
    }
    let (mid, end) = chunk_range(new, a, b_new, st[j].2.clone());
    if tail < st.len() {
        // (Not used unless the end of the re-cut part is at the same levels as the start of the first chunk behind it)
        if end.as_ref() != Some(&st[tail].2) {
            return None;
        }
    }
    let mut out: Vec<(usize, u64, Levels)> = st[..j].to_vec();
    out.extend(mid);
    let d = n.len() as i64 - p.len() as i64;
    for x in &st[tail..] {
        out.push(((x.0 as i64 + d) as usize, x.1, x.2.clone()));
    }
    Some(out)
}

/// A macro a chunk defines: its name, whether its rules may make an item (they have an item's keyword), and the
/// macros they call
#[derive(Clone)]
struct MacroDef {
    name: String,
    items: bool,
    calls: Vec<String>,
}

/// The keywords an item starts with (or has: `impl`, `extern`): tokens without any of them make no item
const ITEM_WORDS: [&[u8]; 14] = [b"fn", b"struct", b"enum", b"impl", b"trait", b"mod", b"use", b"static", b"const", b"macro_rules", b"macro", b"type", b"union", b"extern"];

/// Whether the tokens a..b have an item's keyword, and the macros they call (`name!`). A call through a metavariable
/// (`$m!()`, the macro its input names) may make anything: it counts as an item's keyword
fn items_and_calls(s: &[u8], toks: &[(u8, usize, usize)], a: usize, b: usize) -> (bool, Vec<String>) {
    let mut items = false;
    let mut calls: Vec<String> = vec![];
    for m in a..b.min(toks.len()) {
        if toks[m].0 != 0 {
            continue;
        }
        let w = &s[toks[m].1..toks[m].2];
        items |= ITEM_WORDS.contains(&w);
        if m + 1 < toks.len() && toks[m + 1].0 == 1 && s[toks[m + 1].1] == b'!' {
            // (`$ m!()` too: the macro parser reads `$` and the name as two tokens, with space between them or not)
            if m > 0 && toks[m - 1].0 == 1 && s[toks[m - 1].1] == b'$' {
                items = true;
                continue;
            }
            let w = String::from_utf8_lossy(w).into_owned();
            if !calls.contains(&w) {
                calls.push(w);
            }
        }
    }
    (items, calls)
}

/// A std macro that expands to an expression (not `include!`, `thread_local!`, `cfg_select!`, which can give items)
fn std_expr_macro(n: &str) -> bool {
    lexer::is_std_macro(n) && !matches!(n, "include" | "thread_local" | "global_asm" | "cfg_select" | "cfg_match")
}

/// The macros defined in a chunk
fn macro_defs(t: &str) -> Vec<MacroDef> {
    let mut v = vec![];
    if !t.contains("macro_rules") {
        return v;
    }
    let s = t.as_bytes();
    let toks = lexer::tokenize(s);
    let n = toks.len();
    let opens = |k: usize| k < n && toks[k].0 == 1 && matches!(s[toks[k].1], b'(' | b'[' | b'{');
    let mut k = 0;
    while k + 2 < n {
        if toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == b"macro_rules" && toks[k + 1].0 == 1 && s[toks[k + 1].1] == b'!' && toks[k + 2].0 == 0 {
            let name = String::from_utf8_lossy(&s[toks[k + 2].1..toks[k + 2].2]).into_owned();
            // (its rules: the brackets after the name, to the one that closes them; none read, it may make anything)
            let (items, calls) = if opens(k + 3) {
                let mut d = 0i32;
                let mut e = k + 3;
                while e < n {
                    if toks[e].0 == 1 {
                        match s[toks[e].1] {
                            b'(' | b'[' | b'{' => d += 1,
                            b')' | b']' | b'}' => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    e += 1;
                }
                items_and_calls(s, &toks, k + 4, e)
            } else {
                (true, vec![])
            };
            v.push(MacroDef { name, items, calls });
        }
        k += 1;
    }
    v
}

/// The macros whose calls make no item: no rules of theirs have an item's keyword, and they call only such macros and
/// std's that expand to expressions (to a fixed point; for a name defined twice, all its definitions). Sorted as names
fn item_free(defs: &[MacroDef], names: &[String]) -> Vec<String> {
    // (each name's definitions: whether one has an item's keyword, and all the macros they call)
    let mut by_name: FxMap<&str, (bool, Vec<&str>)> = FxMap::default();
    for d in defs {
        let e = by_name.entry(d.name.as_str()).or_default();
        e.0 |= d.items;
        e.1.extend(d.calls.iter().map(|c| c.as_str()));
    }
    let mut free: crate::fxhash::FxSet<&str> = by_name.iter().filter(|(_, e)| !e.0).map(|(n, _)| *n).collect();
    // (a round takes away those that call a macro no longer free; until a round takes none)
    loop {
        let out: Vec<&str> = free
            .iter()
            .copied()
            .filter(|n| !by_name[n].1.iter().all(|c| free.contains(c) || (!by_name.contains_key(c) && std_expr_macro(c))))
            .collect();
        if out.is_empty() {
            break;
        }
        for n in out {
            free.remove(n);
        }
    }
    names.iter().filter(|n| free.contains(n.as_str())).cloned().collect()
}

/// Skeleton: within fn bodies, removes the tokens that are not items / calls of user macros that may make one (and thread_local!) /
/// brackets / `;` (free: the macros whose calls make no item, item_free).
/// The number of newlines stays the same (newlines inside removed tokens and whitespace are kept). With the hash of what
/// it keeps as written, and whether it keeps something out of the place it was in (odd): attributes after a token that
/// ends no statement (`fo#[cfg(any())] mod m {..}`, which the full preparation does not read as the item's, but the
/// skeleton, which blanks `fo`, would), or a `mod m { .. }` where no statement starts. A chunk of such is not reused
fn skeleton(t: &str, names: &[String], free: &[String]) -> (String, u64, bool) {
    let s = t.as_bytes();
    let toks = lexer::tokenize(s);
    let n = toks.len();
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && s[toks[k].1] == c;
    let mut blank = vec![false; n];
    // (what each token is, for the hash: 0 outside the bodies, 1 in a body (kept only if a bracket or `;`), 2 in an item
    // or macro call a body keeps)
    let mut kind = vec![0u8; n];
    let mut odd = false;
    let mut depth = 0i32;
    let mut pending_fn = false;
    let mut bodies: Vec<i32> = vec![];
    // (The last token of an item or a macro call)
    let end_of = |k: usize, mac: bool| -> usize {
        if mac {
            // (k + 2 is the opening bracket)
            let mut d = 0i32;
            let mut j = k + 2;
            while j < n {
                if toks[j].0 == 1 {
                    let c = s[toks[j].1];
                    if c == b'(' || c == b'[' || c == b'{' {
                        d += 1;
                    } else if c == b')' || c == b']' || c == b'}' {
                        d -= 1;
                        if d == 0 {
                            return j;
                        }
                    }
                }
                j += 1;
            }
            return n - 1;
        }
        let mut d = 0i32;
        let mut j = k + 1;
        while j < n {
            if toks[j].0 == 1 {
                let c = s[toks[j].1];
                if c == b'(' || c == b'[' {
                    d += 1;
                } else if c == b')' || c == b']' {
                    d -= 1;
                } else if c == b';' && d == 0 {
                    return j;
                } else if c == b'{' && d == 0 {
                    let mut e = 0i32;
                    let mut m = j;
                    while m < n {
                        if is_sym(m, b'{') {
                            e += 1;
                        } else if is_sym(m, b'}') {
                            e -= 1;
                            if e == 0 {
                                return m;
                            }
                        }
                        m += 1;
                    }
                    return n - 1;
                }
            }
            j += 1;
        }
        n - 1
    };
    // ((opening token, closing token, whether something is kept) per body, and the bodies open (indices of spans))
    let mut spans: Vec<(usize, usize, bool)> = vec![];
    let mut open_at: Vec<usize> = vec![];
    // Initializer expressions of const / static (from `=` to `;`): removed except for calls of user macros
    let mut init_start: usize = 0;
    let mut init_end: usize = 0;
    let mut k = 0;
    while k < n {
        let in_body = match bodies.last() {
            Some(&d) => depth > d,
            None => false,
        };
        let in_init = k >= init_start && k < init_end;
        // (a `macro_rules!` outside the bodies is kept as written, as one in a body is: a `fn f() { .. }` in its rules is
        // no body to blank, and the calls of the changed chunks expand what it says)
        if !in_body && !in_init && toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == b"macro_rules" && k + 3 < n && is_sym(k + 1, b'!') && toks[k + 2].0 == 0 {
            if toks[k + 3].0 == 1 && matches!(s[toks[k + 3].1], b'(' | b'[' | b'{') {
                let mut d = 0i32;
                let mut j = k + 3;
                while j < n {
                    if toks[j].0 == 1 {
                        match s[toks[j].1] {
                            b'(' | b'[' | b'{' => d += 1,
                            b')' | b']' | b'}' => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    j += 1;
                }
                k = j + 1;
                continue;
            }
        }
        if (in_body || in_init) && toks[k].0 == 0 {
            let w = &s[toks[k].1..toks[k].2];
            let stmt_start = k == 0 || is_sym(k - 1, b'{') || is_sym(k - 1, b'}') || is_sym(k - 1, b';') || is_sym(k - 1, b']');
            // (what hoist_parts_at takes for an item's start, `pub fn` / `unsafe fn` / `async fn` / `extern "C" {..}` too:
            // a skeleton keeps more than it moves, never less (an `unsafe {..}` block is kept whole, which is only text))
            let item_kw = in_body
                && matches!(
                    w,
                    b"fn" | b"struct"
                        | b"enum"
                        | b"const"
                        | b"use"
                        | b"static"
                        | b"impl"
                        | b"trait"
                        | b"type"
                        | b"mod"
                        | b"union"
                        | b"macro_rules"
                        | b"pub"
                        | b"unsafe"
                        | b"async"
                        | b"extern"
                        | b"safe"
                );
            // (and a `mod m { .. }` anywhere in a body, as flatten_modules finds them: where it is not a statement's
            // start too)
            let inline_mod = in_body && w == b"mod" && k + 2 < n && toks[k + 1].0 == 0 && is_sym(k + 2, b'{');
            let mac = k + 2 < n
                && is_sym(k + 1, b'!')
                && (is_sym(k + 2, b'(') || is_sym(k + 2, b'[') || is_sym(k + 2, b'{'))
                && (w == b"thread_local" || names.iter().any(|x| x.as_bytes() == w));
            // (except a call of a macro that makes no item (item_free), with no item in its input and only calls of such
            // macros: an expression, which no other chunk's output reads. Blanked as the body's other expressions are:
            // `sum_all!(a, b, c)` calling itself took a round of expansion over the whole reduced source per argument)
            let mac = mac
                && !(free.iter().any(|x| x.as_bytes() == w) && {
                    let (items, calls) = items_and_calls(s, &toks, k + 3, end_of(k, true));
                    !items && calls.iter().all(|c| free.contains(c) || (!names.contains(c) && std_expr_macro(c)))
                });
            if (stmt_start && item_kw) || inline_mod || mac {
                // (with the attributes before it, `#[cfg(..)] fn f() {}`: what they say decides what is moved)
                let mut a = k;
                while a >= 2 && is_sym(a - 1, b']') {
                    let mut d = 0i32;
                    let mut j = a - 1;
                    loop {
                        if is_sym(j, b']') {
                            d += 1;
                        } else if is_sym(j, b'[') {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        if j == 0 {
                            break;
                        }
                        j -= 1;
                    }
                    if d != 0 || j == 0 || !is_sym(j - 1, b'#') {
                        break;
                    }
                    a = j - 1;
                }
                if a < k && a > 0 && !(is_sym(a - 1, b'{') || is_sym(a - 1, b'}') || is_sym(a - 1, b';')) {
                    odd = true;
                }
                if inline_mod && !stmt_start {
                    odd = true;
                }
                for m in a..k {
                    blank[m] = false;
                    kind[m] = 2;
                }
                // (Kept. The braces inside balance, so the depth does not change. Every body it is in keeps something:
                // not only the last one opened, which may have closed already (a `fn` type before a block,
                // `let f: fn() = || { .. };`), and not only the innermost, which an outer one would blank)
                if in_body {
                    for &o in &open_at {
                        spans[o].2 = true;
                    }
                }
                let e = end_of(k, mac && !(stmt_start && item_kw) && !inline_mod);
                for m in k..=e.min(n - 1) {
                    kind[m] = 2;
                }
                k = e + 1;
                continue;
            }
        }
        if in_init {
            blank[k] = true;
            k += 1;
            continue;
        }
        if in_body {
            kind[k] = 1;
            let keep = toks[k].0 == 1 && matches!(s[toks[k].1], b'{' | b'}' | b'(' | b')' | b'[' | b']' | b';');
            if !keep {
                blank[k] = true;
            }
        } else if toks[k].0 == 0 && depth == 0 && k >= init_end {
            // (The initializer of a const / static at the outermost level of the chunk: from after `=` to before `;`)
            let w = &s[toks[k].1..toks[k].2];
            let is_fn = k + 1 < n && toks[k + 1].0 == 0 && &s[toks[k + 1].1..toks[k + 1].2] == b"fn";
            if (w == b"const" || w == b"static") && !is_fn {
                let mut j = k + 1;
                let mut d = 0i32;
                let mut eq = n;
                while j < n {
                    if toks[j].0 == 1 {
                        let c = s[toks[j].1];
                        if c == b'(' || c == b'[' || c == b'{' {
                            d += 1;
                        } else if c == b')' || c == b']' || c == b'}' {
                            d -= 1;
                        } else if c == b';' && d <= 0 {
                            break;
                        } else if c == b'=' && d <= 0 {
                            eq = j;
                            break;
                        }
                    }
                    j += 1;
                }
                if eq < n {
                    let mut e = eq + 1;
                    let mut dd = 0i32;
                    while e < n {
                        if toks[e].0 == 1 {
                            let c = s[toks[e].1];
                            if c == b'(' || c == b'[' || c == b'{' {
                                dd += 1;
                            } else if c == b')' || c == b']' || c == b'}' {
                                dd -= 1;
                            } else if c == b';' && dd <= 0 {
                                break;
                            }
                        }
                        e += 1;
                    }
                    // (an initializer that holds an item, `const F: fn() = { fn inner() {..} inner };`, is kept as written:
                    // hoist_parts_at moves its items as a fn body's, and their names count with the others')
                    let holds_item = (eq + 1..e).any(|m| {
                        toks[m].0 == 0
                            && m > 0
                            && (is_sym(m - 1, b'{') || is_sym(m - 1, b'}') || is_sym(m - 1, b';') || is_sym(m - 1, b']'))
                            && matches!(
                                &s[toks[m].1..toks[m].2],
                                b"fn" | b"struct" | b"enum" | b"const" | b"use" | b"static" | b"impl" | b"trait" | b"type" | b"mod" | b"union" | b"macro_rules" | b"pub" | b"unsafe" | b"async" | b"extern"
                            )
                    });
                    if !holds_item {
                        init_start = eq + 1;
                        init_end = e;
                    }
                }
            }
        }
        if toks[k].0 == 0 && &s[toks[k].1..toks[k].2] == b"fn" {
            pending_fn = true;
        }
        if toks[k].0 == 1 {
            match s[toks[k].1] {
                b'{' => {
                    if pending_fn {
                        bodies.push(depth);
                        open_at.push(spans.len());
                        spans.push((k, n, false));
                        pending_fn = false;
                    }
                    depth += 1;
                }
                b'}' => {
                    depth -= 1;
                    if bodies.last() == Some(&depth) {
                        bodies.pop();
                        if let Some(o) = open_at.pop() {
                            spans[o].1 = k;
                        }
                    }
                }
                b';' => pending_fn = false,
                _ => {}
            }
        }
        k += 1;
    }
    let sig = blank_signatures(s, &toks, &kind, &blank, &spans);
    // (For a body with nothing to keep, remove everything inside, brackets included)
    for &(o, c, has) in &spans {
        if !has && c < n {
            let mut m = o + 1;
            while m < c {
                blank[m] = true;
                m += 1;
            }
        }
    }
    let mut out: Vec<u8> = Vec::with_capacity(t.len() / 2 + 16);
    let mut last = 0usize;
    let squeeze = |out: &mut Vec<u8>, b: &[u8]| {
        let nl = count_nl(b);
        if nl == 0 {
            if !b.is_empty() && !matches!(out.last(), Some(b' ') | Some(b'\n')) {
                out.push(b' ');
            }
        } else {
            for _ in 0..nl {
                out.push(b'\n');
            }
        }
    };
    // (and its hash, of what it keeps as written, without the whitespace between what a body keeps of its own (its
    // brackets and `;`): what a body's blanking leaves (spaces, and the newlines that keep the lines) is not in it,
    // so a line added to a body changes no hash. The lines are each chunk's own: no other chunk's output moves with
    // them. The items and macro calls a body keeps, and what is outside the bodies, are hashed with their whitespace:
    // a `macro_rules!`'s is in what its uses expand to)
    // (of a body's own brackets, only those around something it keeps: a statement or a block with nothing kept in
    // it, added or removed, changes no hash either. What a body keeps moves no other chunk's output but by its names
    // and the blocks it is in, which those brackets are)
    let mut holds = vec![false; n];
    let mut open: Vec<usize> = vec![];
    for k in 0..n {
        if kind[k] == 2 {
            for &o in &open {
                holds[o] = true;
            }
        } else if !blank[k] && toks[k].0 == 1 {
            match s[toks[k].1] {
                b'{' | b'(' | b'[' => open.push(k),
                b'}' | b')' | b']' => {
                    if let Some(o) = open.pop() {
                        holds[k] = holds[o];
                    }
                }
                _ => {}
            }
        }
    }
    // (a signature's tokens (sig) are blanked in the text, and hashed as kept: a changed signature changes the hash, as
    // it did when it was kept)
    let mut h = FNV0;
    for k in 0..n {
        let (a, b) = (toks[k].1, toks[k].2);
        if a < last {
            continue;
        }
        let out_blank = blank[k] || sig[k] || (k > 0 && (blank[k - 1] || sig[k - 1]));
        if out_blank {
            squeeze(&mut out, &s[last..a]);
        } else {
            out.extend_from_slice(&s[last..a]);
        }
        if !(blank[k] || (k > 0 && blank[k - 1])) {
            let prev = if k > 0 { kind[k - 1] } else { 0 };
            if !((prev == 1 || kind[k] == 1) && prev != 2 && kind[k] != 2) {
                h = fnv(h, &s[last..a]);
            }
        }
        if blank[k] || sig[k] {
            squeeze(&mut out, &s[a..b]);
            if !matches!(out.last(), Some(b' ') | Some(b'\n')) {
                out.push(b' ');
            }
        } else {
            out.extend_from_slice(&s[a..b]);
        }
        if !blank[k] && (kind[k] != 1 || holds[k]) {
            h = fnv(mix(h, 1), &s[a..b]);
        }
        last = b;
    }
    if n > 0 && (blank[n - 1] || sig[n - 1]) {
        squeeze(&mut out, &s[last..]);
    } else {
        out.extend_from_slice(&s[last..]);
    }
    if !(n > 0 && blank[n - 1]) {
        h = fnv(h, &s[last..]);
    }
    (String::from_utf8(out).unwrap_or_else(|_| t.to_string()), h, odd)
}

/// The tokens of the skeleton's fn signatures to blank: of each `fn name<..>(..) -> .. where .. {` / `;` outside the
/// bodies, its generics, what is in its parentheses and what is after them (`fn name() {`). What the preparation
/// collects from the other chunks (names, modules, `use`s, macros, tuple arities) is in no signature, and a third of
/// a skeleton's tokens were signatures. Left as written: one in a macro call or an attribute (a macro may read it),
/// one with a `!`, `#` or brace in it (a macro call, an attribute, a struct pattern, a const block), and the
/// signature of a fn whose body keeps something (a nested item is moved out with the generics around it in view)
/// spans: the bodies, (their `{`, their `}`, whether they keep something)
fn blank_signatures(s: &[u8], toks: &[(u8, usize, usize)], kind: &[u8], blank: &[bool], spans: &[(usize, usize, bool)]) -> Vec<bool> {
    let n = toks.len();
    let mut sig = vec![false; n];
    let sym = |k: usize| -> Option<u8> { (k < n && toks[k].0 == 1).then(|| s[toks[k].1]) };
    let word = |k: usize| -> &[u8] { if k < n && toks[k].0 == 0 { &s[toks[k].1..toks[k].2] } else { b"" } };
    // (the closing bracket of the one opened at k)
    let close_of = |k: usize| -> Option<usize> {
        let mut d = 0i32;
        for j in k..n {
            match sym(j) {
                Some(b'(' | b'[' | b'{') => d += 1,
                Some(b')' | b']' | b'}') => {
                    d -= 1;
                    if d == 0 {
                        return Some(j);
                    }
                }
                _ => {}
            }
        }
        None
    };
    let keeps: crate::fxhash::FxSet<usize> = spans.iter().filter(|x| x.2).map(|x| x.0).collect();
    let mut k = 0;
    while k + 2 < n {
        if kind[k] != 0 || blank[k] {
            k += 1;
            continue;
        }
        // (a macro call (`m! { .. }`, `macro_rules! m { .. }`) or an attribute: passed over whole)
        let opens_at = |j: usize| matches!(sym(j), Some(b'(' | b'[' | b'{'));
        if toks[k].0 == 0 && sym(k + 1) == Some(b'!') && (opens_at(k + 2) || (toks[k + 2].0 == 0 && opens_at(k + 3))) {
            let o = if opens_at(k + 2) { k + 2 } else { k + 3 };
            k = close_of(o).map_or(n, |c| c + 1);
            continue;
        }
        if sym(k) == Some(b'#') && (sym(k + 1) == Some(b'[') || (sym(k + 1) == Some(b'!') && sym(k + 2) == Some(b'['))) {
            let o = if sym(k + 1) == Some(b'[') { k + 1 } else { k + 2 };
            k = close_of(o).map_or(n, |c| c + 1);
            continue;
        }
        if word(k) != b"fn" || toks[k + 1].0 != 0 || !matches!(sym(k + 2), Some(b'<' | b'(')) {
            k += 1;
            continue;
        }
        // Generics: to the `>` that closes the first `<` (not the `>` of `->`)
        let mut j = k + 2;
        let mut ok = true;
        if sym(j) == Some(b'<') {
            let mut d = 0i32;
            loop {
                if j >= n {
                    ok = false;
                    break;
                }
                match sym(j) {
                    Some(b'<') => d += 1,
                    Some(b'>') if !(sym(j - 1) == Some(b'-') && toks[j - 1].2 == toks[j].1) => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    Some(b'{' | b'}' | b';' | b'!' | b'#') => {
                        ok = false;
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
            j += 1;
        }
        // The parameters, then what is after them, to the body's `{` or the `;`
        let close = if ok && sym(j) == Some(b'(') { close_of(j) } else { None };
        let mut end = None;
        if let Some(c) = close {
            let mut d = 0i32;
            let mut m = c + 1;
            while m < n {
                match sym(m) {
                    Some(b'(' | b'[') => d += 1,
                    Some(b')' | b']') => d -= 1,
                    Some(b'{' | b';') if d == 0 => {
                        end = Some(m);
                        break;
                    }
                    Some(b'{' | b'}' | b'!' | b'#') => break,
                    _ => {}
                }
                m += 1;
            }
        }
        let (Some(c), Some(e)) = (close, end) else {
            k += 1;
            continue;
        };
        let inner = (k + 2..j).chain(j + 1..c).chain(c + 1..e);
        let fine = inner.clone().all(|m| kind[m] == 0 && !blank[m] && !matches!(sym(m), Some(b'{' | b'}' | b'!' | b'#')));
        if fine && !(sym(e) == Some(b'{') && keeps.contains(&e)) {
            for m in inner {
                sig[m] = true;
            }
        }
        k = e;
    }
    sig
}

/// Splits the result of preprocessing per chunk (lines: each chunk's start line (0-based), the last is the total line count).
/// None if the number of lines does not match the input
fn split_out(p: &Prep, lines: &[usize], fix: &[(u32, u32, i32)], want: &[bool]) -> Option<Vec<Option<ChunkOut>>> {
    let total = lines[lines.len() - 1];
    let b = p.text.as_bytes();
    let mut nls: Vec<usize> = vec![];
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            nls.push(i);
        }
    }
    // (Body: as many lines as the input. If things were moved, they come after the newline that follows it)
    let (body_end, tail) = match &p.moved {
        Some(info) => {
            let sep = *nls.get(total - 1)?;
            let t = &p.text[sep + 1..p.text.len().checked_sub(1)?];
            let mut pieces: Vec<(usize, String)> = vec![];
            let mut at = 0usize;
            let tb = t.as_bytes();
            for &(line, nl) in info {
                // (The part containing nl newlines, and the separating newline that follows)
                let mut e = at;
                let mut seen = 0;
                while e < tb.len() && !(tb[e] == b'\n' && seen == nl) {
                    if tb[e] == b'\n' {
                        seen += 1;
                    }
                    e += 1;
                }
                pieces.push((line, t[at..e].to_string()));
                at = e + 1;
            }
            (sep, pieces)
        }
        None => {
            if nls.len() != total - 1 {
                return None;
            }
            (b.len(), vec![])
        }
    };
    if nls.iter().filter(|&&x| x < body_end).count() != total - 1 {
        return None;
    }
    let line_start = |l: usize| -> usize { if l == 0 { 0 } else { nls[l - 1] + 1 } };
    let mut outs: Vec<Option<ChunkOut>> = vec![];
    for j in 0..lines.len() - 1 {
        if !want[j] {
            outs.push(None);
            continue;
        }
        let (l0, l1) = (lines[j], lines[j + 1]);
        let a = line_start(l0);
        let e = if l1 >= total { body_end } else { line_start(l1) };
        let mut c = ChunkOut { text: p.text[a..e].to_string(), pieces: vec![], fix: vec![] };
        for (line, t) in &tail {
            if *line >= l0 && *line < l1 {
                // (Not used if a moved item runs past the end of the chunk)
                if *line + count_nl(t.as_bytes()) >= l1 {
                    return None;
                }
                c.pieces.push((*line - l0, t.clone()));
            }
        }
        for &(fl, col, cum) in fix {
            let l = fl as usize - 1;
            if l >= l0 && l < l1 {
                c.fix.push(((l - l0) as u32, col, cum));
            }
        }
        outs.push(Some(c));
    }
    Some(outs)
}

/// std names that zustc's prelude has under another name, because it resolves names by their last segment:
/// `Ordering` from `std::sync::atomic` (not `cmp::Ordering`) and the mpsc channel types (a program may define its
/// own `Receiver`). (module, name, prelude name); the prelude name is as long as the name, so no column moves
const STD_RENAMES: &[(&str, &str, &str)] = &[
    ("atomic", "Ordering", "__AtOrd_"),
    ("mpsc", "Sender", "__Sndr"),
    ("mpsc", "SyncSender", "__SyncSndr"),
    ("mpsc", "Receiver", "__Rcvr__"),
    // (`std::fmt::Write`, not `std::io::Write`)
    ("fmt", "Write", "__FWr"),
];

/// The prelude's name for std's `module::name` that std_renames renames (`fmt`, `Write`: `__FWr`): the trait a bound's
/// path names, for a path written either way (session's stricter_impl_bound)
pub fn std_rename_of(module: &str, name: &str) -> Option<&'static str> {
    STD_RENAMES.iter().find(|r| r.0 == module && r.1 == name).map(|r| r.2)
}

/// Rename them where they come from that module: `module::Name`, or `Name` imported by a `use` of that module (and
/// neither imported from elsewhere nor defined in the file). In the code only: a literal's or a comment's text stays
pub fn std_renames(src: &str) -> Option<String> {
    // (each module name is looked for once: this runs on every build)
    let has_atomic = src.contains("atomic");
    let has_mpsc = src.contains("mpsc");
    let has_fmt_write = src.contains("Write") && src.contains("fmt");
    if !has_atomic && !has_mpsc && !has_fmt_write {
        return None;
    }
    let b = src.as_bytes();
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = src.to_string();
    let mut changed = false;
    // (src's tokens, when a name is to be renamed or a `use` is read)
    let toks: std::cell::OnceCell<Vec<(u8, usize, usize)>> = std::cell::OnceCell::new();
    for &(m, name, to) in STD_RENAMES {
        if !(match m { "atomic" => has_atomic, "fmt" => has_fmt_write, _ => has_mpsc }) || !src.contains(name) {
            continue;
        }
        // (nothing is renamed without a `m::name` path or a `use` that may import the name from m (imports_from
        // holds for every one the scans below take): those read the whole source several times, for a program that
        // has the name only otherwise (`Write` a variant, and `fmt` a module). A macro's expansion writes the path
        // spaced, `core :: fmt :: Write`: a crate's (build_workspace) is expanded before this rename (#264))
        let (m_path, m_spaced) = (format!("{m}::"), format!("{m} :: "));
        if !src.contains(&format!("{m_path}{name}")) && !src.contains(&format!("{m_spaced}{name}")) && !imports_from(src, m, name) {
            continue;
        }
        let (mut imported, mut other) = (false, false);
        for st in src.split(';') {
            // (skip the comment lines and attributes in front: the first `use` follows the file's header)
            let mut t = st.trim_start();
            while t.starts_with("//") || t.starts_with("#") {
                let Some(nl) = t.find('\n') else { break };
                t = t[nl + 1..].trim_start();
            }
            let Some(u) = t.strip_prefix("use ").or_else(|| t.strip_prefix("pub use ")) else { continue };
            let (from_m, elsewhere) = classify_use(u, m, name);
            imported |= from_m;
            other |= elsewhere;
        }
        let defined = ["struct", "enum", "trait", "type", "union"].iter().any(|k| {
            let pat = format!("{k} {name}");
            src.match_indices(&pat).any(|(i, _)| {
                let e = i + pat.len();
                e == b.len() || !is_id(b[e])
            })
        });
        let bare = imported && !other && !defined;
        // (when some `use` imports the name from that module, each block's own `use`s decide for it: a fn's
        // `use std::fmt::Write;` is that fn's, and a module's are that module's (a crate's files, loaded as
        // `mod name { .. }`). The scan above is the answer only without one: it reads every statement that starts with
        // `use` alike, wherever it is, and misses one right after `mod name {`)
        let scoped = (!defined && imports_from(src, m, name)).then(|| use_scopes(src, toks.get_or_init(|| crate::lexer::tokenize(b)), m, name));
        let cur = std::mem::take(&mut out);
        let cb = cur.as_bytes();
        let mut i = 0;
        while let Some(k) = cur[i..].find(name) {
            let at = i + k;
            let end = at + name.len();
            let word = (at == 0 || !is_id(cb[at - 1])) && (end == cb.len() || !is_id(cb[end]));
            let after_m = cur[..at].ends_with(&m_path) || cur[..at].ends_with(&m_spaced);
            let qualified = cur[..at].ends_with("::") || cur[..at].ends_with(":: ");
            out.push_str(&cur[i..at]);
            let bare = match &scoped {
                Some(sc) => sc.bare_at(at),
                None => bare,
            };
            // (only an identifier of the code: not text inside a literal (`"fmt :: Write"`, `r#"fmt::Write"#`,
            // `b"Write"`) or a comment, which the lexer has no token in, with an import or without one. The source is
            // tokenized once, for the first name to rename: a rename keeps every length, so src's tokens are cur's)
            let code = || {
                let t = toks.get_or_init(|| crate::lexer::tokenize(b));
                let k = t.partition_point(|x| x.1 < at);
                t.get(k).is_some_and(|x| x.0 == 0 && x.1 == at)
            };
            if word && (after_m || (bare && (!qualified || cur[..at].ends_with("mpsc::") || cur[..at].ends_with("atomic::") || cur[..at].ends_with("fmt::")))) && code() {
                out.push_str(to);
                changed = true;
            } else {
                out.push_str(name);
            }
            i = end;
        }
        out.push_str(&cur[i..]);
    }
    changed.then_some(out)
}

/// The brace blocks of a source and, for each (the root first), whether its own `use`s import a name from module m
/// and whether they import it from elsewhere
struct UseScopes {
    /// (start, end, a module's body) of each block, in the order they open
    blocks: Vec<(usize, usize, bool)>,
    /// (from m, from elsewhere), by block: 0 the root, k + 1 the block k
    uses: Vec<(bool, bool)>,
    /// The blocks with a `use super::*;`: a module's body that has one sees its parent's imports
    super_globs: Vec<bool>,
}

impl UseScopes {
    /// The innermost block at byte i (0: none)
    fn block_at(&self, i: usize) -> usize {
        let mut best: Option<usize> = None;
        for (k, &(a, b, _)) in self.blocks.iter().enumerate() {
            if a < i && i < b && best.is_none_or(|j| self.blocks[j].0 < a) {
                best = Some(k);
            }
        }
        best.map_or(0, |k| k + 1)
    }

    /// The name at byte i is the one module m's: the nearest block around it whose `use`s import the name decides,
    /// up to the module it is in (a module's body does not see its parent's imports)
    fn bare_at(&self, i: usize) -> bool {
        let mut chain: Vec<usize> = self.blocks.iter().enumerate().filter(|(_, t)| t.0 < i && i < t.1).map(|(k, _)| k).collect();
        chain.sort_by_key(|&k| std::cmp::Reverse(self.blocks[k].0));
        for k in chain {
            let (from_m, other) = self.uses[k + 1];
            if from_m || other {
                return from_m && !other;
            }
            if self.blocks[k].2 && !self.super_globs[k + 1] {
                return false;
            }
        }
        let (from_m, other) = self.uses[0];
        from_m && !other
    }
}

/// What a `use` tree (the text after `use`, to its `;`) imports of a name: (from module m, from elsewhere), by each leaf's
/// own path: `std::{fmt, io::Write}` imports io's `Write`; `fmt::*` counts as m's; `Write as W` binds W, not the name,
/// and `IoWrite as Write` binds the name to another item
fn classify_use(u: &str, m: &str, name: &str) -> (bool, bool) {
    // (the tokens: identifiers, `::`, `{`, `}`, `,`, `*`)
    let mut toks: Vec<&str> = vec![];
    let b = u.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphanumeric() || c == b'_' {
            let s0 = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            toks.push(&u[s0..i]);
            continue;
        }
        if c == b':' && b.get(i + 1) == Some(&b':') {
            toks.push("::");
            i += 2;
            continue;
        }
        // (comments are whitespace: `use std::fmt::Write /* formatter */;`)
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut d = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    d += 1;
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if matches!(c, b'{' | b'}' | b',' | b'*') {
            toks.push(&u[i..i + 1]);
        }
        i += 1;
    }
    // (each leaf: its whole path, and the name it binds)
    let mut leaves: Vec<(Vec<&str>, Option<&str>)> = vec![];
    fn trees<'a>(toks: &[&'a str], mut k: usize, prefix: &[&'a str], out: &mut Vec<(Vec<&'a str>, Option<&'a str>)>) -> usize {
        loop {
            let mut path = prefix.to_vec();
            while k < toks.len() && !matches!(toks[k], "{" | "}" | "," | "*") {
                if toks[k] == "as" {
                    break;
                }
                if toks[k] != "::" {
                    path.push(toks[k]);
                }
                k += 1;
            }
            match toks.get(k).copied() {
                Some("{") => {
                    k = trees(toks, k + 1, &path, out);
                    // (past the group's `}`)
                    k += 1;
                }
                Some("*") => {
                    path.push("*");
                    out.push((path, None));
                    k += 1;
                }
                Some("as") => {
                    out.push((path, toks.get(k + 1).copied()));
                    k += 2;
                }
                _ => {
                    if path.len() > prefix.len() {
                        out.push((path, None));
                    }
                }
            }
            match toks.get(k).copied() {
                Some(",") => k += 1,
                _ => return k,
            }
        }
    }
    trees(&toks, 0, &[], &mut leaves);
    let (mut from_m, mut other) = (false, false);
    for (p, alias) in &leaves {
        // (`Write::{self}` imports `Write`)
        let p = if p.len() > 1 && p.last() == Some(&"self") { &p[..p.len() - 1] } else { &p[..] };
        let Some((&last, pre)) = p.split_last() else { continue };
        let from = pre.last() == Some(&m);
        if last == "*" {
            from_m |= from;
            continue;
        }
        // (the name a leaf binds is its alias's, if it has one: `self::IoWrite as Write` binds `Write` to another
        // item; `fmt::Write as FmtWrite` binds none of the name)
        if alias.unwrap_or(last) != name {
            continue;
        }
        if from && last == name {
            from_m = true;
        } else {
            other = true;
        }
    }
    (from_m, other)
}

/// Some `use` (from the keyword to its `;`) names module m and the name, or m's glob: a cheap look before
/// use_scopes tokenizes the source
fn imports_from(src: &str, m: &str, name: &str) -> bool {
    let word = |u: &str, w: &str| u.match_indices(w).any(|(i, _)| {
        let b = u.as_bytes();
        let id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        (i == 0 || !id(b[i - 1])) && b.get(i + w.len()).is_none_or(|&c| !id(c))
    });
    src.match_indices("use ").any(|(i, _)| {
        let u = &src[i + 4..];
        let u = &u[..u.find(';').unwrap_or(u.len())];
        word(u, m) && (word(u, name) || u.contains('*'))
    })
}

#[cold]
#[inline(never)]
fn use_scopes(src: &str, toks: &[(u8, usize, usize)], m: &str, name: &str) -> UseScopes {
    let b = src.as_bytes();
    let tk = |k: usize| &src[toks[k].1..toks[k].2];
    let sym = |k: usize, c: &str| toks[k].0 == 1 && tk(k) == c;
    // (every brace block, with whether it is a module's body: a `use` in a fn body is that body's)
    let mut blocks: Vec<(usize, usize, bool)> = vec![];
    let mut open: Vec<usize> = vec![];
    for k in 0..toks.len() {
        if sym(k, "{") {
            let is_mod = k >= 2 && toks[k - 1].0 == 0 && toks[k - 2].0 == 0 && tk(k - 2) == "mod";
            open.push(blocks.len());
            blocks.push((toks[k].1, b.len(), is_mod));
        } else if sym(k, "}") {
            if let Some(o) = open.pop() {
                blocks[o].1 = toks[k].1;
            }
        }
    }
    let mut sc = UseScopes { uses: vec![(false, false); blocks.len() + 1], super_globs: vec![false; blocks.len() + 1], blocks };
    let mut k = 0;
    while k < toks.len() {
        if toks[k].0 == 0 && tk(k) == "use" {
            let start = toks[k].2;
            let mut j = k + 1;
            while j < toks.len() && !sym(j, ";") {
                j += 1;
            }
            let tree = &src[start..toks.get(j).map_or(b.len(), |t| t.1)];
            let (from_m, elsewhere) = classify_use(tree, m, name);
            let slot = sc.block_at(toks[k].1);
            sc.uses[slot].0 |= from_m;
            sc.uses[slot].1 |= elsewhere;
            // (`use super::*;`: the module's parent's imports are its own too)
            let flat: String = tree.chars().filter(|c| !c.is_whitespace()).collect();
            if flat == "super::*" {
                sc.super_globs[slot] = true;
            }
            k = j;
        }
        k += 1;
    }
    sc
}

/// Where the items moved out of function bodies came from (Prep::moved), for the source `prep` is given: their
/// lines past the input's last one are theirs. None when nothing was moved
pub fn moved_lines(src: &str, crates: &[String], cfg: Option<&crate::cfg::CfgSet>) -> Option<Vec<(usize, usize)>> {
    let start = file_start(src);
    let src: &str = start.as_deref().unwrap_or(src);
    prep_full(src, crates, &mut vec![], cfg).ok()?.moved
}

/// Preprocessing (incremental). Column shifts are added to fix
pub fn prep(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>, cfg: Option<&crate::cfg::CfgSet>, memo: &mut PrepMemo) -> R<String> {
    memo.calls += 1;
    memo.inc = false;
    let start = file_start(src);
    let src: &str = start.as_deref().unwrap_or(src);
    if std::env::var_os("ZUSTC_NO_PREP_INC").is_some() || (memo.calls == 1 && !memo.keep_first && std::env::var_os("ZUSTC_PREP_VERIFY").is_none()) {
        return prep_full(src, crates, fix, cfg).map(marked);
    }
    let r = prep_inc(src, crates, cfg, memo);
    let (text, fx) = match r {
        Ok(Some(x)) => x,
        Ok(None) => {
            memo.forget();
            return prep_full(src, crates, fix, cfg).map(marked);
        }
        Err(e) => {
            memo.forget();
            return Err(e);
        }
    };
    if std::env::var_os("ZUSTC_PREP_VERIFY").is_some() {
        let mut f2 = vec![];
        let full = marked(prep_full(src, crates, &mut f2, cfg)?);
        if full != text || f2 != fx {
            if let Ok(p) = std::env::var("ZUSTC_PREP_VERIFY_DUMP") {
                let _ = std::fs::write(format!("{p}.inc"), &text);
                let _ = std::fs::write(format!("{p}.full"), &full);
            }
            return Err("internal: incremental preprocessing differs from the full preprocessing".into());
        }
    }
    fix.extend(fx);
    Ok(text)
}

/// Incremental preprocessing: (body, column shifts). Err if preprocessing fails, Ok(None) if it cannot be used (the caller preprocesses the whole)
fn prep_inc(src: &str, crates: &[String], cfg: Option<&crate::cfg::CfgSet>, memo: &mut PrepMemo) -> R<Option<(String, Vec<(u32, u32, i32)>)>> {
    let mut key = FNV0;
    for c in crates {
        key = fnv(mix(key, 1), c.as_bytes());
    }
    if let Some(c) = cfg {
        key = fnv(mix(key, 2), format!("{c:?}").as_bytes());
    }
    if memo.key != key {
        memo.forget();
        memo.key = key;
    }
    let log = std::env::var_os("ZUSTC_PREP_LOG").is_some();
    let (starts, balanced) = chunk_starts(src, &mut memo.chunks);
    if !balanced {
        return Ok(None);
    }
    if log {
        eprintln!("prep: {} chunks", starts.len());
    }
    let sb = src.as_bytes();
    // (Chunk start lines, and the total line count)
    let mut lines: Vec<usize> = Vec::with_capacity(starts.len() + 1);
    let mut line = 0usize;
    let mut at = 0usize;
    for &(st, _) in &starts {
        line += count_nl(&sb[at..st]);
        at = st;
        lines.push(line);
    }
    lines.push(line + count_nl(&sb[at..]) + 1);
    let nchunk = starts.len();
    let text_of = |j: usize| -> &str {
        let e = if j + 1 < nchunk { starts[j + 1].0 } else { src.len() };
        &src[starts[j].0..e]
    };
    let mut hs: Vec<u64> = Vec::with_capacity(nchunk);
    for j in 0..nchunk {
        hs.push(fnv(FNV0, text_of(j).as_bytes()));
    }
    // (a source whose delimiters do not match is preprocessed whole, which moves no item out of a body
    // (hoist_parts_at): a source of the changed chunks and the others' skeletons might match where it does not.
    // What is left of each chunk's, read once per chunk, is matched in order: a chunk can start inside an `impl`,
    // whose `{` an earlier one has)
    let mut new_delims: FxMap<u64, Option<Vec<u8>>> = FxMap::default();
    let mut open: Vec<u8> = vec![];
    for j in 0..nchunk {
        let left = match memo.delims.get(&hs[j]) {
            Some(l) => l.clone(),
            None => lexer::delims_left(text_of(j)),
        };
        let Some(l) = &left else { return Ok(None) };
        for &c in l {
            if matches!(c, b'(' | b'[' | b'{') {
                open.push(c);
            } else if open.pop() != Some(lexer::opener_of(c)) {
                return Ok(None);
            }
        }
        new_delims.insert(hs[j], left);
    }
    if !open.is_empty() {
        return Ok(None);
    }
    memo.delims = new_delims;
    // Macro names, and those whose calls make no item
    let mut names: Vec<String> = vec![];
    let mut defs: Vec<MacroDef> = vec![];
    let mut new_names: FxMap<u64, Vec<MacroDef>> = FxMap::default();
    for j in 0..nchunk {
        let v = match memo.names.get(&hs[j]) {
            Some(v) => v.clone(),
            None => macro_defs(text_of(j)),
        };
        for x in &v {
            if !names.contains(&x.name) {
                names.push(x.name.clone());
            }
            defs.push(x.clone());
        }
        new_names.insert(hs[j], v);
    }
    memo.names = new_names;
    names.sort();
    let free = item_free(&defs, &names);
    let mut nh = FNV0;
    for x in &names {
        nh = fnv(mix(nh, 3), x.as_bytes());
    }
    for x in &free {
        nh = fnv(mix(nh, 4), x.as_bytes());
    }
    // Skeletons, and the hash of the whole
    let mut fp = mix(key, nh);
    let mut new_skel: FxMap<(u64, u64), (String, u64, bool)> = FxMap::default();
    for j in 0..nchunk {
        let k = (hs[j], nh);
        let e = match memo.skel.remove(&k) {
            Some(e) => e,
            None => match new_skel.get(&k) {
                Some(e) => (e.0.clone(), e.1, e.2),
                None => {
                    skeleton(text_of(j), &names, &free)
                }
            },
        };
        if e.2 {
            return Ok(None);
        }
        fp = mix(mix(fp, starts[j].1), e.1);
        new_skel.insert(k, e);
    }
    memo.skel = new_skel;
    if log {
        eprintln!("prep: same fp: {}", memo.fp == fp);
    }
    // (Keys of chunk results)
    let mut occ: FxMap<(u64, u64), u32> = FxMap::default();
    let mut keys: Vec<(u64, u64, u32)> = Vec::with_capacity(nchunk);
    for j in 0..nchunk {
        let c = occ.entry((hs[j], starts[j].1)).or_insert(0);
        keys.push((hs[j], starts[j].1, *c));
        *c += 1;
    }
    let mut outs: Vec<Option<ChunkOut>> = (0..nchunk).map(|_| None).collect();
    if memo.fp == fp && !memo.out.is_empty() {
        memo.inc = true;
        let mut missing = vec![false; nchunk];
        let mut any = false;
        for j in 0..nchunk {
            match memo.out.remove(&keys[j]) {
                Some(o) => outs[j] = Some(o),
                None => {
                    missing[j] = true;
                    any = true;
                }
            }
        }
        if any {
            // Preprocess a source where only the changed chunks stay as they are and the rest is skeleton, and cut out the changed chunks' lines
            let mut red = String::with_capacity(src.len() / 2);
            for j in 0..nchunk {
                if missing[j] {
                    red.push_str(text_of(j));
                } else {
                    match memo.skel.get(&(hs[j], nh)) {
                        Some(e) => red.push_str(&e.0),
                        None => return Ok(None),
                    }
                }
            }
            if log {
                let nm = missing.iter().filter(|&&x| x).count();
                eprintln!("prep: {nm} changed, reduced {} of {} bytes", red.len(), src.len());
            }
            let mut f = vec![];
            let p = match prep_full(&red, crates, &mut f, cfg) {
                Ok(p) => p,
                Err(_) => return Ok(None),
            };
            let Some(got) = split_out(&p, &lines, &f, &missing) else { return Ok(None) };
            for (j, g) in got.into_iter().enumerate() {
                if let Some(g) = g {
                    outs[j] = Some(g);
                }
            }
        }
    } else {
        let mut f = vec![];
        let p = prep_full(src, crates, &mut f, cfg)?;
        let all = vec![true; nchunk];
        let Some(got) = split_out(&p, &lines, &f, &all) else { return Ok(None) };
        outs = got;
        memo.fp = fp;
    }
    // Assemble
    let mut text = String::with_capacity(src.len() + 64);
    let mut pieces: Vec<(usize, &str)> = vec![];
    let mut fx: Vec<(u32, u32, i32)> = vec![];
    for j in 0..nchunk {
        let Some(o) = &outs[j] else { return Ok(None) };
        text.push_str(&o.text);
        for (l, p) in &o.pieces {
            pieces.push((lines[j] + l, p));
        }
        for &(l, c, d) in &o.fix {
            fx.push((l + lines[j] as u32 + 1, c, d));
        }
    }
    if !pieces.is_empty() {
        for (l, p) in &pieces {
            text.push('\n');
            text.push_str(&hoist_mark(*l));
            text.push_str(p);
        }
        text.push('\n');
    }
    memo.out = FxMap::default();
    for (j, o) in outs.into_iter().enumerate() {
        if let Some(o) = o {
            memo.out.insert(keys[j], o);
        }
    }
    Ok(Some((text, fx)))
}

#[cfg(test)]
mod tests {
    /// std_renames renames the code's paths, spaced or not, and no literal's or comment's text, with a `use` of the
    /// module or without one (`println!("fmt :: Write")` printed `fmt :: __FWr`)
    #[test]
    fn std_renames_leave_text() {
        let code = "impl core :: fmt :: Write for X {}\nfn f(_: std::sync::mpsc::Sender<u8>, _: std::sync::atomic::Ordering) {}\n";
        let want = "impl core :: fmt :: __FWr for X {}\nfn f(_: std::sync::mpsc::__Sndr<u8>, _: std::sync::atomic::__AtOrd_) {}\n";
        let text = "// fmt::Write, fmt :: Write\n/* mpsc::Sender */\nfn main() {\n    println!(\"fmt :: Write {} {}\", r#\"fmt::Write\"#, \"mpsc :: Sender\");\n    let _ = (b\"atomic::Ordering\", 'W', \"Write Sender\");\n}\n";
        for (head, head_to) in [("", ""), ("use std::fmt::Write;\nuse std::sync::mpsc::Sender;\n", "use std::fmt::__FWr;\nuse std::sync::mpsc::__Sndr;\n")] {
            let got = super::std_renames(&format!("{head}{code}{text}"));
            assert_eq!(got.as_deref(), Some(format!("{head_to}{want}{text}").as_str()), "{head:?}");
        }
        assert_eq!(super::std_renames(text), None);
    }
}
