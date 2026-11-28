//! コンパイルのセッション ([`Session`]): ソース → (lexer / macros / prelude) → ast_summary → Program →
//! acg (自前の構文木のコード生成) → 機械語 (Cranelift か minclif) → ELF の .o → リンク。
//! セッションはキャッシュ (メモリと、ディスクの `<出力>.cache`) と、編集をまたいで覚えるもの (前処理の差分・check の記録) を
//! 持ち、同じセッションで何度でも build / check できる (zust-analyzer はドキュメントごとに 1 つ持つ)。数の多い部分 (アイテムの
//! 要約、check と機械語の wave) は fork した子に分ける。解析と check はここ、機械語・オブジェクト・リンク・ワークスペースは
//! feature `build` の session/build.rs。外部クレートに依存しないモジュールだけで書く (zustc 自身でもコンパイルできる)。
//! (2026-11-25 までの名前は driver2: 2 つ目のドライバだったため。それより前のレポートはその名前で書いている)

use crate::cfg::{attr_entries, cfg_pred, lit_end, malformed_cfg, str_lit, str_value};
use std::sync::Mutex;

use crate::acg::{self, Blob, ItemFn};
use crate::key::{Key, KeyHasher};
use crate::fxhash::{FxMap, FxSet};
use crate::lexer::{self, ItemKind, SrcItem};
use crate::parse::{Block, Expr, Pat, Stmt};
use crate::front::{FrontMemo, with_prelude_in};
use crate::prof::{Timer, now_ns};
use crate::program::{Insts, Program, TyEnv};
use crate::types::{Summary, Ty};

pub type R<T> = Result<T, String>;

/// ビルド (機械語・オブジェクト・リンク・ワークスペース)。feature `build` (既定) のときだけ
#[cfg(feature = "build")]
mod build;
mod dead;
mod dyncompat;
mod parens;
#[cfg(feature = "build")]
pub use build::*;

/// check の記録 (LSP の編集をまたいで): 宣言の指紋と、検査に通った関数・インスタンス (名前 → 記録)
pub struct ChkMemo {
    fp: u64,
    /// the lint levels it was checked with (lint_bits): `#![allow(overflowing_literals)]` taken out of a body leaves
    /// iface_fp as it was
    lint: u16,
    ents: FxMap<String, ChkEnt>,
}

#[derive(Clone)]
struct ChkEnt {
    /// 本体の関数の名前と、そのアイテムのハッシュ
    body: String,
    body_hash: Key,
    /// 要求したインスタンスの名前
    children: Vec<String>,
    /// インスタンスなら、自分を要求し直すための Blob (自分への再配置だけ)
    me: Vec<u8>,
    /// 検査に通ったか (通らなかったものは毎回検査し直す)
    ok: bool,
}


/// 解析の結果
pub struct Analysis {
    /// An error the type check precedes: reported only where it finds none (E0451, rustc's privacy pass)
    pub late_err: Option<String>,
    /// 宣言の指紋: fn アイテムは要約 (シグネチャ・クロージャ)、ほかのアイテムはテキストのハッシュを並べたもの。
    /// fn の本体だけを変えた編集では変わらない (check の記憶 ChkMemo を使ってよいかの印)
    pub iface_fp: u64,
    pub src: String,
    pub items: Vec<SrcItem>,
    pub prog: Program,
    /// Each fn's references to fns: the groups of the names it refers to (Program::ref_groups), sorted
    pub refs: Vec<Vec<u32>>,
    /// Each group's part of a cache key (Session::group_key), once per analysis: of the keys that keep every fn and of
    /// those that leave out the fns before the prelude's (the program's own), by group (0: not yet), and of a closed
    /// crate's (by group and universe)
    pub group_keys: Mutex<(Vec<[Key; 2]>, FxMap<(u32, u64), Key>)>,
    /// 関数ごとの、名前で現れる struct と const / static (キャッシュのキー用)
    pub srefs: Vec<Vec<u32>>,
    pub crefs: Vec<Vec<u32>>,
    /// 要約を作り直した (キャッシュになかった) アイテムの数
    pub n_parsed: usize,
    /// アイテムごとの、依存するクレートのものか (CrateCtx)
    pub ext_items: Vec<bool>,
    /// `-O` で展開してよい (prelude の小さい) 関数の本体
    pub inl: acg::Inline,
    /// 機械語にするバックエンド (None なら minclif)
    pub cl: Option<Backend>,
    /// prelude の先頭の行 (結合したソースの行)
    pub prelude_line0: usize,
    /// 読んだ関数の本体 (関数 id → 構文木)
    pub asts: Mutex<FxMap<u32, ItemFn>>,
    /// 囲む関数 (アイテム番号) ごとのクロージャの式 (クロージャの本体を引くたびに囲む関数を読み直さない)
    /// item → its closures, and whether each is written inside `unsafe { }`
    pub encl: Mutex<FxMap<u32, Vec<(Expr, bool)>>>,
    /// 関数ごとのシグネチャのハッシュ (0 はまだ: キーを作るたびに参照先のシグネチャをたどり直さない)
    pub sig_keys: Mutex<Vec<Key>>,
    /// 依存するクレートのテキストの範囲と、そこから見えるクレート (CrateCtx::dep_ranges)
    pub dep_ranges: Vec<(u32, u32, Vec<usize>, u32)>,
    /// The definitions of the program's associated types (not the prelude's), for the functions' cache keys: all of
    /// them, and those of each dependency crate (by dep_ranges' index). A function's types and region check read an
    /// associated type's value (`<() as Foo>::Out`, its lifetimes too), which its body and signatures do not show
    pub assoc_keys: (Key, Vec<Key>),
}

/// クレートの印 (panic の場所の行の上位ビット。ライブラリの panic の表の 8 byte 目): 1..=4000
/// `L:C: ..` with L a tagged line (`tag << 20 | line in the crate`, line_base_for): the line in the combined source
/// (the `(at L:C)` of a second place too)
fn untag_lines(e: &str, shift: u32) -> String {
    let fix = |n: u64| if n >= 1 << 20 { (n & ((1 << 20) - 1)) + shift as u64 } else { n };
    let mut out = String::with_capacity(e.len());
    let b = e.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // (a number followed by `:` and a digit, at the start or after `at ` / ` (at `)
        let at_start = i == 0 || e[..i].ends_with("at ");
        if at_start && b[i].is_ascii_digit() {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < b.len() && b[j] == b':' && b[j + 1].is_ascii_digit() {
                let n: u64 = e[i..j].parse().unwrap_or(0);
                out.push_str(&fix(n).to_string());
                i = j;
                continue;
            }
        }
        let ch = e[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

pub fn crate_tag(name: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    1 + h % 4000
}

/// prelude の中の panic の場所の行の基準 (結合したソースの行によらない: prelude の関数の機械語をクレートの間で使い回す)。
/// 場所の表では、この行から後ろは prelude のファイル
pub const PRELUDE_LINE: u32 = 49152;

/// The first line of the fns that `zustc check` adds to read the program's constants (const_check_item): past every
/// line of a program and its prelude, and under 2^20 (an error's line is kept in 20 bits)
pub const CONST_CHECK_LINE: usize = 1_000_000;

/// `s` with its comments (`//..` to the line's end, `/* .. */`) as spaces, its newlines and length kept; strings and chars
/// skipped
/// Where the raw string at b[i] ends (past it), b[i] its `r` (`r#"a "{" b"#`, `br"\"`); None for anything else
fn raw_str_end(b: &[u8], i: usize) -> Option<usize> {
    let ident = |k: usize| b[k].is_ascii_alphanumeric() || b[k] == b'_';
    let alone = |k: usize| k == 0 || !ident(k - 1);
    if b[i] != b'r' || !(alone(i) || (b[i - 1] == b'b' && alone(i - 1))) {
        return None;
    }
    let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
    if b.get(i + 1 + hashes) != Some(&b'"') {
        return None;
    }
    let close = [&b"\""[..], &vec![b'#'; hashes]].concat();
    let from = i + 2 + hashes;
    Some(b[from..].windows(close.len()).position(|w| w == close).map_or(b.len(), |p| from + p + close.len()))
}

fn blank_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        if let Some(e) = raw_str_end(b, i) {
            i = e;
            continue;
        }
        // (a char literal (`b'"'`), not a lifetime (`'a`): its quote starts no string)
        if b[i] == b'\'' {
            if let Some(e) = literal_end(b, i) {
                i = e;
                continue;
            }
        }
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
                continue;
            }
            // (`/* .. /* .. */ .. */` nests)
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                while i < b.len() {
                    if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                    } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        if b[i] != b'\n' {
                            out[i] = b' ';
                        }
                        i += 1;
                    }
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// The end of the `use` declaration whose keyword ends at sb[b]: its `;`, not one in a comment (`use a::{ /* ; */ b };`),
/// else stop
fn use_end(sb: &[u8], b: usize, stop: usize) -> usize {
    let mut i = b;
    while i < stop {
        if sb[i] == b'/' && sb.get(i + 1) == Some(&b'/') {
            i = sb[i..stop].iter().position(|&c| c == b'\n').map_or(stop, |k| i + k);
        } else if sb[i] == b'/' && sb.get(i + 1) == Some(&b'*') {
            // (block comments nest: `/* a /* b */ ; */`)
            let mut depth = 1;
            i += 2;
            while i < stop && depth > 0 {
                if sb[i] == b'/' && sb.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if sb[i] == b'*' && sb.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if sb[i] == b';' {
            return i;
        } else {
            i += 1;
        }
    }
    stop
}

/// The fmt traits the items' `use` declarations import (fn bodies' too), by each declaration's use tree, its comments
/// left out (use_tree_fmt): 1 Display, 2 Debug, then 4 LowerHex, 8 UpperHex, 16 Binary, 32 Octal, 64 LowerExp, 128
/// UpperExp; a glob of a `fmt` path all. Only a name under a `fmt` path: a trait
/// of the program's named Debug is not std's (Program::fmt_imports)
fn fmt_imports(src: &str, items: &[SrcItem]) -> u8 {
    let sb = src.as_bytes();
    let mut mask = 0u8;
    for it in items {
        for &(a, b) in &it.idents {
            if b - a != 3 || sb.get(a as usize..b as usize) != Some(b"use") {
                continue;
            }
            // (the declaration, up to its `;` (not one in a comment), without its comments)
            let end = use_end(sb, b as usize, it.end.min(sb.len()));
            let text = blank_comments(src.get(b as usize..end).unwrap_or(""));
            if !text.contains("fmt") {
                continue;
            }
            let toks = use_tokens(&text);
            let mut k = 0;
            use_tree_fmt(&toks, &mut k, &mut vec![], &mut mask, 0);
        }
    }
    mask
}

/// A use declaration's tokens: names, `::`, `{`, `}`, `,`, `*` (the rest is left out)
fn use_tokens(t: &str) -> Vec<&str> {
    let b = t.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphanumeric() || c == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(&t[s..i]);
            continue;
        }
        if c == b':' && b.get(i + 1) == Some(&b':') {
            out.push("::");
            i += 2;
            continue;
        }
        if matches!(c, b'{' | b'}' | b',' | b'*') {
            out.push(&t[i..i + 1]);
        }
        i += 1;
    }
    out
}

/// One use tree at toks[*k] under the path `prefix`: a leaf `fmt::Display` / `fmt::Debug` (also aliased, `Debug as D`
/// or `as _`: the trait is in scope under any name; an alias that is spelled `Debug`, `io::Error as Debug`, is no trait
/// of fmt) sets its bit, and a glob whose path ends in `fmt` (`fmt::*`, `fmt::{*, ..}`) both. A glob elsewhere
/// (`ops::*`) none
fn use_tree_fmt(toks: &[&str], k: &mut usize, prefix: &mut Vec<String>, mask: &mut u8, depth: u32) {
    let base = prefix.len();
    if toks.get(*k) == Some(&"::") {
        *k += 1;
    }
    while let Some(&t) = toks.get(*k) {
        match t {
            "{" if depth < 16 => {
                *k += 1;
                while toks.get(*k).is_some_and(|&t| t != "}") {
                    use_tree_fmt(toks, k, prefix, mask, depth + 1);
                    if toks.get(*k) == Some(&",") {
                        *k += 1;
                    } else {
                        break;
                    }
                }
                if toks.get(*k) == Some(&"}") {
                    *k += 1;
                }
                break;
            }
            "*" => {
                *k += 1;
                if prefix.last().is_some_and(|p| p == "fmt") {
                    *mask = !0;
                }
                break;
            }
            "::" | "," | "}" | "{" => break,
            name => {
                *k += 1;
                if toks.get(*k) == Some(&"::") {
                    prefix.push(name.to_string());
                    *k += 1;
                    continue;
                }
                // (a leaf, by its own name: what follows `as` is its alias)
                if toks.get(*k) == Some(&"as") {
                    *k += 2;
                }
                if prefix.last().is_some_and(|p| p == "fmt") {
                    match name {
                        "Display" => *mask |= 1,
                        "Debug" => *mask |= 2,
                        "LowerHex" => *mask |= 4,
                        "UpperHex" => *mask |= 8,
                        "Binary" => *mask |= 16,
                        "Octal" => *mask |= 32,
                        "LowerExp" => *mask |= 64,
                        "UpperExp" => *mask |= 128,
                        _ => {}
                    }
                }
                break;
            }
        }
    }
    prefix.truncate(base);
}

/// Where the string or char literal at b[i] ends (past it): `"..."`, `b'{'`, `'\n'`; None for anything else (a
/// lifetime `'a`)
fn literal_end(b: &[u8], i: usize) -> Option<usize> {
    if let Some(e) = raw_str_end(b, i) {
        return Some(e);
    }
    match b[i] {
        b'"' => {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            Some((j + 1).min(b.len()))
        }
        b'\'' => {
            // (a char: one char or an escape, then `'`)
            let j = if b.get(i + 1) == Some(&b'\\') { b[i + 2..].iter().position(|&c| c == b'\'').map(|p| i + 2 + p)? } else {
                let w = std::str::from_utf8(&b[i + 1..(i + 5).min(b.len())]).ok().or_else(|| std::str::from_utf8(&b[i + 1..(i + 2).min(b.len())]).ok())?.chars().next()?.len_utf8();
                (b.get(i + 1 + w) == Some(&b'\'')).then_some(i + 1 + w)?
            };
            Some(j + 1)
        }
        _ => None,
    }
}

/// The names of those fns, with the constant's place in their order after it
pub const CONST_CHECK_FN: &str = "__zust_const_";

/// `zustc check`: a fn for each constant and static of the program that binds its initializer to its type
/// (`fn __zust_const_0() { let _zc: &'static str = ("x"); }`), so that the initializer is checked against the type where
/// it is defined, used or not, as rustc checks every constant, with the inference of a fn's body: (the fns'
/// text, the line of each initializer). None without one. Not one whose initializer defines items (a fn body would
/// hoist them again). An enum's discriminants likewise, of its `#[repr(..)]` integer type or isize
fn const_check_item(src: &str, items: &[SrcItem]) -> Option<(String, Vec<usize>, String)> {
    // (each probe's type and initializer, named at the end)
    let mut probes: Vec<(String, String)> = vec![];
    let mut lines = vec![];
    // (an item hoisted out of a fn body: the line it was written on, prep::HOIST_MARK; looked for at the first)
    let marks: std::cell::OnceCell<Vec<(usize, usize)>> = std::cell::OnceCell::new();
    let written = |l: usize| crate::prep::hoisted_at(marks.get_or_init(|| crate::prep::hoist_marks(src)), l).unwrap_or(l);
    // (nor one with a `return`, which would return from the fn: E0572 in a constant)
    let items_inside = ["fn ", "struct ", "enum ", "union ", "impl ", "trait ", "mod ", "use ", "static ", "const ", "macro_rules", "type ", "return"];
    // (its comments blanked; `*const T` is a type, no item)
    // (nor one that casts to a raw pointer of an inferred type, `as *mut _`, whose inference zustc's casts may get wrong)
    let defines_items = |e: &str| {
        let b = blank_comments(e).replace("*const ", "*");
        let tight: String = b.chars().filter(|c| !c.is_whitespace()).collect();
        let infer_cast = ["as*_", "as*mut_"].iter().any(|k| tight.match_indices(k).any(|(i, _)| !tight[i + k.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')));
        infer_cast || items_inside.iter().any(|k| b.contains(k))
    };
    for it in items {
        if !matches!(it.kind, ItemKind::Normal) {
            continue;
        }
        let text = &src[it.start..it.end];
        // (past the item's attributes and its visibility)
        let mut t = text.trim_start();
        while t.starts_with("#[") {
            let o = 2;
            t = t[(o + attr_end(&t[o..])).min(t.len())..].trim_start();
        }
        if let Some(r) = t.strip_prefix("pub") {
            t = r.trim_start();
            if t.starts_with('(') {
                t = t.find(')').map_or("", |e| t[e + 1..].trim_start());
            }
        }
        // (an enum's discriminants: each of its integer type, `#[repr(u8)]` or isize)
        if let Some(r) = t.strip_prefix("enum ") {
            let head: String = text[..text.len() - t.len()].chars().filter(|c| !c.is_whitespace()).collect();
            let ints = ["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize"];
            // (the attribute's: not `repr(` in a doc's text)
            let repr = head.find("#[repr(").and_then(|k| {
                let args = &head[k + 7..];
                let args = &args[..args.find(')').unwrap_or(args.len())];
                args.split(',').map(str::trim).find(|a| ints.contains(a))
            });
            let Some(open) = r.find('{') else { continue };
            let body_at = text.len() - r.len() + open + 1;
            // (its comments blanked, lengths kept: `//~ NOTE \`3\` (overflowed)` is no part of a discriminant)
            let body = &blank_comments(&text[body_at..]);
            let (mut depth, mut start) = (0i32, 0usize);
            let bb = body.as_bytes();
            let mut parts = vec![];
            let mut skip_to = 0;
            for (i, &c) in bb.iter().enumerate() {
                if i < skip_to {
                    continue;
                }
                // (a string or char literal's brackets are none: `b'{'`)
                if let Some(e) = literal_end(bb, i) {
                    skip_to = e;
                    continue;
                }
                match c {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' if depth == 0 => {
                        parts.push((start, i));
                        break;
                    }
                    b')' | b']' | b'}' => depth -= 1,
                    b',' if depth == 0 => {
                        parts.push((start, i));
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            for (a, z) in parts {
                let v = &body[a..z];
                // (`V = e`: a `=` outside brackets, not `==` / `=>` / `<=` ..)
                let vb = v.as_bytes();
                let mut d = 0i32;
                let mut lit_to = 0;
                let eq = (0..vb.len()).find(|&i| {
                    if i < lit_to {
                        return false;
                    }
                    if let Some(e) = literal_end(vb, i) {
                        lit_to = e;
                        return false;
                    }
                    match vb[i] {
                        b'(' | b'[' | b'{' => d += 1,
                        b')' | b']' | b'}' => d -= 1,
                        _ => {}
                    }
                    d == 0 && vb[i] == b'=' && vb.get(i + 1).is_none_or(|&n| n != b'=' && n != b'>') && (i == 0 || !b"=!<>".contains(&vb[i - 1]))
                });
                let Some(eq) = eq else { continue };
                let e = v[eq + 1..].trim();
                if e.is_empty() || defines_items(e) {
                    continue;
                }
                let at = body_at + a + eq + 1;
                let skip = { let b = blank_comments(&text[at..]); b.len() - b.trim_start().len() };
                probes.push((repr.unwrap_or("isize").to_string(), e.to_string()));
                lines.push(written(it.line + text[..at + skip].matches('\n').count()));
            }
            continue;
        }
        let Some(r) = t.strip_prefix("const ").or_else(|| t.strip_prefix("static ")) else { continue };
        let r = r.trim_start();
        let r = r.strip_prefix("mut ").map_or(r, str::trim_start);
        let name: &str = &r[..r.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(r.len())];
        let Some(ty_from) = r[name.len()..].trim_start().strip_prefix(':') else { continue };
        if name.is_empty() || name == "_" || name == "fn" {
            continue;
        }
        // (the type, to the `=` outside its brackets (`&dyn Iterator<Item = u8>`); then the initializer to the `;`)
        let b = ty_from.as_bytes();
        let mut depth = 0i32;
        let mut eq = None;
        for (i, &c) in b.iter().enumerate() {
            match c {
                b'<' | b'(' | b'[' => depth += 1,
                b'>' if i > 0 && b[i - 1] == b'-' => {}
                b'>' | b')' | b']' => depth -= 1,
                b'=' if depth == 0 => {
                    eq = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(eq) = eq else { continue };
        let ty = ty_from[..eq].trim();
        let init = ty_from[eq + 1..].trim_end().trim_end_matches(';');
        if ty.is_empty() || init.trim().is_empty() || defines_items(init) {
            continue;
        }
        // (a constant's references are 'static: `&str` is `&'static str`)
        let mut ty_s = String::new();
        let tb: Vec<char> = ty.chars().collect();
        // (not a `&` of an array's length, `[u8; 3 & 1]`: after a `;` in brackets)
        let mut lens: Vec<bool> = vec![];
        for (i, &c) in tb.iter().enumerate() {
            match c {
                '[' => lens.push(false),
                ']' => {
                    lens.pop();
                }
                ';' => {
                    if let Some(l) = lens.last_mut() {
                        *l = true;
                    }
                }
                _ => {}
            }
            ty_s.push(c);
            if c == '&' && !lens.last().copied().unwrap_or(false) {
                let next = tb[i + 1..].iter().find(|c| !c.is_whitespace());
                if !next.is_some_and(|&n| n == '\'') {
                    ty_s.push_str("'static ");
                }
            }
        }
        // (the line of its initializer)
        let at = text.len() - ty_from.len() + eq + 1;
        let skip = { let b = blank_comments(&text[at..]); b.len() - b.trim_start().len() };
        // (a `let` of the type: the coercions of a `let`, as a constant's initializer has)
        probes.push((ty_s, init.to_string()));
        lines.push(written(it.line + text[..at + skip].matches('\n').count()));
    }
    if lines.is_empty() {
        return None;
    }
    // (the probes' names are theirs alone: `__zust_const_`, or `__zust_const1_` .. when the program writes that)
    let name = (0..).map(|n| if n == 0 { CONST_CHECK_FN.to_string() } else { format!("__zust_const{n}_") }).find(|p| !items.iter().any(|it| src[it.start..it.end].contains(p.as_str())))?;
    let mut body_out = String::new();
    for (k, (ty, init)) in probes.iter().enumerate() {
        body_out.push_str(&format!("#[allow(dead_code, unused)] fn {name}{k}() {{\n    let _zc: {ty} = (\n{init}\n    );\n}}\n"));
    }
    Some((body_out, lines, name))
}

/// 検査だけの印のキャッシュのキー (ビルドの Blob のキーと混ざらないように)
const CHECK_SALT: Key = 0x5eed_c4ec_0000_0000_0000_0000_0000_0001;

impl Analysis {
    /// The fns fn id refers to (the fns of the names it refers to), ascending and once each
    pub fn ref_fns(&self, id: usize) -> Vec<u32> {
        let mut v: Vec<u32> = self.refs[id].iter().flat_map(|&g| self.prog.ref_groups[g as usize].iter().copied()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// アイテム ii が依存するクレートのものなら、その dep_ranges の番号
    /// シグネチャのハッシュ (覚えておく)
    pub fn sig_key(&self, id: u32) -> Key {
        let mut ks = self.sig_keys.lock().unwrap();
        if ks.is_empty() {
            ks.resize(self.prog.names.len(), 0);
        }
        let k = ks[id as usize];
        if k != 0 {
            return k;
        }
        let mut h = KeyHasher::new();
        self.prog.hash_sig(id, &mut h);
        let k = h.finish() | 1;
        ks[id as usize] = k;
        k
    }

    /// Analysis::assoc_keys: of each associated type the program defines, its impl member's text (which holds its
    /// impl's header's), combined independently of their order
    fn assoc_keys(&self) -> (Key, Vec<Key>) {
        let mut all: Key = 0;
        let mut per = vec![0; self.dep_ranges.len()];
        for (i, &ii) in self.prog.assoc_items.iter().enumerate() {
            if ii >= self.prog.prelude_from {
                continue;
            }
            let mut h = KeyHasher::new();
            h.add(&self.prog.assocs[i].name);
            h.add(&self.items[ii as usize].hash);
            // (and its impl's header: the `'d: 'a` it declares are required where a projection selects it)
            if let ItemKind::ImplMember(hd) = self.items[ii as usize].kind {
                h.add(&self.items[hd as usize].hash);
            }
            let k = h.finish();
            all ^= k;
            if let Some(c) = self.crate_of_item(ii as usize) {
                per[c] ^= k;
            }
        }
        (all, per)
    }

    pub fn crate_of_item(&self, ii: usize) -> Option<usize> {
        if ii >= self.prog.prelude_from as usize || !self.ext_items.get(ii).copied().unwrap_or(false) {
            return None;
        }
        let l = self.items[ii].line as u32;
        self.dep_ranges.iter().position(|r| r.0 <= l && l < r.1)
    }

    /// 閉じた関数 (インスタンス): 本体が prelude か依存するクレートにあり、型引数にこのクレートの型を含まないもの。
    /// 本体が呼びうるのは、本体のクレートと型引数の型のクレートから見えるクレート (と prelude) のものだけなので、
    /// キャッシュのキーはそれらだけで決まる (使う側によらない)。返り値は見えるクレート (dep_ranges の番号ごと)
    pub fn closed_univ(&self, insts: &Insts, id: u32) -> Option<Vec<bool>> {
        let body = insts.body_of(id);
        if self.prog.closures.contains_key(&body) {
            return None;
        }
        let ii = self.prog.item_of[body as usize]? as usize;
        let mut u = vec![false; self.dep_ranges.len()];
        if ii < self.prog.prelude_from as usize {
            let r = self.crate_of_item(ii)?;
            for &x in &self.dep_ranges[r].2 {
                u[x] = true;
            }
        }
        if let Some(inst) = insts.get(id) {
            let mut stack: Vec<Ty> = inst.args.clone();
            while let Some(t) = stack.pop() {
                match t {
                    Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => stack.push(*i),
                    Ty::Struct(sid) => {
                        let b = self.prog.base_of(sid);
                        if let Some(&sii) = self.prog.struct_item.get(b as usize) {
                            let sii = sii as usize;
                            if sii < self.prog.prelude_from as usize {
                                let c = self.crate_of_item(sii)?;
                                for &x in &self.dep_ranges[c].2 {
                                    u[x] = true;
                                }
                            }
                        }
                        stack.extend(self.prog.structs[sid as usize].args.iter().copied());
                    }
                    Ty::Fn(f) => {
                        stack.extend(f.params.iter().copied());
                        stack.push(f.ret);
                    }
                    Ty::Dyn(_) | Ty::Assoc(_) | Ty::Param(_) => return None,
                    _ => {}
                }
            }
        }
        Some(u)
    }

    /// 関数 (インスタンス) id の panic の場所の行の基準。閉じた依存するクレートの関数は、そのクレートのテキストの先頭からの
    /// 行 (使う側の結合したソースの中の位置によらない: .o を書くときに位置を足す)
    pub fn line_base_for(&self, insts: &Insts, id: u32) -> Option<u32> {
        let body = insts.body_of(id);
        let ii = self.prog.item_of[body as usize]? as usize;
        if let Some(r) = self.crate_of_item(ii) {
            if self.closed_univ(insts, id).is_some() {
                return Some((self.dep_ranges[r].3 << 20) | (self.items[ii].line as u32 - self.dep_ranges[r].0));
            }
        }
        Some(self.line_base(ii))
    }

    /// .o に書くときに、関数 id の panic の場所の行に足す数 (line_base_for で依存するクレートの中の行にしたもの)
    pub fn line_shift(&self, insts: &Insts, id: u32) -> u32 {
        let body = insts.body_of(id);
        let Some(ii) = self.prog.item_of.get(body as usize).copied().flatten() else { return 0 };
        match self.crate_of_item(ii as usize) {
            Some(r) if self.closed_univ(insts, id).is_some() => self.dep_ranges[r].0 - 1,
            _ => 0,
        }
    }

    /// アイテム ii の panic の場所の行の基準 (アイテムの先頭の行 - 1)
    pub fn line_base(&self, ii: usize) -> u32 {
        let it = &self.items[ii];
        let user_lines = self.prelude_line0 as u32;
        if ii >= self.prog.prelude_from as usize && self.prelude_line0 > 0 && user_lines < PRELUDE_LINE && it.line >= self.prelude_line0 {
            PRELUDE_LINE + (it.line - self.prelude_line0).saturating_sub(1) as u32
        } else {
            it.line as u32 - 1
        }
    }
}

impl Analysis {
    /// 依存するクレートの .o にある関数 (ここではコンパイルしない)
    pub fn is_ext(&self, id: u32) -> bool {
        concrete(&self.prog, id as usize) && self.prog.item_of[id as usize].is_some_and(|ii| self.ext_items[ii as usize])
    }
    /// 依存するクレートの static
    pub fn is_ext_static(&self, ci: u32) -> bool {
        self.ext_items.get(self.prog.consts[ci as usize].item as usize).copied().unwrap_or(false)
    }
}

pub fn analyze(src0: &str, opt: bool) -> R<Analysis> {
    Session::new(Options { opt, no_cache: true, jobs: 1, ..Default::default() }).analyze(src0)
}

/// 機械語にするバックエンド (Cranelift: zust-clif の compile)。feature `build` なしでは使わない
#[cfg(feature = "build")]
pub type Backend = crate::lower::ClBackend;
#[cfg(not(feature = "build"))]
pub type Backend = fn();

/// ドライバの設定
#[derive(Clone, Debug)]
pub struct Options {
    pub opt: bool,
    /// 関数のコンパイルに使うプロセスの数 (fork する)
    pub jobs: usize,
    pub linker: Option<String>,
    /// `-L` directories, searched for the `lib<name>.a` of a `#[link(name = "..")]`
    pub lib_dirs: Vec<std::path::PathBuf>,
    pub no_cache: bool,
    /// 段階ごとの時間を stderr に出す
    pub time: bool,
    /// minclif の代わりのバックエンド (Cranelift: crate::clif_backend::compile)
    pub backend: Option<Backend>,
    /// `#[cfg(feature = "..")]` で真になる feature (1 つのクレートのビルド。ワークスペースはクレートごとに CrateSpec が持つ)
    pub features: Vec<String>,
    /// wasm32-unknown-unknown: writes the module instead of an ELF object, and does not link (zust_wasm::write_module;
    /// `backend` is then zust_wasm::compile)
    pub module_writer: Option<ModuleWriter>,
    /// The edition (`--edition`, 2021 by default). Only what differs between 2015 and later reads it: the crate root's
    /// own `extern crate std` holds the name `std` in 2015 alone
    pub edition: u16,
    /// `--cfg name` / `--cfg name="value"`: more cfgs of the crate (`--test` adds `test`)
    pub cfgs: Vec<(String, Option<String>)>,
    /// `--test`: build the crate's `#[test]` functions with a harness for its `main` (crate::test_harness)
    pub test: bool,
}

/// Writes the output from the functions and data build.rs lays out (in place of the ELF object)
pub type ModuleWriter = fn(&[(String, Vec<u8>, Vec<crate::ir::Reloc>, bool)], &[(String, Vec<u8>, Vec<(u32, String, i64)>, bool)]) -> Result<Vec<u8>, String>;

impl Default for Options {
    fn default() -> Self {
        Options { opt: false, jobs: cpu_count(), linker: None, lib_dirs: vec![], no_cache: false, time: false, backend: None, features: vec![], module_writer: None, edition: 2021, cfgs: vec![], test: false }
    }
}

impl Options {
    /// キャッシュのキーに入れるバックエンドの印 (Cranelift なら 1)
    pub fn backend_id(&self) -> u8 {
        // (wasm: the address checks are part of the code, so a build without them (ZUSTC_WASM_NO_ADDR_CHECK, for
        // measuring) has its own cache entries)
        if self.module_writer.is_some() {
            return if std::env::var_os("ZUSTC_WASM_NO_ADDR_CHECK").is_some() { 3 } else { 2 };
        }
        self.backend.is_some() as u8
    }

    /// cfg の述語の設定 (features は与えたもの)
    pub fn cfg_with(&self, features: &[String]) -> crate::cfg::CfgSet {
        self.cfg_of(features, &[], &[])
    }

    /// cfg の述語の設定 (ワークスペースのクレート: feature、ビルドスクリプトの cfg、`env!` の環境)
    pub fn cfg_of(&self, features: &[String], cfgs: &[(String, Option<String>)], env: &[(String, String)]) -> crate::cfg::CfgSet {
        crate::cfg::CfgSet {
            features: features.to_vec(),
            debug_assertions: !self.opt,
            wasm: self.module_writer.is_some(),
            cfgs: self.cfgs.iter().chain(cfgs).cloned().chain(self.test.then(|| ("test".to_string(), None))).collect(),
            env: env.to_vec(),
            edition: self.edition,
        }
    }
}

/// Whether a line of a crate's source (its own `ol` lines, then its dependencies) is the crate's own: in its lines, or
/// in an item hoisted out of a fn body written in them (`marks`: prep::hoist_marks)
fn own_line(marks: &[(usize, usize)], ol: usize, line: usize) -> bool {
    line <= ol || crate::prep::hoisted_at(marks, line).is_some_and(|w| w <= ol)
}

/// 複数のクレートのうちの 1 つをコンパイルするときの情報。ソースは「このクレートのコード (1..=own_lines 行)」の後ろに
/// 依存するクレートを `pub mod name { .. }` として付けたもの。依存するクレートの (総称でない) 関数と static は
/// そのクレートの .o にあるので、ここではコンパイルせずシンボルで参照する
#[derive(Clone, Debug, Default)]
pub struct CrateCtx {
    /// ライブラリをコンパイルするときはその名前 (総称でない関数と static を外に見せる)
    pub lib: Option<String>,
    /// このクレートのコードの行数 (0 なら全部)
    pub own_lines: u32,
    /// 結合したソースの行 → ファイル
    pub map: crate::srcmap::SrcMap,
    /// 依存するクレート (推移的に全部。実行ファイルの panic の表がそれらの表を指す)
    pub deps: Vec<String>,
    /// The dependencies this crate and those crates name otherwise (renamed ones, CrateSpec::aliases), as the
    /// flattening of the modules reads them: `crate:alias=name`, `:alias=name` for a bin's own
    /// (lexer::rename_module_collisions_in)
    pub externs: Vec<String>,
    /// このクレートの feature (ワークスペース。None なら Options の features)
    pub features: Option<Vec<String>>,
    /// このクレートの cfg と `env!` の環境 (ワークスペース: CrateSpec の cfgs / env)
    pub cfgs: Vec<(String, Option<String>)>,
    pub env: Vec<(String, String)>,
    /// 依存するクレートのテキストの範囲 (結合したソースの行 [開始, 終わり)) と、そこから見えるクレート (この表の番号。自分も)
    pub dep_ranges: Vec<(u32, u32, Vec<usize>, u32)>,
}

/// ビルドの結果
#[derive(Clone, Debug, Default)]
pub struct BuildReport {
    pub items_parsed: usize,
    pub fns_compiled: usize,
    pub fns_hit: usize,
    pub object_bytes: usize,
    pub linked: bool,
}

// (read / write は std と同じ名前の libc の関数。ポインタを *mut u8 で受ける: zustc でもコンパイルできるように)
#[allow(suspicious_runtime_symbol_definitions)]
unsafe extern "C" {
    #[link_name = "sysconf"]
    fn c_sysconf(name: i32) -> i64;
    #[link_name = "stat"]
    fn c_stat(path: *const u8, buf: *mut u8) -> i32;
    #[link_name = "fork"]
    fn c_fork() -> i32;
    #[link_name = "pipe"]
    fn c_pipe(fds: *mut i32) -> i32;
    #[link_name = "read"]
    fn c_read(fd: i32, buf: *mut u8, n: usize) -> isize;
    #[link_name = "write"]
    fn c_write(fd: i32, buf: *const u8, n: usize) -> isize;
    #[link_name = "close"]
    fn c_close(fd: i32) -> i32;
    #[link_name = "waitpid"]
    fn c_waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    #[cfg(feature = "build")]
    #[link_name = "mkdir"]
    fn c_mkdir(path: *const u8, mode: u32) -> i32;
    #[link_name = "_exit"]
    fn c_exit(code: i32) -> !;
}

/// 使える CPU の数
pub fn cpu_count() -> usize {
    // SAFETY: _SC_NPROCESSORS_ONLN (84)
    let n = unsafe { c_sysconf(84) };
    if n < 1 { 1 } else { n as usize }
}

/// 単調時計 (ナノ秒)

/// コンパイラ自身の識別 (実行ファイルの大きさと更新時刻): 作り直したらキャッシュを使わない
fn compiler_id() -> Key {
    let mut buf = [0u8; 256];
    // SAFETY: "/proc/self/exe" は NUL で終わる。buf は struct stat (144 byte) より大きい
    let r = unsafe { c_stat(b"/proc/self/exe\0".as_ptr(), buf.as_mut_ptr()) };
    let mut h = KeyHasher::new();
    h.add("session-1");
    if r == 0 {
        // (x86-64: st_size は 48、st_mtim は 88 / 96)
        h.add(&buf[48..56]);
        h.add(&buf[88..104]);
    }
    h.finish()
}

/// 時間の計測 (-time)

/// コンパイルのセッション: キャッシュと、編集をまたいで覚えるものを持ち、build / check / analyze を何度でも呼べる
pub struct Session {
    pub opts: Options,
    cid: Key,
    disk: crate::build_cache::BuildCache,
    mem: FxMap<Key, Vec<u8>>,
    used: FxSet<Key>,
    /// put したキー (ワークスペースの子プロセスが、作ったエントリを親に返す)
    fresh: Vec<Key>,
    /// prelude の選択の、アイテムごとの記憶 (LSP の編集のたびに全アイテムを調べ直さない)
    sel: FrontMemo,
    /// fn アイテムのハッシュ → 要約の符号のハッシュ (Analysis::iface_fp を作るため)
    sum_fp: FxMap<Key, u64>,
    /// 前回の check の記録
    chk: Option<ChkMemo>,
    /// 複数のクレートのうちの 1 つをコンパイルするとき
    pub ctx: CrateCtx,
    /// the files the last build_path read (for the CLI's stamp file)
    pub last_map: Option<crate::srcmap::SrcMap>,
    /// A crate of one file that the caller configured before it checks it: the `extern crate x;` items the configuring
    /// blanked (SrcMap::externs), whose names may be defined twice. Read by the next analyze
    pub externs: Vec<(usize, String)>,
    /// The same crate's first `cfg` predicate rustc cannot read, found in the file as written (SrcMap::cfg_err)
    pub cfg_err: Option<String>,
    /// In a `check`: its analysis adds const_check_item
    checking: bool,
    /// The lines of the constants that the check's const_check_item reads, in its order
    const_lines: Vec<usize>,
    /// their fns' names' prefix (CONST_CHECK_FN, or another when the program writes that)
    const_fn: String,
    /// In a check_workspace: each crate's result (and no objects are written)
    #[cfg(feature = "build")]
    ws_diags: Option<Vec<CrateCheck>>,
    /// (keep_for_edits) The last analysis's item summaries by their cache key (as the cache's bytes decode: a summary
    /// encodes every field): the next analysis takes those of its items from here instead of decoding each (and
    /// dropping them after)
    sum_keep: Option<FxMap<Key, Vec<Summary>>>,
    /// What the last E0038 check found of the program's traits (dyncompat::check)
    dyn_memo: dyncompat::DynMemo,
    /// (keep_for_edits) Each item's refs (Program::refs_in) by its hash, for the program's name tables of the
    /// fingerprint (Program::ref_tables_fp): an edit that adds or renames nothing reads only the changed items again
    refs_memo: Option<(Key, FxMap<Key, (Vec<u32>, Vec<u32>, Vec<u32>)>)>,
    /// (keep_for_edits) A program as its build made it (before the analysis changed it), with the fingerprint of what
    /// the build read (prog_fp), and the last analysis's fingerprint: a program is kept when two analyses in a row
    /// build the same, so opening a file pays nothing for it
    prog_keep: Option<ProgKeep>,
}

#[derive(Default)]
struct ProgKeep {
    last: Key,
    prog: Option<(Key, Program)>,
}

impl Session {
    pub fn new(opts: Options) -> Session {
        Session { opts, cid: compiler_id(), disk: crate::build_cache::BuildCache::empty(), mem: FxMap::default(), used: FxSet::default(), fresh: vec![], ctx: CrateCtx::default(), last_map: None, sel: Default::default(), sum_fp: FxMap::default(), chk: None, externs: vec![], cfg_err: None, checking: false, const_lines: vec![], const_fn: String::new(), #[cfg(feature = "build")] ws_diags: None, sum_keep: None, dyn_memo: Default::default(), refs_memo: None, prog_keep: None }
    }

    /// For a session whose document will be edited (zust-analyzer): the preprocessing keeps its chunks from the first
    /// analysis on, so that the first edit preprocesses only what it changed (PrepMemo::keep_from_first)
    pub fn keep_for_edits(&mut self) {
        self.sel.prep.keep_from_first();
        // (and the item summaries, decoded, from one analysis to the next, and the items' refs)
        self.sum_keep = Some(FxMap::default());
        self.refs_memo = Some((0, FxMap::default()));
        self.prog_keep = Some(ProgKeep::default());
    }

    pub fn load_cache(&mut self, path: &std::path::Path) {
        if !self.opts.no_cache {
            self.disk = crate::build_cache::BuildCache::open(path);
        }
    }

    /// 今回使ったエントリだけを書く
    pub fn save_cache(&mut self, path: &std::path::Path) -> Result<usize, String> {
        // (今回使ったものはディスクの上の古い表にしかなくてもメモリに移す: 書いたファイルを開き直さないので)
        let used = self.used.clone();
        for &k in &used {
            if !self.mem.contains_key(&k) {
                if let Some(v) = self.disk.get(k) {
                    let v = v.to_vec();
                    self.mem.insert(k, v);
                }
            }
        }
        let n = self.save_cache_keep(path)?;
        self.mem.retain(|k, _| used.contains(k));
        self.disk = crate::build_cache::BuildCache::empty();
        Ok(n)
    }

    /// save_cache と同じだが、メモリの上のエントリは残す (ワークスペースで次のクレートが使う)
    fn save_cache_keep(&mut self, path: &std::path::Path) -> Result<usize, String> {
        if self.opts.no_cache {
            return Ok(0);
        }
        let mut keys: Vec<Key> = self.used.iter().copied().collect();
        keys.sort_unstable();
        let mut entries: Vec<(Key, Vec<u8>)> = vec![];
        for k in keys {
            let v = match self.mem.get(&k) {
                Some(v) => v.clone(),
                None => match self.disk.get(k) {
                    Some(v) => v.to_vec(),
                    None => continue,
                },
            };
            entries.push((k, v));
        }
        let refs: Vec<(Key, &[u8])> = entries.iter().map(|(k, v)| (*k, &v[..])).collect();
        let n = crate::build_cache::BuildCache::save(path, &refs)?;
        self.used.clear();
        Ok(n)
    }

    fn get(&mut self, k: Key) -> Option<Vec<u8>> {
        if self.opts.no_cache {
            return None;
        }
        self.used.insert(k);
        match self.mem.get(&k) {
            Some(v) => Some(v.clone()),
            None => self.disk.get(k).map(|v| v.to_vec()),
        }
    }

    fn put(&mut self, k: Key, v: Vec<u8>) {
        if self.opts.no_cache {
            return;
        }
        self.used.insert(k);
        self.mem.insert(k, v);
        self.fresh.push(k);
    }

    /// Where the items moved out of fn bodies came from, for src as analyze prepares it (Prep::moved)
    fn moved_lines(&self, src0: &str) -> Option<Vec<(usize, usize)>> {
        let stripped = if self.ctx.map.inputs.is_empty() { crate::cfg::strip(src0, &self.opts.cfg_with(&self.opts.features)) } else { None };
        let src0 = stripped.as_deref().unwrap_or(src0);
        let mut crates = self.ctx.deps.clone();
        crates.extend(self.ctx.lib.iter().cloned());
        crates.extend(self.ctx.externs.iter().cloned());
        let cfg = self.opts.cfg_with(self.ctx.features.as_ref().unwrap_or(&self.opts.features));
        let renamed = crate::prep::std_renames(src0);
        crate::prep::moved_lines(renamed.as_deref().unwrap_or(src0), &crates, Some(&cfg))
    }

    /// 要約はアイテムのハッシュで引く
    pub fn analyze(&mut self, src0: &str) -> R<Analysis> {
        // (closures' captures by edition: `let _ = s;` captures s before 2021 only (ast_closure::WILD_FAKE_READS))
        crate::ast_closure::WILD_FAKE_READS.with(|c| c.set(self.opts.edition >= 2021));
        let mut externs = std::mem::take(&mut self.externs);
        // (a `cfg` predicate rustc cannot read, in the source as written: `#[cfg(..)]`, `#[cfg_attr(..)]` and `cfg!(..)`
        // are taken away before the items are read; reported where the attributes are, after a parse error)
        let mut cfg_err = self.cfg_err.take();
        // (ファイルから読んだもの (load_crate) は読むときに cfg を評価してある)
        let stripped;
        let src0 = if self.ctx.map.inputs.is_empty() {
            let cfg = self.opts.cfg_with(&self.opts.features);
            let mut ext = vec![];
            let configured = crate::cfg::strip_configured(src0, &cfg, &mut ext);
            if cfg_err.is_none() {
                cfg_err = crate::cfg::fn_qualifier_order(src0).or_else(|| malformed_cfg(src0, configured.as_deref(), &cfg));
            }
            match configured {
                Some(t) => {
                    // (the `extern crate` items it blanked, for the names they define)
                    if externs.is_empty() {
                        externs = ext.iter().map(|&(a, b)| (a, src0[a..b].to_string())).collect();
                    }
                    stripped = t;
                    stripped.as_str()
                }
                None => src0,
            }
        } else {
            src0
        };
        let mut crates = self.ctx.deps.clone();
        crates.extend(self.ctx.lib.iter().cloned());
        crates.extend(self.ctx.externs.iter().cloned());
        let mut fix = vec![];
        let cfg = self.opts.cfg_of(self.ctx.features.as_ref().unwrap_or(&self.opts.features), &self.ctx.cfgs, &self.ctx.env);
        crate::macros::BAD_FRAGMENT.with(|c| c.borrow_mut().take());
        crate::macros::BAD_DEF.with(|c| c.set(false));
        crate::macros::BAD_FOLLOW.with(|c| c.borrow_mut().take());
        crate::macros::EXPORTED.with(|c| c.borrow_mut().clear());
        crate::macros::PAT_WITH_OR.with(|c| c.set(self.opts.edition >= 2021));
        crate::macros::UNBOUND.with(|c| c.borrow_mut().take());
        crate::macros::DUP_EXPORT.with(|c| c.borrow_mut().take());
        let (mut src, mut items, mut user, prelude_line0) = with_prelude_in(src0, &crates, &mut fix, Some(&cfg), &mut self.sel)?;
        self.ctx.map.colfix = fix;
        // (`zustc check`: each constant of the program is checked where it is defined, used or not, as rustc does: a
        // fn of the program that reads them, on lines past every other (check maps its errors back to them))
        // (the program's own items, before the constants' fns: what the source's own scans read)
        let user_src = user;
        self.const_lines.clear();
        if self.checking {
            if let Some((text, lines, name)) = const_check_item(&src, &items[..user]) {
                if !src.ends_with('\n') {
                    src.push('\n');
                }
                let off = src.len();
                src.push_str(&text);
                if let Ok(mut its) = lexer::split(&text) {
                    for it in &mut its {
                        it.start += off;
                        it.end += off;
                        it.line += CONST_CHECK_LINE - 1;
                        for id in &mut it.idents {
                            id.0 += off as u32;
                            id.1 += off as u32;
                        }
                    }
                    let n = its.len();
                    // (the prelude's items move by n: its members' headers too)
                    for it in &mut items[user..] {
                        if let ItemKind::ImplMember(h) | ItemKind::TraitMember(h) = &mut it.kind {
                            if *h as usize >= user {
                                *h += n as u32;
                            }
                        }
                    }
                    items.splice(user..user, its);
                    user += n;
                    self.const_lines = lines;
                    self.const_fn = name;
                }
            }
        }
        // (the configured text's tokens, for the scans of its modules (E0428, E0603, E0432, E0659): read once, where it
        // has a module, a `use` of 2015, whose paths start at the crate root (`use main::bar;`), or a glob import)
        let modules = src0.contains("mod ") || self.opts.edition < 2018 && src0.contains("use ") || src0.contains("::*");
        let src0_toks = (self.ctx.map.inputs.is_empty() && modules && src0.contains("::")).then(|| crate::lexer::tokenize(src0.as_bytes()));
        // (E0432: a top-level `use` of a path whose first segment names nothing; the `use` items come from the item
        // split, the names they could mean from the original text)
        let mut dyn_err: Option<String> = None;
        // (E0201 / E0592: two items of one name and namespace in one impl block, before the program is built (which
        // would report it as another error))
        if let Some(e) = duplicate_impl_item(&src, &items[..user_src]) {
            return Err(e);
        }
        // (E0116, E0118, E0091: an inherent impl's self type, a type alias's parameters)
        if let Some(e) = item_decl_err(&src, &items[..user_src], self.ctx.deps.is_empty()) {
            return Err(e);
        }
        if self.ctx.map.inputs.is_empty() {
            if let Some(e) = unresolved_import(src0, &src, &items[..user_src], &self.ctx.deps) {
                return Err(e);
            }
            if let Some(e) = duplicate_import(src0, &src, &items[..user_src]) {
                return Err(e);
            }
            // (E0428 of an inline module and an item: a whole-text pass, so not on each build of a crate loaded from
            // files, whose incremental builds would pay it every time)
            if let Some(e) = duplicate_name(src0, src0_toks.as_deref(), &externs, self.opts.edition < 2018) {
                return Err(e);
            }
            // (E0038: a `dyn` of a trait of the program's that is not dyn compatible. rustc's well-formedness check
            // reports it after the errors of resolution and the type check: as late_err below)
            dyn_err = dyncompat::check(&src, &items[..user_src], &mut self.dyn_memo);
            if let Some(e) = cfg_err.take().or_else(|| malformed_attr(src0, &src, &items[..user_src], self.ctx.deps.is_empty() && self.ctx.lib.is_none(), self.opts.edition)) {
                return Err(e);
            }
            // (a `macro_rules!` matcher's fragment specifier rustc does not know, used or not: the expansion notes one when
            // it reads the definitions; its place is read from the text then)
            // (a doc comment the source ends in, read from its end as written)
            if let Some(e) = crate::lexer::doc_at_end(src0) {
                return Err(e);
            }
            // (E0255: a `#[macro_export]` macro of a name a `pub use` of std's at the crate root brings into the
            // macro namespace too (`pub use std::panic;`), at the macro)
            let exported = crate::macros::EXPORTED.with(|c| std::mem::take(&mut *c.borrow_mut()));
            if !exported.is_empty() {
                if let Some(e) = export_beside_std_reexport(src0, &exported) {
                    return Err(e);
                }
            }
            // (E0428: two `#[macro_export]` macros of one name, at the second)
            if let Some(n) = crate::macros::DUP_EXPORT.with(|c| c.borrow_mut().take()) {
                let pat = format!("macro_rules! {n}");
                if let Some((at, _)) = src0.match_indices(pat.as_str()).nth(1) {
                    let line = 1 + src0[..at].matches('\n').count();
                    return Err(format!("{line}:1: the name `{n}` is defined multiple times (E0428)"));
                }
            }
            // (a rule's `$name` its matcher does not bind, left in the expansion: noted by the expansion, read from the
            // text then)
            if let Some((n, rep)) = crate::macros::UNBOUND.with(|c| c.borrow_mut().take()) {
                if let Some(e) = crate::macros::unbound_err(&src, src0, &n, rep) {
                    return Err(e);
                }
            }
            // (a `macro_rules!` definition rustc rejects as it reads it: no rule, a matcher outside delimiters, a rule cut
            // short; noted by the expansion, its place read from the text then)
            if crate::macros::BAD_DEF.with(|c| c.replace(false)) {
                if let Some(e) = crate::macros::macro_def_err(src0) {
                    return Err(e);
                }
            }
            // (a fragment followed by a token rustc does not allow after it, `$e:expr +`)
            if let Some((frag, msg)) = crate::macros::BAD_FOLLOW.with(|c| c.borrow_mut().take()) {
                if let Some(e) = crate::macros::follow_err(src0, &frag, &msg) {
                    return Err(e);
                }
            }
            if crate::macros::BAD_FRAGMENT.with(|c| c.borrow_mut().take()).is_some() {
                if let Some(e) = bad_fragment(src0) {
                    return Err(e);
                }
            }
        }
        let mut tm = Timer { on: std::env::var_os("ZUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
        if let Some(p) = std::env::var_os("ZUSTC_DUMP_SRC") {
            let _ = std::fs::write(p, &src);
        }
        let cid = self.cid;
        let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
        let mut n_parsed = 0;
        // (the last analysis's summaries (keep_for_edits; not without the cache, which would not give them), and each
        // item's key, to keep this one's for the next. One parsed now is kept as it is: its bytes decode to the same
        // summary (every field encodes))
        let mut keep = if self.opts.no_cache { None } else { self.sum_keep.take() };
        // (still on after an analysis that stops at an error: the next starts with none kept)
        if keep.is_some() {
            self.sum_keep = Some(FxMap::default());
        }
        let mut sum_keys: Vec<Key> = if keep.is_some() { vec![0; items.len()] } else { vec![] };
        // (the keys' common prefix, hashed once: a closure's captures depend on the edition (WILD_FAKE_READS))
        let seed = {
            let mut h = KeyHasher::new();
            h.add(&cid);
            h.add(&0u8);
            h.add(&(self.opts.edition >= 2021));
            h
        };
        for pass in 0..2 {
            // (キャッシュにないもの: (番号, キー))
            let mut miss: Vec<(usize, Key)> = vec![];
            for (i, it) in items.iter().enumerate() {
                let header = matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader);
                if header != (pass == 0) {
                    continue;
                }
                let mut h = seed.clone();
                h.add(&it.hash);
                let k = h.finish();
                if let Some(keep) = &mut keep {
                    sum_keys[i] = k;
                    // (what the cache's bytes for k decode to, from the last analysis; still a use of k for the cache)
                    if let Some(sm) = keep.get_mut(&k).and_then(|v| v.pop()) {
                        self.used.insert(k);
                        sums[i] = sm;
                        continue;
                    }
                }
                if let Some(sm) = self.get(k).and_then(|d| Summary::decode(&d)) {
                    sums[i] = sm;
                    continue;
                }
                miss.push((i, k));
            }
            // (多ければ fork して分けて作る: 子 k は j % jobs == k のものを作り、直列化して返す)
            // (not in a process of more threads than this one: single_threaded)
            let jobs = if single_threaded() { self.opts.jobs.min(miss.len() / 32).max(1) } else { 1 };
            let mut got: Vec<Option<Summary>> = vec![None; miss.len()];
            if jobs > 1 {
                let mut kids: Vec<(i32, i32)> = vec![];
                for kk in 0..jobs {
                    let mut fds = [0i32; 2];
                    // SAFETY: fds は 2 つの int
                    if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                        continue;
                    }
                    let pid = fork_counted();
                    if pid == 0 {
                        unsafe { c_close(fds[0]) };
                        let mut buf = vec![];
                        let mut j = kk;
                        while j < miss.len() {
                            let it = &items[miss[j].0];
                            let ow = if pass == 1 { crate::ast_summary::owner_of(it, &sums) } else { None };
                            // (失敗したものは送らない: 親が作り直してエラーを出す)
                            if let Ok(sm) = crate::ast_summary::parse_and_summarize(&src, it, ow.as_ref()) {
                                let mut e = vec![];
                                sm.encode(&mut e);
                                crate::build_cache::put_uv(&mut buf, j as u64);
                                crate::build_cache::put_uv(&mut buf, e.len() as u64);
                                buf.extend_from_slice(&e);
                            }
                            j += jobs;
                        }
                        write_all(fds[1], &buf);
                        unsafe { c_close(fds[1]) };
                        unsafe { c_exit(0) };
                    }
                    unsafe { c_close(fds[1]) };
                    if pid < 0 {
                        unsafe { c_close(fds[0]) };
                        continue;
                    }
                    kids.push((pid, fds[0]));
                }
                for (pid, fd) in kids {
                    let d = read_all(fd);
                    unsafe { c_close(fd) };
                    let mut st = 0i32;
                    unsafe { c_waitpid(pid, &mut st, 0) };
                    let mut r = crate::build_cache::Rd { d: &d, pos: 0 };
                    while r.pos < d.len() {
                        let (Some(j), Some(n)) = (r.uv(), r.uv()) else { break };
                        let Some(body) = r.take(n as usize) else { break };
                        if let Some(slot) = got.get_mut(j as usize) {
                            *slot = Summary::decode(body);
                        }
                    }
                }
            }
            for (j, &(i, k)) in miss.iter().enumerate() {
                let it = &items[i];
                let sm = match got[j].take() {
                    Some(sm) => sm,
                    None => {
                        let ow = if pass == 1 { crate::ast_summary::owner_of(it, &sums) } else { None };
                        crate::ast_summary::parse_and_summarize(&src, it, ow.as_ref()).map_err(|e| {
                            // (a validation error rustc reports after parsing: a later item's syntax error first)
                            if let Some(d) = e.strip_prefix('\u{7}') {
                                return deferred_after_parse(relocate(d.to_string(), it, &src), &src, &items[i + 1..user_src]);
                            }
                            let e = match (e.strip_prefix('\u{2}'), it.kind) {
                                // (`\u{2}msg`: an impl member's error that is at its impl's header)
                                (Some(m), ItemKind::ImplMember(h)) => format!("{}:1: {m}", items[h as usize].line),
                                _ => relocate(e.trim_start_matches('\u{2}').to_string(), it, &src),
                            };
                            let e = ret_type_line(e, &src, it);
                            // (an item moved out of a fn body that cannot see a name of the items around it: rustc's
                            // E0401, where that is what it names)
                            let line_start = src[..it.start.min(src.len())].rfind('\n').map_or(0, |p| p + 1);
                            let hoisted = src[line_start..].starts_with(crate::prep::HOIST_MARK) || src[it.start.min(src.len())..].starts_with(crate::prep::HOIST_MARK);
                            let unseen = hoisted || ["`Self` is only available in impls", "cannot find type `", "cannot find value `"].iter().any(|m| e.contains(m));
                            if unseen && self.ctx.map.inputs.is_empty() && src.contains(crate::prep::HOIST_MARK) {
                                if let Some(o) = outer_generics(src0, &crate::lexer::tokenize(src0.as_bytes())) {
                                    return o;
                                }
                            }
                            e
                        })?
                    }
                };
                n_parsed += 1;
                let mut b = vec![];
                sm.encode(&mut b);
                sums[i] = sm;
                self.put(k, b);
            }
        }
        // (E0603: a path through the program's own modules to an item it may not see; resolved after the items parse,
        // as rustc reports a syntax error first. And the fields modules may not read, for the type check's E0616)
        let mut fpriv: Option<Box<crate::program::FieldPriv>> = None;
        let mut late_err: Option<String> = None;
        if let Some(toks) = &src0_toks {
            if let Some(e) = private_path(src0, toks, &externs, self.opts.edition < 2018, &mut fpriv, &mut late_err) {
                return Err(e);
            }
        }
        if late_err.is_none() {
            late_err = dyn_err;
        }
        // (E0401: an item in a fn body that names a generic parameter, or `Self`, of an item around it; read where an
        // item was moved out of a fn body, after the items parse)
        if self.ctx.map.inputs.is_empty() && src.contains(crate::prep::HOIST_MARK) {
            let own;
            let toks: &[(u8, usize, usize)] = match &src0_toks {
                Some(t) => t,
                None => {
                    own = crate::lexer::tokenize(src0.as_bytes());
                    &own
                }
            };
            if let Some(e) = outer_generics(src0, toks) {
                return Err(e);
            }
        }
        // 宣言の指紋 (要約の符号のハッシュはアイテムのハッシュで覚える)
        let iface_fp = {
            let mut h = KeyHasher::new();
            h.add(&self.ctx.dep_ranges.len());
            // (the edition changes what a body's check finds: tail temporaries)
            h.add(&self.opts.edition);
            let mut new_fp: FxMap<Key, u64> = FxMap::default();
            for (i, it) in items.iter().enumerate() {
                let is_fn = !sums[i].fns.is_empty() && sums[i].structs.is_empty() && sums[i].enums.is_empty() && matches!(it.kind, ItemKind::Normal | ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
                if !is_fn || self.opts.opt {
                    h.add(&it.hash);
                    continue;
                }
                let x = match self.sum_fp.get(&it.hash) {
                    Some(&x) => x,
                    None => {
                        let mut e = vec![];
                        sums[i].encode(&mut e);
                        let mut hh = KeyHasher::new();
                        hh.add(&e[..]);
                        hh.finish() as u64
                    }
                };
                new_fp.insert(it.hash, x);
                h.add(&x);
            }
            self.sum_fp = new_fp;
            h.finish() as u64
        };
        tm.lap("summaries", {
            if tm.on {
                let ol = self.ctx.own_lines as usize;
                let marks = crate::prep::hoist_marks(&src);
                let own = items[..user_src].iter().filter(|it| ol == 0 || own_line(&marks, ol, it.line)).count();
                format!("{} items: own {own}, deps {}, prelude {}", items.len(), user - own, items.len() - user)
            } else {
                String::new()
            }
        });
        // (E0106 / E0107: the program's fields, type aliases and associated types that name a struct or enum with
        // lifetimes and write none of them, or another number: the definitions are known now. Only the items that
        // mention one by name are read again)
        {
            let mut adts: crate::fxhash::FxMap<&str, usize> = Default::default();
            for sm in &sums[..user_src] {
                if let Some(r) = sm.adt_regions.as_deref().filter(|r| !r.lts.is_empty()) {
                    for n in sm.structs.iter().map(|s| s.0.as_str()).chain(sm.enums.iter().map(|e| e.0.as_str())) {
                        adts.insert(n, r.lts.len());
                    }
                }
            }
            if !adts.is_empty() {
                for (it, sm) in items[..user_src].iter().zip(&sums[..user_src]) {
                    let kind = !sm.structs.is_empty() || !sm.enums.is_empty() || !sm.aliases.is_empty() || !sm.consts.is_empty() || !sm.statics.is_empty() || !sm.assocs.is_empty();
                    if !kind {
                        continue;
                    }
                    // (from the summary's types: only an item whose types name such a struct or enum is read again)
                    fn names(t: &crate::types::TySyn, adts: &crate::fxhash::FxMap<&str, usize>) -> bool {
                        use crate::types::TySyn as T;
                        match t {
                            T::Named(n) => adts.contains_key(n.as_str()),
                            T::App(n, a) => adts.contains_key(n.as_str()) || a.iter().any(|t| names(t, adts)),
                            T::Ref(_, i) | T::Ptr(_, i) | T::Slice(i) | T::Array(i, _) => names(i, adts),
                            T::Tuple(a) => a.iter().any(|t| names(t, adts)),
                            _ => false,
                        }
                    }
                    let named = sm.structs.iter().any(|s| s.1.iter().any(|f| names(&f.1, &adts)))
                        || sm.enums.iter().any(|e| e.1.iter().any(|v| v.fields.iter().any(|f| names(&f.1, &adts))))
                        || sm.aliases.iter().any(|a| names(&a.2, &adts))
                        || sm.assocs.iter().any(|a| names(&a.value, &adts))
                        || sm.consts.iter().any(|c| names(&c.1, &adts))
                        || sm.statics.iter().any(|c| names(&c.1, &adts));
                    if !named {
                        continue;
                    }
                    let outer: Vec<String> = sm.assocs.iter().flat_map(|a| a.generics.iter().cloned()).collect();
                    if let Some(e) = crate::ast_summary::hidden_lifetimes(&src, it, &adts, &outer) {
                        return Err(e);
                    }
                }
            }
        }
        // (E0310: the program's fields that pass a type parameter to a struct or enum of the program that bounds it by
        // 'static, where the item does not declare it so: the definitions are known now. Only the items whose fields
        // name one are read again)
        {
            let short = |n: &str| n.rsplit("::").next().unwrap_or(n).to_string();
            let mut statics: crate::fxhash::FxMap<String, Vec<usize>> = Default::default();
            for sm in &sums[..user_src] {
                let Some(r) = sm.adt_regions.as_deref().filter(|r| r.ty_outlives.iter().any(|t| t.1 == crate::types::R_STATIC)) else { continue };
                let idx: Vec<usize> = sm.adt_generics.iter().enumerate().filter(|(_, n)| r.ty_outlives.iter().any(|(t, c)| t == *n && *c == crate::types::R_STATIC)).map(|(i, _)| i).collect();
                if idx.is_empty() {
                    continue;
                }
                for n in sm.structs.iter().map(|s| s.0.as_str()).chain(sm.enums.iter().map(|e| e.0.as_str())) {
                    statics.insert(short(n), idx.clone());
                }
            }
            if !statics.is_empty() {
                // (a name two of the program's types have is not followed: which one a field means is not known here)
                let mut seen: crate::fxhash::FxMap<String, u32> = Default::default();
                for sm in &sums[..user_src] {
                    for n in sm.structs.iter().map(|s| s.0.as_str()).chain(sm.enums.iter().map(|e| e.0.as_str())) {
                        *seen.entry(short(n)).or_default() += 1;
                    }
                }
                let statics: crate::fxhash::FxMap<&str, Vec<usize>> = statics.iter().filter(|(n, _)| seen.get(*n) == Some(&1)).map(|(n, v)| (n.as_str(), v.clone())).collect();
                fn names(t: &crate::types::TySyn, st: &crate::fxhash::FxMap<&str, Vec<usize>>) -> bool {
                    use crate::types::TySyn as T;
                    match t {
                        T::Named(n) => st.contains_key(n.as_str()),
                        T::App(n, a) => st.contains_key(n.as_str()) || a.iter().any(|t| names(t, st)),
                        T::Ref(_, i) | T::Ptr(_, i) | T::Slice(i) | T::Array(i, _) => names(i, st),
                        T::Tuple(a) | T::FnPtr(a) => a.iter().any(|t| names(t, st)),
                        T::Dyn(b) => st.keys().any(|k| b.contains(k)),
                        _ => false,
                    }
                }
                for (it, sm) in items[..user_src].iter().zip(&sums[..user_src]) {
                    if sm.adt_generics.is_empty() {
                        continue;
                    }
                    let named = sm.structs.iter().any(|s| s.1.iter().any(|f| names(&f.1, &statics))) || sm.enums.iter().any(|e| e.1.iter().any(|v| v.fields.iter().any(|f| names(&f.1, &statics))));
                    if !named {
                        continue;
                    }
                    if let Some((line, col, m)) = crate::ast_summary::static_field_unmet(&src, it, &statics) {
                        let line = crate::prep::hoisted_at(&crate::prep::hoist_marks(&src), line).unwrap_or(line);
                        return Err(format!("{line}:{col}: {m}"));
                    }
                }
            }
        }
        // (a `Drop` impl of one of the program's structs, enums or unions: its self type's arguments are its own
        // parameters (E0366), and it requires of them only what the definition does (E0367): the definitions are known
        // now. Only the program's `impl Drop for` headers are read again)
        {
            let short = |n: &str| n.rsplit("::").next().unwrap_or(n).to_string();
            let drops: Vec<usize> = (0..user_src).filter(|&i| sums[i].impl_trait.as_deref() == Some("Drop") && sums[i].impl_self.is_some()).collect();
            if !drops.is_empty() {
                let mut adts: crate::fxhash::FxMap<String, (usize, u32)> = Default::default();
                for (i, sm) in sums[..user_src].iter().enumerate() {
                    for n in sm.structs.iter().map(|s| s.0.as_str()).chain(sm.enums.iter().map(|e| e.0.as_str())) {
                        let e = adts.entry(short(n)).or_insert((i, 0));
                        e.1 += 1;
                    }
                }
                let mut supers: crate::fxhash::FxMap<String, Vec<String>> = sums
                    .iter()
                    .filter_map(|sm| {
                        let t = sm.trait_decl.as_deref()?;
                        let s = sm.trait_generics.iter().find(|(n, _)| n == "#supers")?;
                        Some((short(t), s.1.iter().map(|x| short(x.split('<').next().unwrap_or(x).trim())).collect()))
                    })
                    .collect();
                // (the program's own traits: their supertraits are not all recorded (`#supers` leaves out Debug, Clone
                // ..), so a parameter bounded by one is not checked for the trait bounds a Drop impl adds: "#?")
                // (and the markers its header names before its body, which `#supers` leaves out: `trait Tr: Sized`,
                // `trait Tr: Clone`, `where Self: Sized`, so that a Sized one is known)
                for (it, sm) in items[..user_src].iter().zip(&sums[..user_src]) {
                    if let Some(t) = sm.trait_decl.as_deref() {
                        let e = supers.entry(short(t)).or_default();
                        e.push("#?".to_string());
                        let head = src.get(it.start..it.end).map(|x| x.split('{').next().unwrap_or(x)).unwrap_or("");
                        let words: Vec<&str> = head.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
                        for m in ["Sized", "Clone", "Copy", "Default"] {
                            if words.contains(&m) && !e.iter().any(|x| x == m) {
                                e.push(m.to_string());
                            }
                        }
                    }
                }
                // (std's supertraits, which the prelude's declarations leave out: `T: Ord` gives `T: PartialOrd`)
                for (t, sup) in [
                    ("Ord", &["Eq", "PartialOrd"][..]),
                    ("Eq", &["PartialEq"]),
                    ("PartialOrd", &["PartialEq"]),
                    ("Copy", &["Clone"]),
                    ("Fn", &["FnMut"]),
                    ("FnMut", &["FnOnce"]),
                    ("Error", &["Debug", "Display"]),
                    ("DoubleEndedIterator", &["Iterator"]),
                    ("ExactSizeIterator", &["Iterator"]),
                    ("FusedIterator", &["Iterator"]),
                    ("DerefMut", &["Deref"]),
                    ("IndexMut", &["Index"]),
                    ("BufRead", &["Read"]),
                ] {
                    let e = supers.entry(t.to_string()).or_default();
                    for x in sup {
                        if !e.iter().any(|y| y == x) {
                            e.push(x.to_string());
                        }
                    }
                }
                for &i in &drops {
                    // (a name two of the program's types have is not followed: which one the impl means is not known here)
                    let Some(&(ai, 1)) = sums[i].impl_self.as_deref().and_then(|n| adts.get(&short(n))) else { continue };
                    if let Some((rel, m)) = crate::ast_summary::drop_impl_unmet(&src, &items[i], &items[ai], &supers) {
                        let line = items[i].line + rel.max(1) - 1;
                        let line = crate::prep::hoisted_at(&crate::prep::hoist_marks(&src), line).unwrap_or(line);
                        return Err(format!("{line}:1: {m}"));
                    }
                }
            }
        }
        // (what the program's types require of traits' `Self: 'x` bounds (E0478, E0310): the traits are known now)
        {
            let mut so: crate::fxhash::FxMap<&str, &[String]> = sums
                .iter()
                .filter_map(|sm| Some((sm.trait_decl.as_deref()?, sm.trait_generics.iter().find(|g| g.0 == crate::types::SELF_OUTLIVES)?.1.as_slice())))
                .collect();
            // (std's `Any` is `trait Any: 'static`: `dyn Any + 'a` needs 'a: 'static, unless the program has a trait of
            // that name. Only where the program names it: otherwise the items need not be read again)
            let any_req = ["static".to_string()];
            let user_end = items[..user_src].last().map_or(0, |it| it.end);
            if !so.contains_key("Any") && src.get(..user_end).is_some_and(|t| t.contains("Any")) && !sums.iter().any(|sm| sm.trait_decl.as_deref() == Some("Any")) {
                so.insert("Any", &any_req);
            }
            if !so.is_empty() {
                for (it, sm) in items[..user_src].iter().zip(&sums[..user_src]) {
                    // (an impl of such a trait, or a type whose fields write `dyn`: read again)
                    let of_trait = sm.impl_trait.as_deref().is_some_and(|t| so.contains_key(t));
                    let dyns = (!sm.structs.is_empty() || !sm.enums.is_empty() || !sm.unions.is_empty()) && src.get(it.start..it.end).is_some_and(|t| t.contains("dyn"));
                    if !of_trait && !dyns {
                        continue;
                    }
                    for o in &crate::ast_summary::trait_obls(&src, it) {
                        let Some(req) = so.get(o.tr.as_str()) else { continue };
                        let k = crate::wf::Known { pairs: o.known.clone(), vague: o.vague.clone(), ..Default::default() };
                        for c in req.iter() {
                            let x = if c == "static" {
                                "'static".to_string()
                            } else {
                                match c.parse::<usize>().ok().and_then(|i| o.args.get(i)) {
                                    Some(a) => a.clone(),
                                    None => continue,
                                }
                            };
                            for s in &o.sub {
                                if o.vague.contains(s) || k.outlives(s, &x) {
                                    continue;
                                }
                                let line = it.line + o.line.max(1) as usize - 1;
                                return Err(if s.starts_with('\'') {
                                    format!("{line}:1: lifetime bound not satisfied (`{s}` must outlive `{x}`, which trait `{}` requires) (E0478)", o.tr)
                                } else {
                                    format!("{line}:1: the parameter type `{s}` may not live long enough (E0{})", if x == "'static" { 310 } else { 309 })
                                });
                            }
                        }
                    }
                }
            }
        }
        // (an impl's associated type's value against the lifetime bounds its trait's declaration gives it (E0477))
        {
            let decl: crate::fxhash::FxMap<(&str, &str), Vec<String>> = sums
                .iter()
                .flat_map(|sm| sm.assoc_bounds.iter())
                .filter_map(|(t, n, bs)| {
                    let lb: Vec<String> = bs.iter().filter(|b| b.starts_with('\'')).cloned().collect();
                    (!lb.is_empty()).then_some(((t.as_str(), n.as_str()), lb))
                })
                .collect();
            if !decl.is_empty() {
                let aliases: crate::fxhash::FxSet<&str> = sums.iter().flat_map(|sm| sm.aliases.iter().map(|a| a.0.as_str())).collect();
                let is_alias = |n: &str| aliases.contains(n) || aliases.iter().any(|a| a.rsplit("::").next() == Some(n));
                // (the structs and enums whose declarations bound their lifetimes (`struct Static<'a: 'static>`))
                let bounded: crate::fxhash::FxSet<&str> = sums
                    .iter()
                    .filter(|sm| sm.adt_regions.as_ref().is_some_and(|r| !r.outlives.is_empty()))
                    .flat_map(|sm| sm.structs.iter().map(|x| x.0.as_str()).chain(sm.enums.iter().map(|x| x.0.as_str())))
                    .collect();
                let lt_bounded = |n: &str| bounded.contains(n) || bounded.iter().any(|a| a.rsplit("::").next() == Some(n));
                for (it, sm) in items[..user_src].iter().zip(&sums[..user_src]) {
                    let ItemKind::ImplMember(h) = it.kind else { continue };
                    let Some(tr) = sums.get(h as usize).and_then(|hs| hs.impl_trait.as_deref()) else { continue };
                    let short = tr.split('<').next().unwrap_or(tr).trim();
                    let short = short.rsplit("::").next().unwrap_or(short);
                    for a in &sm.assocs {
                        let Some(bs) = decl.get(&(short, a.name.as_str())) else { continue };
                        if let Some((x, v)) = crate::ast_summary::assoc_value_unmet(&src, &items[h as usize], it, bs, &is_alias, &lt_bounded) {
                            return Err(format!("{}:1: the type `{v}` does not fulfill the required lifetime (it must outlive `{x}`) (E0477)", it.line));
                        }
                    }
                }
            }
        }
        // (struct / enum / union definitions of the program: unused parameters, union fields ..)
        for (it, sm) in items[..user_src].iter().zip(&sums[..user_src]) {
            if let Some(e) = &sm.adt_err {
                // (a union that derives `Clone` without deriving `Copy` may have an `impl Copy for U {}` of its own)
                if e.ends_with("a union can derive `Clone` only with `Copy`")
                    && sm.unions.iter().any(|u| sums[..user_src].iter().any(|s| s.impl_trait.as_deref() == Some("Copy") && s.impl_self.as_deref() == Some(u.as_str())))
                {
                    continue;
                }
                // (`\u{3}line\u{3}msg`: at that line of the item)
                if let Some((l, m)) = e.strip_prefix('\u{3}').and_then(|r| r.split_once('\u{3}')) {
                    // (an item hoisted out of a fn body: at the line it was written on)
                    let line = it.line + l.parse::<usize>().unwrap_or(1) - 1;
                    let line = crate::prep::hoisted_at(&crate::prep::hoist_marks(&src), line).unwrap_or(line);
                    return Err(format!("{line}:1: {m}"));
                }
                // (`\u{5}word\u{5}msg`: at the line of the first whole word `word` of the item's text)
                if let Some((w, m)) = e.strip_prefix('\u{5}').and_then(|r| r.split_once('\u{5}')) {
                    let text = src.get(it.start..it.end).unwrap_or("");
                    let b = text.as_bytes();
                    let id = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
                    let at = text.match_indices(w).map(|(i, _)| i).find(|&i| (i == 0 || !id(b[i - 1])) && b.get(i + w.len()).is_none_or(|&c| !id(c)));
                    let line = at.map(|i| it.line + text[..i].matches('\n').count()).unwrap_or(it.line);
                    return Err(format!("{line}:1: {m}"));
                }
                // (`\u{1}field\u{1}msg`: at the line of the field `field:`)
                if let Some((f, m)) = e.strip_prefix('\u{1}').and_then(|r| r.split_once('\u{1}')) {
                    let text = src.get(it.start..it.end).unwrap_or("");
                    // (a lifetime is found where it is written, a field by `name:`)
                    let pat = if f.starts_with('\'') || f == "&" { f.to_string() } else { format!("{f}:") };
                    let line = text.find(pat.as_str()).map(|i| it.line + text[..i].matches('\n').count()).unwrap_or(it.line);
                    return Err(format!("{line}:1: {m}"));
                }
                return Err(format!("{}:1: {e}", it.line));
            }
        }
        // (E0658: an attribute behind a feature gate the file does not enable)
        // (attributes start items: only the text at the start of the program's items, not the whole source)
        let mut feats: Option<Vec<String>> = None;
        for it in &items[..user_src] {
            let head = src.get(it.start..it.end).unwrap_or("");
            // (and an extern block's items: `extern "C" { #[unsafe(ffi_const)] fn f(); }`)
            let ext = extern_block(head.as_bytes(), 0);
            if ext || head.starts_with('#') {
                // (an item's outer attributes, not its body; an extern block's items are read whole)
                let end = if ext { Some(it.end) } else { gated_attrs_len(head.as_bytes()).map(|n| it.start + n) };
                if let Some(e) = end.and_then(|end| gated_attr(&src, it.start, end, &mut feats)) {
                    return Err(e);
                }
            }
            // (E0433: an outer attribute's path whose first segment is no tool and no module of the crate's
            // (`#[foo::bar]`), or a module's that has no such attribute (`#[existent::nonexistent]`))
            if head.starts_with("#[") {
                if let Some(e) = attr_path_error(&src, src0, it.start, leading_attrs(head).len(), it.line) {
                    return Err(e);
                }
            }
            // (an unstable ABI: `extern "rust-cold" fn`)
            // (`extern` among the item's first names, `pub unsafe extern "C" fn`, from the lexer's identifiers: no
            // search of its text)
            const KEYWORDS: &[&str] = &["fn", "struct", "enum", "union", "trait", "impl", "type", "const", "static", "mod", "use", "macro_rules"];
            let ids = || it.idents.iter().map(|&(a, b)| src.get(a as usize..b as usize).unwrap_or(""));
            if ids().take_while(|w| !KEYWORDS.contains(w)).any(|w| w == "extern") {
                if let Some(e) = gated_abi(&src, it.start, it.end, &mut feats) {
                    return Err(e);
                }
            }
        }
        // (E0449: a visibility where it means nothing: on an impl, an extern block, a trait's items, a trait impl's
        // items, an enum's variants and their fields. An item's names are looked at by their length first: most are
        // fns with none of these)
        {
            let bytes = src.as_bytes();
            let is = |&(a, b): &(u32, u32), w: &str| (b - a) as usize == w.len() && &bytes[a as usize..b as usize] == w.as_bytes();
            let report = |it: &SrcItem, off: usize| {
                let (line, col) = item_pos(&src, src0, &crate::prep::hoist_marks(&src), it, off);
                format!("{line}:{col}: visibility qualifiers are not permitted here (E0449)")
            };
            for it in &items[..user_src] {
                let text = src.get(it.start..it.end).unwrap_or("");
                let v = match it.kind {
                    crate::lexer::ItemKind::ImplHeader | crate::lexer::ItemKind::TraitMember(_) => leading_pub(text),
                    crate::lexer::ItemKind::ImplMember(h) if sums.get(h as usize).is_some_and(|sm| sm.impl_trait.is_some()) => leading_pub(text),
                    crate::lexer::ItemKind::Normal => {
                        let head = &it.idents[..it.idents.len().min(8)];
                        // (an enum: a `pub` among its names after its own)
                        if let Some(e) = head.iter().position(|x| is(x, "enum")) {
                            if it.idents[e..].iter().any(|x| is(x, "pub")) {
                                if let Some(off) = enum_inner_pub(text) {
                                    return Err(report(it, off));
                                }
                            }
                        }
                        // (`pub extern "C" { .. }`, `pub(crate) unsafe extern "C" { .. }`)
                        if head.iter().any(|x| is(x, "extern")) {
                            leading_pub(text).filter(|&v| {
                                let b = text.as_bytes();
                                let mut j = skip_space(b, v + 3);
                                // (a `pub(..)`'s path, past the comments in it: `pub(in crate /* ) */)`)
                                if b.get(j) == Some(&b'(') {
                                    j += 1;
                                    loop {
                                        j = skip_space(b, j);
                                        match b.get(j) {
                                            Some(b')') => break j += 1,
                                            None => break,
                                            _ => j += 1,
                                        }
                                    }
                                }
                                extern_block(b, skip_space(b, j))
                            })
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(v) = v {
                    return Err(report(it, v));
                }
            }
        }
        // (E0434: a fn written inside another fn that uses one of its locals; only with hoisted items)
        if let Some(e) = first_env_capture(&src, src0, &items[..user_src]) {
            return Err(e);
        }
        // (E0276: an impl's method whose own type parameter has a bound the trait's does not give)
        if let Some(e) = stricter_impl_bound(&src, &items[..user_src]) {
            return Err(e);
        }
        // (E0276: an impl's method whose where clause bounds the impl's lifetimes or types by a lifetime the trait's,
        // read through the impl header's trait arguments, does not imply)
        if let Some(e) = stricter_impl_where(&src, &items[..user_src]) {
            return Err(e);
        }
        // (E0119: two blanket impls of one trait for a bare parameter, `impl<T: A> Tr for T` and `impl<T: B> Tr for T`,
        // overlap whatever their bounds say. Only the program's own traits declared once (modules flatten names),
        // without trait arguments, and not under the features that allow overlapping impls)
        {
            // (a `where` clause only on the parameter itself: `where (): Mirror<Assoc = T>` can make the impls disjoint)
            let plain_where = |ii: usize, st: &str| {
                let it = &items[ii];
                let head = src.get(it.start..it.end).unwrap_or("");
                let head = &head[..head.find('{').unwrap_or(head.len())];
                let w = head.match_indices("where").find(|&(i, _)| {
                    let b = head.as_bytes();
                    (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) && b.get(i + 5).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
                });
                w.is_none_or(|(i, _)| top_level_split(&head[i + 5..]).iter().all(|p| p.trim().is_empty() || p.split(':').next().map(str::trim) == Some(st)))
            };
            let mut first: FxMap<&str, (usize, &str)> = FxMap::default();
            for (ii, sm) in sums[..user_src].iter().enumerate() {
                let (Some(tr), Some(st)) = (sm.impl_trait.as_deref(), sm.impl_self.as_deref()) else { continue };
                // (the self type the bare parameter: not `impl<T> Tr for &T` / `&mut T`, whose self path is T too)
                let bare = matches!(&sm.impl_self_ty, Some(crate::types::TySyn::Named(n)) if n == st);
                if !sm.impl_trait_args.is_empty() || tr.contains(['<', ':']) || !sm.impl_generics.iter().any(|g| g.0 == st) || !bare {
                    continue;
                }
                // (the bare parameter itself, not a reference to it: `impl<T> Tr for &T` and `.. for &mut T` do not overlap)
                if !matches!(&sm.impl_self_ty, Some(crate::types::TySyn::Named(n)) if n == st) {
                    continue;
                }
                // (an impl whose `where` clause may make it disjoint is no candidate either way)
                if !plain_where(ii, st) {
                    continue;
                }
                let Some((_, _)) = first.insert(tr, (ii, st)) else { continue };
                let once = sums[..user_src].iter().filter(|s| s.trait_decl.as_deref() == Some(tr)).count() == 1;
                const OVERLAP: [&str; 6] =
                    ["specialization", "min_specialization", "marker_trait_attr", "negative_impls", "with_negative_coherence", "rustc_attrs"];
                let enabled = feats.get_or_insert_with(|| enabled_features(&src));
                if once && !enabled.iter().any(|f| OVERLAP.contains(&f.as_str())) {
                    return Err(format!("{}:1: conflicting implementations of trait `{tr}` (E0119)", items[ii].line));
                }
            }
        }
        // (E0276: an impl's generic associated type with a `where` clause that its trait's declaration does not have:
        // `type Assoc<T> = Vec<T> where T: Iterator;` for `type Assoc<T>;`. Only a trait declared once)
        {
            let word = |t: &str, w: &str| {
                t.match_indices(w).any(|(i, _)| {
                    let b = t.as_bytes();
                    let id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
                    (i == 0 || !id(b[i - 1])) && b.get(i + w.len()).is_none_or(|&c| !id(c))
                })
            };
            for it in &items[..user_src] {
                let crate::lexer::ItemKind::ImplMember(h) = it.kind else { continue };
                let t = src.get(it.start..it.end).unwrap_or("").trim_start();
                let Some(rest) = t.strip_prefix("type ") else { continue };
                let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                // (a generic one, with a `where` after its value)
                let gat = rest.trim_start()[name.len()..].trim_start().starts_with('<');
                if !gat || !t.split_once('=').is_some_and(|(_, v)| word(v, "where")) {
                    continue;
                }
                let Some(tr) = sums.get(h as usize).and_then(|s| s.impl_trait.as_deref()) else { continue };
                if sums[..user_src].iter().filter(|s| s.trait_decl.as_deref() == Some(tr)).count() != 1 {
                    continue;
                }
                let decl = items[..user_src].iter().find(|d| {
                    matches!(d.kind, crate::lexer::ItemKind::TraitMember(th) if sums.get(th as usize).and_then(|s| s.trait_decl.as_deref()) == Some(tr))
                        && src.get(d.start..d.end).unwrap_or("").trim_start().strip_prefix("type ").is_some_and(|r| {
                            let r = r.trim_start();
                            r.starts_with(name.as_str()) && !r[name.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                        })
                });
                if let Some(d) = decl {
                    let dt = src.get(d.start..d.end).unwrap_or("");
                    if !word(dt, "where") {
                        return Err(format!("{}:1: impl has stricter requirements than trait (E0276)", it.line));
                    }
                    // (both with a `where`: an outlives between the GAT's own lifetimes that the declaration's do not
                    // imply, by their places (`where 'b2: 'a2` for the trait's `where 'a1: 'b1`), at the impl's)
                    let t_full = src.get(it.start..it.end).unwrap_or("");
                    if let Some(off) = gat_stricter_outlives(dt, t_full) {
                        let abs = it.start + off;
                        let line = it.line + t_full[..off].matches('\n').count();
                        let col = abs - src[..abs].rfind('\n').map_or(0, |k| k + 1) + 1;
                        return Err(format!("{line}:{col}: impl has stricter requirements than trait (E0276)"));
                    }
                }
            }
        }
        // (`#![crate_type = "lib"]`: a library, no `main` needed)
        let mut deny_unreachable = false;
        let mut deny_unused_vars = false;
        let mut forbid_unused_vars = false;
        let mut deny_dead_code = false;
        // (`deny(warnings)` / `forbid(warnings)` of the crate: a lint an item or field sets to warn is an error)
        let mut warn_denied = false;
        let mut deny_dead_pub = false;
        let mut deny_unused_imports = false;
        // (unused_imports denied by its own name or its group's, not only through `deny(warnings)`)
        let mut imports_denied = false;
        let mut deny_unreachable_code = false;
        let mut deny_must_use = false;
        let mut deny_unused_alloc = false;
        let mut deny_bare = false;
        let mut deny_unused_mut = false;
        // (unused_parens denied for the crate; or only by the attributes of items of the program's (marked); the
        // crate's levels (parens::denied))
        let mut deny_unused_parens = false;
        let mut marked_parens = false;
        let mut parens_levels = (1u8, false);
        let mut deny_trivial_casts = (false, false);
        let mut deny_unused_lifetimes = (false, false);
        let mut deny_mismatched_lts = false;
        let mut deny_deprecated = false;
        // (improper_ctypes (an `extern` block's fns), improper_ctypes_definitions (an `extern "C" fn`))
        let mut deny_ctypes = (false, false);
        // (non_snake_case, non_camel_case_types, non_upper_case_globals)
        let mut deny_style = [false; 3];
        // (the crate's own text as written, for unused_parens' levels: a library of a workspace is in `pub mod name { .. }`
        // (ws_source), on lines of its own)
        let lib_wrap = self.ctx.lib.as_ref().map(|n| format!("pub mod {n} {{ "));
        let own_src0 = lib_wrap.as_deref().and_then(|w| src0.strip_prefix(w)).unwrap_or(src0);
        let lib_attr = {
            let mut end = src.len().min(4096);
            while !src.is_char_boundary(end) {
                end -= 1;
            }
            let c: String = src[..end].chars().filter(|c| !c.is_whitespace()).collect();
            // (`#![deny(unreachable_patterns)]`: inner attributes come first in the file, so the head is enough)
            deny_unreachable = (c.contains("#![deny(") || c.contains("#![forbid(")) && c.contains("unreachable_patterns");
            // (`#![deny(unused_variables)]` / `deny(unused)` / `deny(warnings)`)
            // (only the lints inside `#![deny(..)]` / `#![forbid(..)]`, not an `#![allow(unused_variables)]`)
            let mut levels: Vec<&str> = vec![];
            for head in ["#![deny(", "#![forbid("] {
                let mut rest = c.as_str();
                while let Some(k) = rest.find(head) {
                    rest = &rest[k + head.len()..];
                    if let Some(e) = rest.find(')') {
                        levels.extend(rest[..e].split(','));
                    }
                }
            }
            // (`#![forbid(unused_variables)]` is not undone by an `#![allow(unused)]`)
            // (a group's `forbid(unused)` lets a member be allowed, with a warning)
            forbid_unused_vars = c.match_indices("#![forbid(").any(|(k, _)| c[k + 10..].split(')').next().is_some_and(|l| l.split(',').any(|l| l == "unused_variables")));
            deny_unused_vars = forbid_unused_vars || (levels.iter().any(|l| matches!(*l, "unused_variables" | "unused" | "warnings")) && !c.contains("#![allow(unused"));
            deny_dead_code = levels.iter().any(|l| matches!(*l, "dead_code" | "unused" | "warnings")) && !c.contains("#![allow(dead_code") && !c.contains("#![expect(dead_code") && !c.contains("#![allow(unused");
            deny_dead_pub = levels.contains(&"dead_code_pub_in_binary");
            deny_unused_imports = levels.iter().any(|l| matches!(*l, "unused_imports" | "unused" | "warnings")) && !c.contains("#![allow(unused");
            deny_unreachable_code = levels.iter().any(|l| matches!(*l, "unreachable_code" | "unused" | "warnings")) && !c.contains("#![allow(unreachable_code");
            deny_must_use = levels.iter().any(|l| matches!(*l, "unused_must_use" | "unused" | "warnings")) && !c.contains("#![allow(unused_must_use");
            // (an `#![allow(..)]` of the lint undoes a deny, as the levels apply in order; forbid cannot be undone)
            // (the last of them wins: `#![allow(x)]` then `#![deny(x)]` denies)
            let lint = |n: &str| {
                // (the last `#![a(..)]` naming n anywhere in its list: `#![allow(dead_code, n)]`)
                let last = |a: &str| {
                    let open = format!("#![{a}(");
                    c.match_indices(&open)
                        .filter(|(i, _)| {
                            let rest = &c[i + open.len()..];
                            let list = &rest[..rest.find(')').unwrap_or(rest.len())];
                            list.split(',').any(|x| x.trim() == n)
                        })
                        .map(|(i, _)| i)
                        .last()
                };
                levels.contains(&n) && (last("forbid").is_some() || last("allow") < last("deny"))
            };
            // (a lint of a group: denied by its name, the group's or `warnings`, unless an allow of its name or the
            // group's comes after that (`#![allow(unused_variables)]` leaves dead_code as it is); forbid stays)
            let last_of = |a: &str, names: &[&str]| -> Option<usize> {
                let open = format!("#![{a}(");
                c.match_indices(&open)
                    .filter(|(i, _)| {
                        let rest = &c[i + open.len()..];
                        let list = &rest[..rest.find(')').unwrap_or(rest.len())];
                        list.split(',').any(|x| names.contains(&x.trim()))
                    })
                    .map(|(i, _)| i)
                    .last()
            };
            // (the level of a lint of a group, by the last of its levels in source order: a forbid of it or its group
            // wins; a deny of it or its group denies; a warn of it (or no level of its own) is denied by
            // `deny(warnings)` or `forbid(warnings)`, which reach only the lints left at warn; an allow or an expect
            // does not deny)
            let grouped = |n: &str, group: &str| {
                if last_of("forbid", &[n, group]).is_some() {
                    return true;
                }
                let own = [n, group];
                let levels = [("deny", last_of("deny", &own)), ("warn", last_of("warn", &own)), ("allow", last_of("allow", &own)), ("expect", last_of("expect", &own))];
                let last = levels.iter().filter_map(|(l, at)| at.map(|at| (at, *l))).max();
                let warnings = last_of("deny", &["warnings"]).is_some() || last_of("forbid", &["warnings"]).is_some();
                match last.map(|(_, l)| l) {
                    Some("deny") => true,
                    Some("warn") | None => warnings,
                    _ => false,
                }
            };
            deny_dead_code = grouped("dead_code", "unused");
            warn_denied = last_of("deny", &["warnings"]).is_some() || last_of("forbid", &["warnings"]).is_some();
            imports_denied = last_of("forbid", &["unused_imports", "unused"]).is_some() || {
                let own = ["unused_imports", "unused"];
                let levels = [("deny", last_of("deny", &own)), ("warn", last_of("warn", &own)), ("allow", last_of("allow", &own)), ("expect", last_of("expect", &own))];
                levels.iter().filter_map(|(l, at)| at.map(|at| (at, *l))).max().is_some_and(|(_, l)| l == "deny")
            };
            deny_trivial_casts = (lint("trivial_casts"), lint("trivial_numeric_casts"));
            deny_unused_lifetimes = (lint("unused_lifetimes"), lint("single_use_lifetimes"));
            deny_mismatched_lts = lint("mismatched_lifetime_syntaxes");
            deny_deprecated = lint("deprecated") || (levels.contains(&"warnings") && !c.contains("#![allow(deprecated"));
            // (warn by default: the crate attribute naming the lint last sets its level, and `deny(warnings)` makes
            // it an error while it is left at warn, unless a later attribute sets `warnings` back; `allow(x)` then
            // `warn(x)` puts it back at warn)
            let last_level = |names: &[&str]| {
                ["forbid", "deny", "warn", "allow", "expect"].into_iter().filter_map(|l| last_of(l, names).map(|at| (at, l))).max().map(|(_, l)| l)
            };
            let ffi = |n: &str| match last_level(&[n]) {
                Some("forbid" | "deny") => true,
                Some("warn") | None => matches!(last_level(&["warnings"]), Some("forbid" | "deny")),
                _ => false,
            };
            deny_ctypes = (ffi("improper_ctypes"), ffi("improper_ctypes_definitions"));
            // (a trait object without `dyn` before 2021: the lint warns, as rust_2018_idioms does. The crate's level
            // is its attributes' last one; an item's `#[deny(bare_trait_objects)]` is taken for the crate, as lint
            // levels below it are not followed)
            // (read from the tokens, so a string or a comment that holds such an attribute is not one; only when the
            // head names the lint at all)
            let item_deny = c.contains("bare_trait_objects") && {
                let h = &src.as_bytes()[..end];
                let t = crate::lexer::tokenize(h);
                let tx = |k: usize| t.get(k).map(|&(_, a, b)| &h[a..b]);
                (0..t.len()).any(|k| {
                    tx(k) == Some(b"#") && tx(k + 1) == Some(b"[") && matches!(tx(k + 2), Some(b"deny" | b"forbid")) && tx(k + 3) == Some(b"(") && {
                        (k + 4..t.len()).take_while(|&j| tx(j) != Some(b")")).any(|j| t[j].0 == 0 && tx(j) == Some(b"bare_trait_objects"))
                    }
                })
            };
            deny_bare = item_deny || match last_level(&["bare_trait_objects", "rust_2018_idioms"]) {
                Some("forbid" | "deny") => true,
                Some("warn") | None => matches!(last_level(&["warnings"]), Some("forbid" | "deny")),
                _ => false,
            };
            deny_unused_alloc = lint("unused_allocation")
                || (!levels.contains(&"unused_allocation") && levels.iter().any(|l| matches!(*l, "unused" | "warnings")) && !c.contains("#![allow(unused"));
            deny_unused_mut = lint("unused_mut") || (!levels.contains(&"unused_mut") && levels.iter().any(|l| matches!(*l, "unused" | "warnings")) && !c.contains("#![allow(unused"));
            // (unused_parens: the crate's level of it and whether `warnings` is denied, which makes it an error while it
            // is at warn (parens::crate_levels, from the head's inner attributes in order). Read where the head denies
            // something, or the attributes an item of the program's starts with do (`#[deny(unused_parens)] fn f`):
            // those items are checked then, under the crate's levels)
            let marked = || {
                items[..user_src].iter().any(|it| {
                    src.as_bytes().get(it.start) == Some(&b'#')
                        && src.get(it.start..it.end).is_some_and(|t| {
                            let head = leading_attrs(t);
                            (head.contains("deny") || head.contains("forbid")) && (head.contains("unused") || head.contains("warnings"))
                        })
                })
            };
            if !levels.is_empty() || marked() {
                parens_levels = parens::crate_levels(own_src0);
                deny_unused_parens = parens::denied(parens_levels);
                marked_parens = !deny_unused_parens && marked();
            }
            for (k, n) in ["non_snake_case", "non_camel_case_types", "non_upper_case_globals"].iter().enumerate() {
                deny_style[k] = levels.iter().any(|l| l == n || *l == "nonstandard_style" || *l == "warnings") && !c.contains(&format!("#![allow({n}"));
            }
            ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"].iter().any(|k| c.contains(&format!("#![crate_type=\"{k}\"]")))
        };
        // (unused_parens: the first parentheses rustc finds unnecessary in the crate's own fns and consts (not its
        // dependencies', after its own lines), where the lint is denied; an early lint, reported before the others.
        // The levels the items' text does not show, of the modules, impls and traits around them, are read from the
        // source as written)
        if deny_unused_parens || marked_parens {
            let ol = self.ctx.own_lines as usize;
            let own0 = if ol == 0 { own_src0 } else { own_src0.match_indices('\n').nth(ol - 1).map_or(own_src0, |(k, _)| &own_src0[..k]) };
            let own0 = if lib_wrap.is_some() { own0.trim_end().strip_suffix('}').unwrap_or(own0) } else { own0 };
            if let Some(around) = parens::levels_around(own0) {
                if let Some(e) = parens::first_unused(&src, src0, &items[..user_src], ol, parens_levels, &around) {
                    return Err(e);
                }
            }
        }
        // (dead_code, in a binary: the first fn / struct / enum / const / static / trait of the program's whose name
        // appears nowhere but in its own definition and in `impl` headers)
        if deny_dead_code && self.ctx.lib.is_none() {
            // (a `pub` item counts as used, except in a binary under `deny(dead_code_pub_in_binary)`)
            if let Some((line, msg)) = first_dead_item(&src, &items[..user_src], deny_dead_pub && !lib_attr, warn_denied) {
                return Err(format!("{line}:1: {msg}"));
            }
            // (a field nothing reads, a variant nothing constructs, an inherent impl's const or fn nothing names)
            if let Some((line, msg)) = dead::first_dead_part(&src, &items[..user_src], warn_denied) {
                return Err(format!("{line}:1: {msg}"));
            }
        }
        // (unused_imports: a top-level `use` whose name nothing else mentions)
        if deny_unused_imports && self.ctx.lib.is_none() {
            if let Some((line, col, msg)) = first_unused_import(&src, &items[..user_src], imports_denied, warn_denied) {
                return Err(format!("{line}:{col}: {msg}"));
            }
        }
        // (naming lints: the first item whose name breaks its case convention)
        if deny_style.iter().any(|&d| d) && self.ctx.lib.is_none() {
            if let Some((line, msg)) = first_bad_name(&src, &items[..user_src], deny_style) {
                return Err(format!("{line}:1: {msg}"));
            }
        }
        // (unreachable_code: a statement after one that diverges)
        if deny_unreachable_code && self.ctx.lib.is_none() {
            // (the source as written, as flattening blanks a `mod process {` header: `mod`, any space, `process`)
            let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
            let b0 = src0.as_bytes();
            let user_process = src0.match_indices("process").any(|(i, _)| {
                let before = src0[..i].trim_end();
                i > before.len() && before.ends_with("mod") && !b0.get(i + 7).is_some_and(|&c| word(c)) && !before[..before.len() - 3].bytes().last().is_some_and(word)
            });
            for it in &items[..user_src] {
                let text = src.get(it.start..it.end).unwrap_or("");
                if !text.contains("fn ") || text.contains("allow(unreachable") || text.contains("#[cfg") {
                    continue;
                }
                let Ok(Some(f)) = parse_fn(&src, it) else { continue };
                if let Some((p, what)) = first_unreachable(&f.block, user_process) {
                    if p.line > 0 {
                        return Err(format!("{}:{}: unreachable {what}", it.line + p.line as usize - 1, p.col + 1));
                    }
                }
            }
        }
        // (improper_ctypes: a C string type of std's, `CStr` / `CString`, in an `extern "C"` block's or fn's signature)
        if deny_ctypes.0 || deny_ctypes.1 {
            if let Some(e) = first_cstr_in_extern(&src, &items[..user_src], deny_ctypes) {
                return Err(e);
            }
        }
        // (deprecated: a path through `std::intrinsics` to what is stable elsewhere (`std::mem::transmute`))
        if deny_deprecated {
            if let Some(e) = first_accidentally_stable(&src, &items[..user_src]) {
                return Err(e);
            }
        }
        // (unused_lifetimes: a lifetime parameter of a fn or an impl of the program's that nothing in it names;
        // single_use_lifetimes: one named once, in a parameter's type (or the impl's header), which eliding it would
        // say)
        // (mismatched_lifetime_syntaxes: a lifetime of a signature's inputs its output writes another way. Denied for
        // the crate, or for the items that deny it (`#[deny(mismatched_lifetime_syntaxes)] fn f`))
        let marked_mls = !deny_mismatched_lts && src.contains("mismatched_lifetime_syntaxes");
        if deny_mismatched_lts || marked_mls {
            let mut adts: crate::fxhash::FxMap<&str, usize> = Default::default();
            for sm in &sums[..user_src] {
                if let Some(r) = sm.adt_regions.as_deref().filter(|r| !r.lts.is_empty()) {
                    for n in sm.structs.iter().map(|s| s.0.as_str()).chain(sm.enums.iter().map(|e| e.0.as_str())) {
                        adts.insert(n.rsplit("::").next().unwrap_or(n), r.lts.len());
                    }
                }
            }
            if let Some(e) = first_mismatched_lifetime_syntax(&src, &items[..user_src], &adts, marked_mls) {
                return Err(e);
            }
        }
        if deny_unused_lifetimes.0 || deny_unused_lifetimes.1 {
            if let Some(e) = first_unused_lifetime(&src, &items[..user_src], deny_unused_lifetimes) {
                return Err(e);
            }
        }
        // (unused_variables: the first binding nothing reads, in the program's own fns, where the lint is denied: for
        // the crate, or by an attribute of the fn (`#[deny(unused)] fn main`) or of the statement or arm the binding
        // is in; an `allow` there covers it (`#[allow(unused_variables)] let x = 1;`), one of the crate's or the fn's
        // all of the fn)
        // (or an item of the program's whose own attributes deny it: an item starts with its attributes. A `deny` on a
        // statement or an arm alone, with none on an item, is not seen: finding one would read all of the source)
        let scoped_vars = !deny_unused_vars
            && items[..user_src].iter().any(|it| {
                src.as_bytes().get(it.start) == Some(&b'#') && src.get(it.start..it.end).is_some_and(|t| t.starts_with("#[")) && {
                    let head = leading_attrs(&src[it.start..it.end]);
                    (head.contains("deny") || head.contains("forbid")) && lint_attrs(head, "unused_variables").iter().any(|a| a.2 > 0)
                }
            });
        if deny_unused_vars || scoped_vars {
            for it in &items[..user_src] {
                let text = src.get(it.start..it.end).unwrap_or("");
                if !text.contains("fn ") || text.contains("#[cfg") {
                    continue;
                }
                let attrs = if text.contains("unused") || text.contains("warnings") { lint_attrs(text, "unused_variables") } else { vec![] };
                if !deny_unused_vars && !attrs.iter().any(|a| a.2 > 0) {
                    continue;
                }
                // (the level at a place of the item's text: the crate's, then each attribute that covers it, in order)
                let denied = |o: usize| {
                    let (mut on, mut forbid) = (deny_unused_vars, forbid_unused_vars);
                    for &(_, _, l, _) in attrs.iter().filter(|a| a.0 <= o && o < a.1) {
                        if l == 2 {
                            on = true;
                            forbid = true;
                        } else if !forbid {
                            on = l == 1;
                        }
                    }
                    on
                };
                let Ok(Some(f)) = parse_fn(&src, it) else { continue };
                let params: Vec<&Pat> = f.sig.params.iter().map(|(p, _)| p).collect();
                let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
                let sig_at = text.find("fn ").unwrap_or(0);
                for (pos, n) in crate::ast_closure::unused_bindings(&params, &f.block) {
                    let at = match pos {
                        Some(p) if p.line > 0 => starts.get(p.line as usize - 1).map_or(sig_at, |&l| l + p.col as usize),
                        _ => sig_at,
                    };
                    // (at the binding itself, the name's first whole word from there: a field of a pattern may have
                    // an attribute of its own)
                    let tb = text.as_bytes();
                    let id = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
                    let own = text.get(at..).and_then(|r| r.match_indices(n.as_str()).map(|(i, _)| at + i).find(|&i| (i == 0 || !id(tb[i - 1])) && tb.get(i + n.len()).is_none_or(|&c| !id(c))));
                    if !denied(own.unwrap_or(at)) {
                        continue;
                    }
                    // (reported there when it is on the line of the binding's statement, arm or condition)
                    if let Some(o) = own.filter(|&o| !text[at.min(o)..o].contains('\n')) {
                        let abs = it.start + o;
                        let col = abs - src[..abs].rfind('\n').map_or(0, |p| p + 1) + 1;
                        return Err(format!("{}:{col}: unused variable: `{n}`", it.line + text[..o].matches('\n').count()));
                    }
                    let (line, col) = match pos {
                        Some(p) if p.line > 0 => (it.line + p.line as usize - 1, p.col as usize + 1),
                        _ => (it.line, 1),
                    };
                    return Err(format!("{line}:{col}: unused variable: `{n}`"));
                }
            }
        }
        unsafe_impls(&src, &items[..user_src])?;
        let need_main = self.ctx.lib.is_none() && !lib_attr;
        // (keep_for_edits: a build that reads the same as one kept builds the same program; the copy is the kept one's)
        let prog_fp = self.prog_keep.as_ref().map(|_| prog_fp(iface_fp, &items, need_main, user, self.opts.edition, deny_bare));
        let kept = match (&self.prog_keep, prog_fp) {
            (Some(ProgKeep { prog: Some((fp, p)), .. }), Some(f)) if *fp == f => Some(p.clone()),
            _ => None,
        };
        let mut prog = match kept {
            Some(p) => p,
            None => Program::build_with_edition(&items, &mut sums, need_main, user, self.opts.edition, deny_bare).map_err(|e| name_line(&e, &src, &items))?,
        };
        if let (Some(k), Some(f)) = (&mut self.prog_keep, prog_fp) {
            if k.last == f && k.prog.as_ref().is_none_or(|(fp, _)| *fp != f) {
                k.prog = Some((f, prog.clone()));
            }
            k.last = f;
        }
        tm.lap("program", String::new());
        prog.prelude_from = user as u32;
        if !prog.variant_shadowed.is_empty() {
            imported_prelude_variants(&mut prog, &src, &items[..user]);
        }
        if let Some(e) = cross_kind_duplicate(&prog, &src, &items, &sums, user) {
            return Err(e);
        }
        // (E0405 / E0404: a fn's trait bound that names no trait in scope; a single file's, whose names are all its own
        // or std's)
        if self.ctx.map.inputs.is_empty() && self.ctx.deps.is_empty() {
            if let Some(e) = unknown_trait_bound(&prog, &src, src0, &items, user) {
                return Err(e);
            }
            if let Some(e) = unknown_impl_type(&prog, &src, src0, &items, user) {
                return Err(e);
            }
        }
        // (E0191 at a `dyn Tr` in a type that leaves one of the trait's associated types without a value; before
        // E0038, as rustc)
        // (one look at the program's own text for `dyn`, for this and E0038's check)
        let has_dyn = items[..user].last().is_some_and(|l| src[..l.end.min(src.len())].contains("dyn"));
        if let Some(e) = has_dyn.then(|| first_dyn_unspecified_assoc(&src, src0, &items[..user])).flatten() {
            return Err(e);
        }
        // (in a check (zustc check, the LSP): a build stays as fast as it was, and reports it when it makes the vtable)
        if let Some(e) = (self.checking && has_dyn).then(|| dyn_in_signature(&prog, &src, &items)).flatten() {
            // (the program's own `dyn` scan, when it found one: at the `dyn`, naming the trait rustc names)
            return Err(late_err.take().filter(|l| l.ends_with("is not dyn compatible (E0038)")).unwrap_or(e));
        }
        // (E0080: a const's or a static's value that reads an extern static, `static X: u32 = symbol;`, which constant
        // evaluation cannot; rustc's E0133 is on the same line)
        if let Some((ci, n)) = extern_static_in_value(&prog) {
            let line = items.get(prog.consts[ci].item as usize).map_or(0, |it| it.line);
            return Err(format!("{line}:1: cannot access extern static `{n}` (E0080)"));
        }
        // (traits of one name in two modules: which one a method call means depends on where it is. Read from the
        // crate's source (its files joined, as the items' lines count); a build with dependency crates keeps the earlier
        // choice, the first impl)
        // (whether the source has modules: one search, for trait scopes and for has_mods below)
        // (the keyword, not `mode`: a `mod` not after an identifier's letter and followed by a space)
        let has_mod_word = src0.contains("mod") && {
            let b = src0.as_bytes();
            src0.match_indices("mod").any(|(i, _)| {
                (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) && b.get(i + 3).is_some_and(|c| c.is_ascii_whitespace())
            })
        };
        if self.ctx.deps.is_empty() && self.ctx.lib.is_none() {
            // (src0's tokens when they were read for the module scans: tokenizing a large text again was ~2% of an edit)
            prog.trait_scopes = lexer::trait_scopes_in(src0, has_mod_word, src0_toks.as_deref());
            if !prog.trait_scopes.is_empty() {
                prog.item_lines = items.iter().map(|it| it.line as u32).collect();
            }
        }
        prog.overflow_checks = !self.opts.opt;
        prog.deny_unreachable = deny_unreachable;
        // (the program has modules: read from its source as written, before flattening blanks their headers)
        prog.has_mods = has_mod_word;
        prog.field_priv = fpriv.take();
        prog.lower_consts = prog.consts.iter().any(|c| !c.name.contains("::") && c.name.trim_start_matches('_').starts_with(|ch: char| ch.is_lowercase()));
        prog.deny_must_use = deny_must_use;
        prog.edition = self.opts.edition;
        prog.path_lts = crate::regions::src_path_lts(src0);
        prog.deny_unused_alloc = deny_unused_alloc;
        // (not where an item or a statement may allow it: any `allow` / `warn` / `expect` of unused_mut, unused or
        // warnings in the source turns it off, as the borrow check does not follow lint levels below the crate)
        // (an attribute's: after `#[` / `#![` (not the word in a comment, `// allow (unused_mut)`), also with spaces
        // before the `(`: `#[allow (unused_mut)]`)
        prog.deny_unused_mut = deny_unused_mut && !["allow", "warn", "expect"].iter().any(|a| {
            src0.match_indices(a).any(|(i, _)| {
                let attr = src0[..i].trim_end().strip_suffix('[').map(str::trim_end).is_some_and(|p| p.ends_with('#') || p.strip_suffix('!').is_some_and(|q| q.trim_end().ends_with('#')));
                if !attr {
                    return false;
                }
                let Some(rest) = src0[i + a.len()..].trim_start().strip_prefix('(') else { return false };
                let list = &rest[..rest.find(')').unwrap_or(rest.len())];
                list.split(',').any(|x| matches!(x.trim(), "unused_mut" | "unused" | "warnings"))
            })
        });
        // (an `allow` / `warn` / `expect` of it anywhere, at the crate or an item: not denied there, so not checked)
        // (one search of the source for the name, then the attribute around each place it is written)
        // (not the name in a string or after `//` on its line: `"#[allow(overflowing_literals)]"` allows nothing)
        let in_text = |i: usize| {
            let line = &src[src[..i].rfind('\n').map_or(0, |k| k + 1)..i];
            let (mut quoted, mut esc, mut prev) = (false, false, ' ');
            for c in line.chars() {
                match c {
                    _ if esc => esc = false,
                    '\\' if quoted => esc = true,
                    '"' => quoted = !quoted,
                    '/' if !quoted && prev == '/' => return true,
                    _ => {}
                }
                prev = c;
            }
            quoted
        };
        prog.allow_overflowing_literals = src.match_indices("overflowing_literals").any(|(i, _)| {
            let head = &src[src[..i].rfind('(').map(|k| src[..k].rfind(|c: char| c == '[' || c == ',').map_or(0, |b| b + 1)).unwrap_or(i)..i];
            ["allow", "warn", "expect"].iter().any(|a| head.trim_start().strip_prefix(a).is_some_and(|rest| rest.trim_start().starts_with('('))) && !in_text(i)
        });
        prog.allow_implicit_autorefs = src.match_indices("dangerous_implicit_autorefs").any(|(i, _)| {
            let head = &src[src[..i].rfind('(').map(|k| src[..k].rfind(|c: char| c == '[' || c == ',').map_or(0, |b| b + 1)).unwrap_or(i)..i];
            ["allow", "warn", "expect"].iter().any(|a| head.trim_start().strip_prefix(a).is_some_and(|rest| rest.trim_start().starts_with('('))) && !in_text(i)
        });
        prog.deny_trivial_casts = deny_trivial_casts;
        // (`#![feature(fn_traits)]`, from the crate's attributes)
        prog.fn_traits = items[..user].iter().any(|it| it.kind == crate::lexer::ItemKind::InnerAttr && src.get(it.start..it.end).is_some_and(|t| t.contains("fn_traits")));
        // (bindings_with_variant_name (E0170): the names the program's `use`s go through, for which a variant of the
        // name may be in scope)
        for it in &items[..user] {
            if !matches!(src.as_bytes().get(it.start), Some(b'u' | b'p')) {
                continue;
            }
            let t = src.get(it.start..it.end).unwrap_or("");
            let t = t.strip_prefix("pub ").unwrap_or(t).trim_start();
            let Some(rest) = t.strip_prefix("use ") else { continue };
            let b = rest.as_bytes();
            let mut k = 0;
            while let Some(d) = rest[k..].find("::") {
                let end = k + d;
                let start = rest[..end].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
                if start < end && b[start].is_ascii_uppercase() {
                    prog.use_prefixes.push(rest[start..end].to_string());
                }
                k = end + 2;
            }
        }
        if deny_must_use {
            for it in &items[..user] {
                let text = src.get(it.start..it.end).unwrap_or("");
                if let Some(k) = text.find("#[must_use") {
                    let after = &text[k..];
                    if let Some(f) = after.find("fn ") {
                        let name: String = after[f + 3..].trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                        if !name.is_empty() {
                            prog.must_use_fns.push(name);
                        }
                    }
                }
            }
        }
        prog.opt = self.opts.opt;
        resolve_opaque(&mut prog, &src, &items)?;
        // (the words of the program's `use` declarations, fn bodies' too: from the lexer's identifier tokens, the
        // `use` keyword's to its `;`)
        // (one search of the source first: most programs have no `use`)
        let sb = src.as_bytes();
        // (only `zustc check` has the placeholders' round that reads them)
        // (the keyword and any space after it: `use` then a newline is one too)
        // (the SIMD `contains` first: a source with no "use" at all costs one fast scan, not a byte-by-byte search)
        let any_use = self.checking && src.contains("use") && src.match_indices("use").any(|(i, _)| sb.get(i + 3).is_some_and(|c| c.is_ascii_whitespace()));
        for it in items[..user].iter().filter(|_| any_use) {
            for &(a, b) in &it.idents {
                let (a, b) = (a as usize, b as usize);
                if b - a != 3 || sb.get(a..b) != Some(b"use") {
                    continue;
                }
                let end = use_end(sb, b, it.end.min(sb.len()));
                // (its identifier tokens: not the words of a comment in it)
                prog.use_names.extend(it.idents.iter().map(|&(x, y)| (x as usize, y as usize)).filter(|&(x, y)| x > b && y <= end).filter_map(|(x, y)| src.get(x..y)).map(str::to_string));
            }
        }
        // (the fmt traits the program's `use` declarations import, for `x.fmt(f)` of a type with both (#256): only when
        // the program calls a `fmt` method)
        let user_end = items[..user].last().map_or(0, |it| it.end.min(src.len()));
        if src[..user_end].contains(".fmt") {
            prog.fmt_imports = fmt_imports(&src, &items[..user]);
        }
        // (a field or an impl's self type that names a struct of the program's whose parameter's bound does not hold:
        // `struct Hoge<K> { s: S<K> }` with `struct S<T: Tr>`)
        // (only when one of them has a bounded parameter)
        // (or a trait of the program's with a bounded parameter: `trait Getter<T: Clone2>`)
        if sums[..user.min(sums.len())].iter().any(|s| !s.adt_bounds.is_empty() || (s.trait_decl.is_some() && s.trait_generics.iter().any(|g| !g.0.starts_with('#') && g.0 != "Self" && !g.1.is_empty()))) {
            if let Some(e) = wf_error(&prog, &sums, &src, &items, user) {
                return Err(e);
            }
        }
        if let Some(e) = sized_error(&sums, &src, &items, user) {
            return Err(e);
        }
        if let Some(e) = sized_wf_error(&prog, &sums, &src, &items, user) {
            return Err(e);
        }
        // (E0119: two impls of a trait of the program's that both apply to one goal, by the trait solver: their heads
        // unify and both impls' where clauses hold there (crate::traits::TraitDb::conflict))
        // (an impl's where clauses, from its header's text: the summary keeps those on one of its parameters (`T: Tr`);
        // the others (`where T::Assoc: Foo`, `where (): Array<Element = &()>`) are read here and added to its clause.
        // read_all: every where clause of the impl was read)
        let mut where_all: FxMap<usize, bool> = FxMap::default();
        // (the impls with a where clause on another type than a parameter, `where Box<[T]>: IntoWasmAbi`: the
        // supertrait check below does not assume it)
        let mut where_other: FxSet<usize> = FxSet::default();
        for (ii, sm) in sums.iter().enumerate().take(user.min(items.len())) {
            if sm.impl_self_ty.is_none() || !matches!(items[ii].kind, ItemKind::Normal | ItemKind::ImplHeader) {
                continue;
            }
            let text = blank_comments(src.get(items[ii].start..items[ii].end).unwrap_or(""));
            let head = text.split('{').next().unwrap_or("");
            let Some(preds) = where_preds(head) else { continue };
            let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
            let mut all = true;
            let mut extra = vec![];
            for (subj, bounds) in &preds {
                if subj.starts_with('\'') || gn.contains(&subj.as_str()) {
                    continue;
                }
                let ty = if subj.starts_with("for") && subj[3..].trim_start().starts_with('<') {
                    None
                } else {
                    crate::ast_summary::ty_of_text(subj, sm.impl_self_ty.as_ref()).and_then(|t| prog.resolve_g(&t, &gn).ok())
                };
                let Some(ty) = ty else {
                    all = false;
                    continue;
                };
                let bs: Vec<&str> = bounds.iter().map(|b| b.as_str()).filter(|b| !b.starts_with('\'')).collect();
                let ps = crate::traits::preds_of(&prog, ty, &bs, &gn);
                if ps.len() != bs.iter().filter(|b| !b.starts_with('?')).count() {
                    all = false;
                }
                extra.extend(ps);
            }
            if !extra.is_empty() || !all {
                where_other.insert(ii);
            }
            prog.tdb.add_where(ii, extra, all);
            where_all.insert(ii, all);
        }
        let read_all = |ii: usize| -> bool { where_all.get(&ii).copied().unwrap_or(true) };
        // (the auto traits among the program's traits' supertraits, which the summaries leave out: `trait Foo: Send`,
        // `trait Foo where Self: Sync`, from the header's text)
        if !prog.tdb.user.is_empty() {
            for (ii, sm) in sums.iter().enumerate().take(user.min(items.len())) {
                let Some(tr) = sm.trait_decl.as_deref() else { continue };
                let text = blank_comments(src.get(items[ii].start..items[ii].end).unwrap_or(""));
                let head = text.split('{').next().unwrap_or("");
                // (after the name and its parameters: the supertraits and the where clause)
                let Some(at) = head.find(tr) else { continue };
                let rest = &head[at + tr.len()..];
                let mut depth = 0i32;
                let mut cut = rest.len();
                for (i, c) in rest.char_indices() {
                    match c {
                        '<' => depth += 1,
                        '>' => depth -= 1,
                        ':' if depth == 0 => {
                            cut = i;
                            break;
                        }
                        _ => {}
                    }
                }
                let sup = &rest[cut.min(rest.len())..];
                let word = |w: &str| sup.match_indices(w).any(|(i, _)| {
                    let ok = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                    ok(sup[..i].chars().next_back()) && ok(sup[i + w.len()..].chars().next())
                });
                let autos: Vec<String> = ["Send", "Sync"].iter().filter(|w| word(w)).map(|w| w.to_string()).collect();
                if !autos.is_empty() {
                    prog.tdb.auto_supers.insert(tr.to_string(), autos);
                }
            }
        }
        // (E0277: a static is reachable from every thread, so its type must be Sync. After the auto traits of the
        // program's traits' supertraits are known: a `&dyn Stage` of `trait Stage: Sync` is Sync)
        if let Some((ci, tn)) = crate::acg::static_not_sync(&prog) {
            let line = items.get(prog.consts[ci].item as usize).map_or(0, |it| it.line);
            return Err(format!("{line}:1: `{tn}` cannot be shared between threads safely: the type of a `static` must be Sync (E0277)"));
        }
        // (E0277: an impl of a trait of the program's needs its supertraits, all the way up, of its self type, with the
        // impl's parameters as placeholders whose bounds are all the impl assumes (`impl<T> Complete for T` with `trait
        // Complete: Partial` and `trait Partial: Copy` needs `T: Copy`))
        if !prog.tdb.user.is_empty() {
            for (ii, sm) in sums.iter().enumerate().take(user.min(items.len())) {
                let (Some(tr), Some(st)) = (sm.impl_trait.as_deref(), sm.impl_self_ty.as_ref()) else { continue };
                let tr = tr.split('<').next().unwrap_or(tr).trim();
                let tr = tr.rsplit("::").next().unwrap_or(tr);
                if !prog.tdb.user.contains(tr) || !matches!(items[ii].kind, ItemKind::Normal | ItemKind::ImplHeader) || !read_all(ii) {
                    continue;
                }
                // (the supertraits, transitively: the program's traits' recorded ones, and the markers (Copy, Clone))
                let mut supers: Vec<String> = vec![];
                let mut todo: Vec<String> = vec![tr.to_string()];
                while let Some(t) = todo.pop() {
                    for sup in prog.trait_supers.get(&t).into_iter().flatten().chain(prog.trait_marker_supers.get(&t).into_iter().flatten()) {
                        let sup = sup.trim();
                        if sup.starts_with('#') || sup.contains('<') || sup.contains('(') || supers.iter().any(|x| x == sup) || sup == tr {
                            continue;
                        }
                        supers.push(sup.to_string());
                        if prog.tdb.user.contains(sup) {
                            todo.push(sup.to_string());
                        }
                    }
                    for a in prog.tdb.auto_supers.get(&t).into_iter().flatten() {
                        if !supers.contains(a) {
                            supers.push(a.clone());
                        }
                    }
                }
                if supers.is_empty() {
                    continue;
                }
                let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                let Ok(t) = prog.resolve_g(st, &gn) else { continue };
                let rigid: Vec<Ty> = sm.impl_generics.iter().enumerate().map(|(k, (n, bs))| Ty::Struct(prog.placeholder(u32::MAX - ii as u32, k as u32, n, bs))).collect();
                let t = prog.subst(t, &rigid);
                // (a supertrait the impl's own where clause assumes (`impl<T> Child for W<T> where W<T>: Parent`): given)
                let assumed: Vec<String> = {
                    let text = blank_comments(src.get(items[ii].start..items[ii].end).unwrap_or(""));
                    let head = text.split('{').next().unwrap_or("");
                    // (only a predicate on the impl's self type itself (or `Self`): `where T: Parent` gives nothing
                    // about `W<T>`)
                    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                    let self_txt = head.rfind(" for ").map(|i| {
                        let rest = &head[i + 5..];
                        let ok = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                        let end = rest.match_indices("where").find(|(j, _)| ok(rest[..*j].chars().next_back()) && ok(rest[j + 5..].chars().next())).map_or(rest.len(), |(j, _)| j);
                        squash(&rest[..end])
                    });
                    where_preds(head)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|(subj, _)| {
                            let subj = squash(subj);
                            subj == "Self" || self_txt.as_deref() == Some(subj.as_str())
                        })
                        .flat_map(|(_, bs)| bs)
                        .map(|b| {
                            let b = b.trim().split('<').next().unwrap_or("").trim().to_string();
                            b.rsplit("::").next().unwrap_or(&b).to_string()
                        })
                        .collect()
                };
                for sup in &supers {
                    if assumed.contains(sup) {
                        continue;
                    }
                    // (an impl whose where clause is on another type, `impl<T> IntoWasmAbi for Vec<T> where Box<[T]>:
                    // IntoWasmAbi`: `Vec<T>: WasmDescribe` may hold through that clause (`impl<T> WasmDescribe for Vec<T>
                    // where Box<[T]>: WasmDescribe`, a supertrait of IntoWasmAbi), which holds_rigid does not assume: not
                    // reported)
                    if where_other.contains(&ii) {
                        continue;
                    }
                    if prog.tdb.holds_rigid(&prog, t, sup) == crate::traits::Ans::No {
                        let n = crate::traits::display_ty(&prog, t);
                        return Err(format!("{}:1: {} (E0277)", items[ii].line, crate::traits::not_held(&n, sup)));
                    }
                }
            }
        }
        // (E0277: a trait's method that names `<Self as Tr>::X` needs `Self: Tr`: the trait itself, one of its
        // supertraits (all the way up), or the method's own `where Self: Tr`; a trait of the program's)
        if !prog.tdb.user.is_empty() {
            for (ii, it) in items.iter().enumerate().take(user.min(items.len())) {
                let ItemKind::TraitMember(h) = it.kind else { continue };
                let Some(own) = sums.get(h as usize).and_then(|s| s.trait_decl.clone()) else { continue };
                let text = blank_comments(src.get(it.start..it.end).unwrap_or(""));
                let head = text.split('{').next().unwrap_or("").split(';').next().unwrap_or("");
                let mut supers: Vec<String> = vec![own.clone()];
                // (the trait's own `where Self: Parent`, a supertrait as `trait Child: Parent` is)
                if let Some(hi) = items.get(h as usize) {
                    let htext = blank_comments(src.get(hi.start..hi.end).unwrap_or(""));
                    let hhead = htext.split('{').next().unwrap_or("");
                    for (subj, bs) in where_preds(hhead).unwrap_or_default() {
                        if subj == "Self" {
                            for b in bs {
                                let b = b.trim().split('<').next().unwrap_or("").trim();
                                let b = b.rsplit("::").next().unwrap_or(b).to_string();
                                if !supers.contains(&b) {
                                    supers.push(b);
                                }
                            }
                        }
                    }
                }
                let mut k = 0;
                while k < supers.len() {
                    let t = supers[k].clone();
                    for sup in prog.trait_supers.get(&t).into_iter().flatten() {
                        let n = sup.split('<').next().unwrap_or(sup).trim();
                        let n = n.rsplit("::").next().unwrap_or(n).to_string();
                        if !n.starts_with('#') && !supers.contains(&n) {
                            supers.push(n);
                        }
                    }
                    k += 1;
                }
                for (at, _) in head.match_indices("<Self as ") {
                    let rest = &head[at + 9..];
                    let n: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':').collect();
                    let n = n.rsplit("::").next().unwrap_or(&n).to_string();
                    if !prog.tdb.user.contains(&n) || supers.contains(&n) {
                        continue;
                    }
                    // (the method's own where clause: `where Self: Tr`, or a trait of it whose supertraits (all the
                    // way up) have Tr: `where Self: Child` with `trait Child: Parent`)
                    let mut own_bounds: Vec<String> = vec![];
                    // (split with nesting kept: `Self: Pair<u8, u16> + Parent`)
                    for (subj, bs) in where_preds(head).unwrap_or_default() {
                        if subj != "Self" {
                            continue;
                        }
                        for x in bs {
                            let b = x.trim().split('<').next().unwrap_or("").trim();
                            own_bounds.push(b.rsplit("::").next().unwrap_or(b).to_string());
                        }
                    }
                    let mut k = 0;
                    while k < own_bounds.len() {
                        let t = own_bounds[k].clone();
                        for sup in prog.trait_supers.get(&t).into_iter().flatten() {
                            let b = sup.split('<').next().unwrap_or(sup).trim();
                            let b = b.rsplit("::").next().unwrap_or(b).to_string();
                            if !b.starts_with('#') && !own_bounds.contains(&b) {
                                own_bounds.push(b);
                            }
                        }
                        k += 1;
                    }
                    let given = own_bounds.contains(&n);
                    if !given {
                        return Err(format!("{}:1: the trait bound `Self: {n}` is not satisfied (E0277)", it.line));
                    }
                }
            }
        }
        // (E0277 / E0271: a non-generic fn's `-> impl Tr` return: the type its body gives holds Tr, by the trait solver
        // (`fn test() -> impl Test { () }` with no `impl Test for ()`))
        if let Some(e) = opaque_bound_error(&prog, &src, &items[..user.min(items.len())])? {
            return Err(e);
        }
        // (E0271 / E0277: the bounds a trait of the program's puts on its associated types (`type Sibling: Bar2<Ok =
        // Self::Ok>`) hold of each impl's definitions, with the impl's parameters as placeholders: reported at the
        // definition)
        if !prog.tdb.user.is_empty() && !prog.assoc_bounds.is_empty() {
            let mut checks: Vec<(usize, String, String, Ty)> = vec![];
            for cs in prog.tdb.clauses.values() {
                for c in cs {
                    for (n, v) in &c.assocs {
                        checks.push((c.item, n.clone(), String::new(), *v));
                    }
                }
            }
            for (ii, n, _, v) in checks {
                let Some(sm) = sums.get(ii) else { continue };
                let (Some(tr), Some(st)) = (sm.impl_trait.as_deref(), sm.impl_self_ty.as_ref()) else { continue };
                let tr = tr.split('<').next().unwrap_or(tr).trim();
                let tr = tr.rsplit("::").next().unwrap_or(tr).to_string();
                let Some(bounds) = prog.assoc_bounds.get(&(tr.clone(), n.clone())).cloned() else { continue };
                // (nor an impl whose where clause is on another type: the definition may hold its bounds through it,
                // as the one wasm-bindgen's `#[wasm_bindgen]` writes, `impl<T> IntoJsGeneric for JsOption<T> where
                // JsOption<T>: JsGeneric { type JsCanon = JsOption<T>; }`, which the placeholders do not assume)
                if !read_all(ii) || where_other.contains(&ii) {
                    continue;
                }
                let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                let Ok(st) = prog.resolve_g(st, &gn) else { continue };
                let rigid: Vec<Ty> = sm.impl_generics.iter().enumerate().map(|(k, (g, bs))| Ty::Struct(prog.placeholder(u32::MAX - ii as u32, k as u32, g, bs))).collect();
                let (st, v) = (prog.subst(st, &rigid), prog.subst(v, &rigid));
                // (the definition's line: the member item that defines it. Looked for only for an error: a search of
                // every item for each definition was quadratic in a program of many impls)
                let line = || {
                    (0..items.len().min(sums.len()))
                        .find(|&jj| matches!(items[jj].kind, ItemKind::ImplMember(h) if h as usize == ii) && sums[jj].assocs.iter().any(|a| a.name == n))
                        .map_or(items[ii].line, |jj| items[jj].line)
                };
                for b in &bounds {
                    match prog.tdb.assoc_bound(&prog, v, b, st) {
                        (_, Some((proj, got, want))) => {
                            return Err(format!("{}:1: type mismatch resolving `{proj} == {}`: it is `{}` (E0271)", line(), prog.tn(want).replace('#', ""), prog.tn(got).replace('#', "")));
                        }
                        (crate::traits::Ans::No, None) => {
                            let tn = crate::traits::display_ty(&prog, v);
                            return Err(format!("{}:1: {} (E0277)", line(), crate::traits::not_held(&tn, b)));
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some((later, _, tr, g)) = prog.tdb.conflict(&prog, &read_all) {
            let line = items.get(later).map_or(1, |it| it.line);
            let ty = g.map(|g| format!(" for type `{}`", prog.tn(g).replace("__u_", ""))).unwrap_or_default();
            return Err(format!("{line}:1: conflicting implementations of trait `{tr}`{ty} (E0119)"));
        }
        // (E0107: an impl giving one of the program's traits the wrong number of generic arguments; only with a trait
        // header and an impl header)
        if items[..user].iter().any(|it| matches!(it.kind, crate::lexer::ItemKind::TraitHeader)) && items[..user].iter().any(|it| matches!(it.kind, crate::lexer::ItemKind::ImplHeader)) {
            if let Some(e) = first_trait_arity_error(&src, src0, &items[..user]) {
                return Err(e);
            }
        }
        // (E0107 at a use of a generic associated type without its arguments, or with the wrong number)
        if let Some(e) = first_gat_arity_error(&src, src0, &items[..user]) {
            return Err(e);
        }
        // (a derive whose field has not the trait: `#[derive(Hash)] struct S { x: Error }`)
        if !prog.derived.is_empty() {
            if let Some(e) = derive_field_error(&prog, &src, &items[..user]) {
                return Err(e);
            }
        }
        let ol = self.ctx.own_lines as usize;
        // (an item past this crate's own lines is a dependency's, but for one hoisted out of a fn body of this crate
        // (prep::HOIST_MARK): hoisted items come last, and are this crate's when they were written in its lines)
        let marks = crate::prep::hoist_marks(&src);
        let ext_items: Vec<bool> = items.iter().enumerate().map(|(i, it)| ol > 0 && i < user && !own_line(&marks, ol, it.line)).collect();
        // (ライブラリ: 自分の総称でない関数は全部外に見せる (使う側のクレートが呼ぶ))
        if self.ctx.lib.is_some() {
            for id in 0..prog.names.len() {
                if concrete(&prog, id) && prog.item_of[id].is_some_and(|ii| (ii as usize) < user && !ext_items[ii as usize]) {
                    prog.sigs[id].export = true;
                }
            }
        }
        let mut refs = Vec::with_capacity(prog.item_of.len());
        let mut srefs = Vec::with_capacity(prog.item_of.len());
        let mut crefs = Vec::with_capacity(prog.item_of.len());
        let names = prog.ref_names();
        // (keep_for_edits: the refs of an item of the same hash, read in a program of the same name tables, are the
        // same: an item's hash is of its tokens, identifiers and all)
        let mut memo = self.refs_memo.take().map(|(fp, m)| {
            let now = prog.ref_tables_fp();
            (now, if fp == now { m } else { FxMap::default() })
        });
        let mut used: FxMap<Key, (Vec<u32>, Vec<u32>, Vec<u32>)> = FxMap::default();
        for ii in &prog.item_of {
            let (f, st, c) = match (ii, &mut memo) {
                (Some(ii), Some((_, m))) => {
                    let it = &items[*ii as usize];
                    let r = match used.get(&it.hash) {
                        Some(r) => r.clone(),
                        None => match m.remove(&it.hash) {
                            Some(r) => r,
                            None => prog.refs_in(&src, it, Some(&names)),
                        },
                    };
                    used.entry(it.hash).or_insert_with(|| r.clone());
                    r
                }
                (Some(ii), None) => prog.refs_in(&src, &items[*ii as usize], Some(&names)),
                (None, _) => Default::default(),
            };
            refs.push(f);
            srefs.push(st);
            crefs.push(c);
        }
        if let Some((fp, _)) = memo {
            self.refs_memo = Some((fp, used));
        }
        tm.lap("refs", String::new());
        let mut a = Analysis { late_err, iface_fp, src, items, prog, refs, group_keys: Mutex::new((vec![], FxMap::default())), srefs, crefs, n_parsed, ext_items, inl: acg::Inline::default(), cl: self.opts.backend, prelude_line0, dep_ranges: self.ctx.dep_ranges.clone(), asts: Mutex::new(FxMap::default()), encl: Mutex::new(FxMap::default()), sig_keys: Mutex::new(Vec::new()), assoc_keys: (0, vec![]) };
        a.assoc_keys = a.assoc_keys();
        if self.opts.opt && std::env::var_os("ZUSTC_NO_INLINE").is_none() {
            a.inl = inline_bodies(&a);
        }
        // (this analysis's summaries for the next, by key. The last ones it did not take are dropped)
        if let Some(mut k) = keep {
            k.clear();
            for (i, sm) in sums.into_iter().enumerate() {
                k.entry(sum_keys[i]).or_default().push(sm);
            }
            self.sum_keep = Some(k);
        }
        Ok(a)
    }
}

/// `-> impl Iterator<..>` の戻り値を本体の型推論で決める (front.rs の resolve_opaque と同じ)
/// a `_` left in a type (inference did not decide it)
fn has_hole_ty(t: Ty) -> bool {
    match t {
        Ty::Param(i) => i == crate::program::HOLE,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => has_hole_ty(*i),
        _ => false,
    }
}

/// the declared return type (`Result<(), impl Debug>`, its `impl` parts holes) with the holes of the inferred one
/// filled from it, where both have the same shape (inference may leave `Ok`'s type open: `Err(Target)`)
fn merge_declared(prog: &Program, decl: Ty, inf: Ty) -> Ty {
    let hole = |t: Ty| t == Ty::Param(crate::program::HOLE);
    match (decl, inf) {
        (d, i) if hole(i) && !hole(d) => d,
        (Ty::Struct(x), Ty::Struct(y)) if x != y && prog.base_of(x) == prog.base_of(y) => {
            let (ax, ay) = (prog.structs[x as usize].args.clone(), prog.structs[y as usize].args.clone());
            if ax.len() != ay.len() {
                return inf;
            }
            let args: Vec<Ty> = ax.iter().zip(&ay).map(|(&d, &i)| merge_declared(prog, d, i)).collect();
            prog.instantiate(prog.base_of(x), args).map(Ty::Struct).unwrap_or(inf)
        }
        (Ty::Ref(_, d), Ty::Ref(m, i)) => Ty::Ref(m, crate::types::intern(merge_declared(prog, *d, *i))),
        (Ty::Box(d), Ty::Box(i)) => Ty::Box(crate::types::intern(merge_declared(prog, *d, *i))),
        _ => inf,
    }
}

fn resolve_opaque(prog: &mut Program, src: &str, items: &[SrcItem]) -> R<()> {
    if prog.opaque.is_empty() {
        return Ok(());
    }
    let ids = prog.opaque.clone();
    let mut done = vec![false; ids.len()];
    for _ in 0..4 {
        let mut progress = false;
        for (k, &id) in ids.iter().enumerate() {
            if done[k] {
                continue;
            }
            let sig = prog.sigs[id as usize].clone();
            let Some(ii) = prog.item_of[id as usize] else { continue };
            let Some(f) = parse_fn(src, &items[ii as usize])? else { continue };
            let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty };
            // (a generic fn: each instance's return type comes from its body with the instance's arguments, when
            // the instance is made (Insts::request); calls before that see a new type variable)
            if sig.is_generic() {
                // (`fn opaque<T>(t: T) -> impl Sized { opaque(Some(t)) }`: each instance's type is the next one's)
                let me = prog.names[id as usize].rsplit("::").next().unwrap_or("").to_string();
                if let Some(crate::parse::Stmt::Expr(crate::parse::Expr::Call(callee, _), false)) = f.block.stmts.last() {
                    if matches!(&**callee, crate::parse::Expr::Path(None, p) if p.segs.last().is_some_and(|s| s.name == me)) {
                        return Err(format!("{}:1: cannot resolve opaque type (`{me}` returns its own result for other arguments)", items[ii as usize].line));
                    }
                }
                prog.opaque_generic.insert(id, std::sync::Arc::new((f.sig.params.clone(), f.block.clone(), f.sig.recv.is_some())));
                prog.sigs[id as usize].ret = Ty::Param(crate::program::HOLE);
                done[k] = true;
                progress = true;
                continue;
            }
            // (`-> Result<(), impl Debug>`: the declared type with a hole for each `impl`, which the body fills)
            let decl = f.sig.ret.as_ref().filter(|t| !matches!(t, crate::parse::Ty::Impl(_))).and_then(|t| {
                fn holes(t: &crate::parse::Ty) -> crate::parse::Ty {
                    use crate::parse::{GArg, Ty as P};
                    match t {
                        P::Impl(_) => P::Infer,
                        P::Ref(l, m, i) => P::Ref(l.clone(), *m, Box::new(holes(i))),
                        P::Tuple(v) => P::Tuple(v.iter().map(holes).collect()),
                        P::Path(q, p) => {
                            let mut p = p.clone();
                            for s in &mut p.segs {
                                for a in &mut s.args {
                                    if let GArg::Ty(t) = a {
                                        *t = holes(t);
                                    }
                                }
                            }
                            P::Path(q.clone(), p)
                        }
                        t => t.clone(),
                    }
                }
                let ts = crate::ast_types::tysyn(&holes(t), None).ok()?;
                prog.resolve_env(&ts, &env).ok()
            });
            // (`-> impl Iterator<Item = u32> { 0..3 }`: a range at the tail is a Range of the declared Item, so its
            // literals take that type)
            let decl = decl.or_else(|| {
                use crate::parse::{Bound, Expr, GArg, Stmt, Ty as P};
                let Some(P::Impl(bs)) = &f.sig.ret else { return None };
                let item = bs.iter().find_map(|b| match b {
                    Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name == "Iterator") => p.segs.last()?.args.iter().find_map(|a| match a {
                        GArg::Bind(n, t) if n == "Item" => Some(t.clone()),
                        _ => None,
                    }),
                    _ => None,
                })?;
                let Some(Stmt::Expr(Expr::Range(Some(_), Some(_), incl), false)) = f.block.stmts.last() else { return None };
                let rn = if *incl { "RangeInclusive" } else { "Range" };
                let it = prog.resolve_env(&crate::ast_types::tysyn(&item, None).ok()?, &env).ok()?;
                let rid = *prog.struct_index.get(rn)?;
                prog.instantiate(rid, vec![it]).ok().map(Ty::Struct)
            });
            let (_, r, _) = crate::ast_infer::infer_ctx(prog, f.sig.recv.is_some(), &f.sig.params, &f.block, &sig, &env, &[], decl, Some((id, 0)));
            let sig = match decl {
                Some(d) => crate::program::FnSig { ret: d, ..sig },
                None => sig,
            };
            // (a body with no value (`-> impl Sized {}`) returns `()`)
            let no_value = !matches!(f.block.stmts.last(), Some(crate::parse::Stmt::Expr(_, false))) && !format!("{:?}", f.block).contains("Return(Some");
            // (a body ending in `()` itself, or in a diverging macro (`panic!()` / `todo!()`), is `()` too)
            let unit_tail = match f.block.stmts.last() {
                Some(crate::parse::Stmt::Expr(crate::parse::Expr::Tuple(v), false)) => v.is_empty(),
                // (`async {}` / `{ f(); }`: a block with no value of its own)
                Some(crate::parse::Stmt::Expr(crate::parse::Expr::Block(_, _, b), false)) => !matches!(b.stmts.last(), Some(crate::parse::Stmt::Expr(_, false))),
                Some(crate::parse::Stmt::Expr(crate::parse::Expr::Macro(p, _, _), _)) => {
                    matches!(p.segs.last().map(|s| s.name.as_str()), Some("panic" | "todo" | "unimplemented" | "unreachable"))
                }
                _ => false,
            };
            let no_value = no_value || unit_tail;
            // (`-> Vec<impl Debug> { panic!() }`: nothing says what the type is)
            if no_value && !matches!(f.sig.ret, Some(crate::parse::Ty::Impl(_))) {
                return Err(format!("{}:1: type annotations needed (the return type of `{}` is not known from its body)", items[ii as usize].line, f.sig.name));
            }
            let r = r.map(|t| merge_declared(prog, sig.ret, t));
            if let Some(t) = r.filter(|&t| (t != Ty::Unit || no_value) && !prog.has_param(t) && !has_hole_ty(t)) {
                prog.sigs[id as usize].ret = t;
                done[k] = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    // (`fn test() -> impl Sized { test() }`: a body that only returns its own result has no type of its own, and
    // never returns; it is `()`)
    for (k, &id) in ids.iter().enumerate() {
        if done[k] {
            continue;
        }
        let me = prog.names[id as usize].rsplit("::").next().unwrap_or("").to_string();
        let Some(ii) = prog.item_of[id as usize] else { continue };
        let Some(f) = parse_fn(src, &items[ii as usize])? else { continue };
        if f.block.stmts.len() == 1 && matches!(f.sig.ret, Some(crate::parse::Ty::Impl(_))) {
            if let Some(crate::parse::Stmt::Expr(crate::parse::Expr::Call(callee, _), false)) = f.block.stmts.last() {
                if matches!(&**callee, crate::parse::Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == me) {
                    prog.sigs[id as usize].ret = Ty::Unit;
                    done[k] = true;
                }
            }
        }
    }
    if let Some(k) = done.iter().position(|d| !*d) {
        return Err(format!("cannot infer the `impl Trait` return type of `{}`", prog.names[ids[k] as usize]));
    }
    Ok(())
}

/// The first `-> impl Tr` of the program's non-generic fns whose type from the body is surely not Tr: the trait solver's
/// `No` (`solve_no_sound`), for each bound written (`impl Tr + Send`, `impl Tr<Assoc = X>`). A generic fn's type is
/// each instance's, and a nested `impl` (`Option<impl Tr>`) is not followed
/// The values of an `if` / `match`'s arms as the paths they name (`S`, `S { .. }`, `S(..)`), into out; false when an
/// arm's value is another expression (its type is not read here)
fn arm_heads(e: &crate::parse::Expr, out: &mut Vec<String>, depth: u32) -> bool {
    use crate::parse::{Expr, Stmt};
    if depth > 16 {
        return false;
    }
    let path = |p: &crate::parse::Path| p.segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("::");
    fn block_tail(b: &crate::parse::Block) -> Option<&Expr> {
        match b.stmts.last() {
            Some(Stmt::Expr(t, false)) => Some(t),
            _ => None,
        }
    }
    match e {
        Expr::If(_, then, els) => {
            let Some(t) = block_tail(then) else { return false };
            let Some(els) = els else { return false };
            arm_heads(t, out, depth + 1) && arm_heads(els, out, depth + 1)
        }
        Expr::Match(_, arms) => !arms.is_empty() && arms.iter().all(|a| arm_heads(&a.body, out, depth + 1)),
        Expr::Block(_, _, b) => block_tail(b).is_some_and(|t| arm_heads(t, out, depth + 1)),
        Expr::Paren(x) => arm_heads(x, out, depth + 1),
        Expr::Path(None, p) | Expr::Struct(p, _, _) => {
            out.push(path(p));
            true
        }
        Expr::Call(f, _) => match &**f {
            Expr::Path(None, p) => {
                out.push(path(p));
                true
            }
            _ => false,
        },
        _ => false,
    }
}

fn opaque_bound_error(prog: &Program, src: &str, items: &[SrcItem]) -> R<Option<String>> {
    // (a bound as a goal the solver may answer: the program's traits, the auto traits, and std's for the types
    // `implements` judges exactly (the solver believes its no only there); a std bound whose arguments are all
    // bindings (`Iterator<Item = &u32>`) is asked without them, which is weaker: its no stands)
    let check = |t: Ty, b: &str, line: usize| -> Option<String> {
        let n = b.split('<').next().unwrap_or(b).split('(').next().unwrap_or(b).trim();
        let n = n.rsplit("::").next().unwrap_or(n);
        let user = prog.tdb.user.contains(n);
        let goal = if !user && b.contains('<') {
            let inner = b[b.find('<')? + 1..].trim_end().strip_suffix('>')?;
            if inner.split(',').all(|a| a.contains('=')) { n.to_string() } else { return None }
        } else if user {
            b.to_string()
        } else {
            n.to_string()
        };
        let tn = crate::traits::display_ty(prog, t);
        if user && b.contains('=') {
            if let Some((proj, got, want)) = prog.tdb.binding_mismatch(prog, t, b, &[], &[]) {
                return Some(format!("{line}:1: type mismatch resolving `{proj} == {}`: it is `{}` (E0271)", prog.tn(want), prog.tn(got)));
            }
        }
        (prog.tdb.holds(prog, t, &goal) == crate::traits::Ans::No).then(|| format!("{line}:1: {} (E0277)", crate::traits::not_held(&tn, &goal)))
    };
    let ground = |t: Ty| !prog.has_param(t) && !prog.names_placeholder(t, 0);
    for &id in &prog.opaque {
        let sig = &prog.sigs[id as usize];
        let t = sig.ret;
        if sig.is_generic() || !ground(t) {
            continue;
        }
        let Some(ii) = prog.item_of[id as usize].map(|i| i as usize).filter(|&i| i < items.len()) else { continue };
        let Some(f) = parse_fn(src, &items[ii])? else { continue };
        let Some(crate::parse::Ty::Impl(bs)) = &f.sig.ret else { continue };
        let Ok(names) = crate::ast_summary::bound_names(bs) else { continue };
        // (a tail `if` / `match`: the type recorded is its first arm's; arms of other types are the arms' error
        // (E0308, "`if` and `else` have incompatible types"), reported by the body's check. Arms whose values all
        // name one type (`if b { S } else { S }`) are checked as one value)
        if let Some(crate::parse::Stmt::Expr(tail @ (crate::parse::Expr::If(..) | crate::parse::Expr::Match(..)), false)) = f.block.stmts.last() {
            let mut heads = vec![];
            // (a call to a non-generic fn names its return type: `if b { S } else { make_s() }` is one type)
            let heads: Vec<String> = if arm_heads(tail, &mut heads, 0) {
                heads
                    .into_iter()
                    .map(|h| match prog.index.get(h.as_str()).map(|&g| &prog.sigs[g as usize]) {
                        Some(s) if !s.is_generic() && ground(s.ret) && !prog.struct_index.contains_key(h.as_str()) => prog.tn(s.ret),
                        _ => h,
                    })
                    .collect()
            } else {
                continue;
            };
            if heads.windows(2).any(|w| w[0] != w[1]) {
                continue;
            }
        }
        for b in &names {
            if let Some(e) = check(t, b, items[ii].line) {
                return Ok(Some(e));
            }
        }
    }
    // (a trait's method declared `-> impl Tr` (its declaration's type is a placeholder): each impl's method returns
    // a type that is Tr, `impl Foo for () { fn bar() -> () {} }` with `fn bar() -> impl Display`)
    for (fi, sig) in prog.sigs.iter().enumerate() {
        let Some(tr) = sig.impl_trait.as_deref() else { continue };
        let tr = tr.split('<').next().unwrap_or(tr).trim();
        if sig.is_generic() || !ground(sig.ret) || !prog.tdb.user.contains(tr.rsplit("::").next().unwrap_or(tr)) {
            continue;
        }
        let Some(ii) = prog.item_of[fi].map(|i| i as usize).filter(|&i| i < items.len()) else { continue };
        let m = prog.names[fi].rsplit("::").next().unwrap_or("").split('#').next().unwrap_or("");
        let Some(&d) = prog.traits.get(tr).into_iter().flatten().find(|&&d| prog.names[d as usize].rsplit("::").next() == Some(m)) else { continue };
        let Some(di) = prog.item_of[d as usize].map(|i| i as usize).filter(|&i| i < items.len()) else { continue };
        let text = blank_comments(src.get(items[di].start..items[di].end).unwrap_or(""));
        let head = text.split('{').next().unwrap_or("").split(';').next().unwrap_or("");
        let Some(at) = head.find("-> impl ") else { continue };
        let rest = &head[at + 8..];
        let rest = rest.split(" where").next().unwrap_or(rest);
        // (the bounds, split at `+` at depth 0)
        let mut depth = 0i32;
        let mut bounds = vec![String::new()];
        for c in rest.chars() {
            match c {
                '<' | '(' => depth += 1,
                '>' | ')' => depth -= 1,
                '+' if depth == 0 => {
                    bounds.push(String::new());
                    continue;
                }
                _ => {}
            }
            if let Some(l) = bounds.last_mut() {
                l.push(c);
            }
        }
        // (a bound that names the trait's parameters or Self is not asked of a concrete type here)
        for b in bounds.iter().map(|b| b.trim()).filter(|b| !b.is_empty() && !b.starts_with('\'') && !b.contains("Self")) {
            let decl = &prog.sigs[d as usize];
            if decl.generics.iter().skip(1).any(|g| b.contains(g.0.as_str())) {
                continue;
            }
            if let Some(e) = check(sig.ret, b, items[ii].line) {
                return Ok(Some(e));
            }
        }
    }
    Ok(None)
}

/// アイテムの中の `行:桁:` のエラーを、ファイルの中の行にする (driver.rs の relocate_err と同じ)。位置がなければアイテムの先頭の行
/// rustc's fragment specifiers
const FRAGS: &[&str] = &["block", "expr", "expr_2021", "ident", "item", "lifetime", "literal", "meta", "pat", "pat_param", "path", "stmt", "tt", "ty", "vis"];

/// "invalid fragment specifier `id`": a `$x:id` in a `macro_rules!` matcher (not its transcriber) whose fragment is
/// none of rustc's, at the fragment
#[cold]
fn bad_fragment(src0: &str) -> Option<String> {
    let b = src0.as_bytes();
    for (at, _) in src0.match_indices("macro_rules!") {
        // (a definition at a line's start, past `#[..]` / whitespace: not one in a comment or a string)
        let ls = src0[..at].rfind('\n').map_or(0, |k| k + 1);
        if !src0[ls..at].trim().is_empty() && !src0[ls..at].trim_start().starts_with("#[") {
            continue;
        }
        // (the body: the bracket after the name, to its match, past strings and comments)
        let Some(open) = src0[at..].find(['{', '(', '[']).map(|k| at + k) else { continue };
        let (mut d, mut k, mut end) = (0i32, open, None);
        while k < b.len() {
            if let Some(e) = lit_end(b, k) {
                k = e;
                continue;
            }
            match b[k] {
                b'{' | b'(' | b'[' => d += 1,
                b'}' | b')' | b']' => {
                    d -= 1;
                    if d == 0 {
                        end = Some(k);
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        let Some(end) = end else { continue };
        let body = &src0[open + 1..end];
        let toks = crate::lexer::tokenize(body.as_bytes());
        let tk = |j: usize| &body[toks[j].1..toks[j].2];
        // (the rules: a matcher group at depth 0, `=>`, a transcriber group, `;`)
        let mut j = 0;
        while j < toks.len() {
            if !matches!(tk(j), "(" | "[" | "{") {
                j += 1;
                continue;
            }
            // (the matcher: to its close)
            let (mut dd, mut m) = (0i32, j);
            while m < toks.len() {
                match tk(m) {
                    "(" | "[" | "{" => dd += 1,
                    ")" | "]" | "}" => {
                        dd -= 1;
                        if dd == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                m += 1;
            }
            for q in j..m.min(toks.len()) {
                if tk(q) == "$" && q + 3 <= m && toks[q + 1].0 == 0 && tk(q + 2) == ":" && toks.get(q + 3).is_some_and(|t| t.0 == 0) {
                    let f = tk(q + 3);
                    // (`guard`: a fragment behind `macro_guard_matcher`)
                    if f == "guard" && !src0.contains("macro_guard_matcher") {
                        let pos = open + 1 + toks[q + 3].1;
                        let line = 1 + src0[..pos].matches('\n').count();
                        return Some(format!("{line}:1: `guard` fragments in macro are unstable (E0658)"));
                    }
                    if !FRAGS.contains(&f) && f != "guard" {
                        let pos = open + 1 + toks[q + 3].1;
                        let line = 1 + src0[..pos].matches('\n').count();
                        let col = pos - src0[..pos].rfind('\n').map_or(0, |x| x + 1) + 1;
                        return Some(format!("{line}:{col}: invalid fragment specifier `{f}`"));
                    }
                }
            }
            // (past the transcriber: `=>` then its group)
            let mut t = m + 1;
            while t < toks.len() && !matches!(tk(t), "(" | "[" | "{") {
                t += 1;
            }
            let (mut dd, mut u) = (0i32, t);
            while u < toks.len() {
                match tk(u) {
                    "(" | "[" | "{" => dd += 1,
                    ")" | "]" | "}" => {
                        dd -= 1;
                        if dd == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                u += 1;
            }
            j = u + 1;
        }
    }
    None
}

/// A validation error the parser deferred (`for<'a: 'b>`), unless a later item does not parse: rustc stops at that
/// one first
#[cold]
fn deferred_after_parse(e: String, src: &str, later: &[SrcItem]) -> String {
    use crate::lexer::ItemKind as K;
    for it in later {
        if matches!(it.kind, K::InnerAttr) {
            continue;
        }
        let text = src.get(it.start..it.end).unwrap_or("");
        let text = match it.kind {
            K::ImplHeader | K::TraitHeader => format!("{text}}}"),
            _ => text.to_string(),
        };
        if let Err(e2) = crate::parse::parse_file(&text) {
            if !e2.starts_with('\u{7}') {
                return relocate(e2, it, src);
            }
        }
    }
    e
}

/// E0746 at the fn's return type (rustc's span), not its first line: `fn f() ->\n    dyn Tr`
#[cold]
fn ret_type_err(e: String, src: &str, it: &SrcItem) -> String {
    let text = src.get(it.start..it.end).unwrap_or("");
    let Some(at) = sig_part(text, "r") else { return e };
    let Some((_, msg)) = e.split_once(":1: ") else { return e };
    let line = it.line + text[..at].matches('\n').count();
    let col = at - text[..at].rfind('\n').map_or(0, |k| k + 1) + 1;
    format!("{line}:{col}: {msg}")
}

fn ret_type_line(e: String, src: &str, it: &SrcItem) -> String {
    if e.ends_with("return type cannot be a trait object without pointer indirection (E0746)") {
        return ret_type_err(e, src, it);
    }
    e
}

/// An error of an item's own text at its place in src: its line, and on its first line its column too (an impl's member
/// starts after its indentation)
fn relocate(e: String, it: &SrcItem, src: &str) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            if l == 1 {
                let head = &src[..it.start.min(src.len())];
                let indent = head.len() - head.rfind('\n').map_or(0, |p| p + 1);
                if let Some((c, msg)) = rest.split_once(':').filter(|_| indent > 0) {
                    if let Ok(c) = c.parse::<usize>() {
                        let indent = head[head.len() - indent..].chars().count();
                        return format!("{}:{}:{msg}", it.line, c + indent);
                    }
                }
            }
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    format!("{}:1: {e}", it.line)
}

/// E0434: a fn item written inside another fn (hoisted out of it) that uses a local of a fn it is in, or its `self`:
/// `fn foo() { let y = 5; fn bar() -> u32 { y } }`. A name the program also has an item of is that item.
/// E0435 the same way for a value a constant holds: a `const` / `static` item's, an enum's discriminant, an impl's
/// associated const, written in a fn (`let n = 1; static Y: usize = n + 1;`)
fn first_env_capture(src: &str, orig: &str, items: &[SrcItem]) -> Option<String> {
    use crate::parse::Expr;
    // (hoisted items come last, each piece's first line starting with a mark: none when the last item's isn't)
    let last = items.last()?;
    let head = match last.kind {
        crate::lexer::ItemKind::ImplMember(h) | crate::lexer::ItemKind::TraitMember(h) => items.get(h as usize)?,
        _ => last,
    };
    let line_start = src[..head.start].rfind('\n').map_or(0, |n| n + 1);
    if !src[line_start..].starts_with(crate::prep::HOIST_MARK) && !src[head.start..].starts_with(crate::prep::HOIST_MARK) {
        return None;
    }
    let marks = crate::prep::hoist_marks(src);
    if marks.is_empty() {
        return None;
    }
    let text_of = |it: &SrcItem| src.get(it.start..it.end).unwrap_or("");
    // (each item's lines as written)
    let range = |it: &SrcItem| {
        let a = crate::prep::hoisted_at(&marks, it.line).unwrap_or(it.line);
        (a, a + text_of(it).matches('\n').count())
    };
    let hoisted: Vec<usize> = (0..items.len())
        .filter(|&i| {
            let t = text_of(&items[i]);
            crate::prep::hoisted_at(&marks, items[i].line).is_some() && (t.contains("fn ") || t.contains("const ") || t.contains("static ") || (t.contains("enum ") && t.contains('=')))
        })
        .collect();
    if hoisted.is_empty() {
        return None;
    }
    // (the program's item names)
    const KEYWORDS: &[&str] = &["fn", "const", "static", "struct", "enum", "union", "type", "mod", "trait"];
    let mut item_names: crate::fxhash::FxSet<&str> = Default::default();
    for it in items {
        let ids: Vec<&str> = it.idents.iter().map(|&(a, b)| src.get(a as usize..b as usize).unwrap_or("")).collect();
        if let Some(k) = ids.iter().take(8).position(|w| KEYWORDS.contains(w)) {
            if let Some(n) = ids.get(k + 1) {
                item_names.insert(n);
            }
        }
    }
    // (a fn's own locals: its parameters, its `let`s and its `self`)
    fn locals_of(sig: &crate::parse::FnSig, body: &crate::parse::Block, out: &mut Vec<String>) {
        if sig.recv.is_some() {
            out.push("self".to_string());
        }
        for (p, _) in &sig.params {
            crate::ast_closure::pat_idents(p, out);
        }
        out.extend(crate::ast_closure::let_bindings(body).into_iter().map(|(_, n)| n));
    }
    // (the first fn in `body` or nested in it (one level of items per hoisting) that names an outer local: the fn's
    // name, the local, and where the fn's text uses it free (line 0 when not known))
    fn capture(sig: &crate::parse::FnSig, body: &crate::parse::Block, outer: &[String], items: &crate::fxhash::FxSet<&str>) -> Option<(String, String, crate::parse::Pos)> {
        let mut used = crate::ast_closure::used_vars_at(&Expr::Block(None, false, body.clone()));
        // (`self` in its own statements, not in an item inside it: serde's `deserialize` with an `impl Visitor for
        // KindVisitor` whose methods take `self`)
        let names_self = body.stmts.iter().any(|st| !matches!(st, crate::parse::Stmt::Item(_)) && format!("{st:?}").contains("\"self\""));
        if sig.recv.is_none() && names_self && !used.iter().any(|(n, _)| n == "self") {
            used.push(("self".to_string(), Default::default()));
        }
        // (its own parameters are in its signature, not its body)
        let mut own = vec![];
        for (p, _) in &sig.params {
            crate::ast_closure::pat_idents(p, &mut own);
        }
        if sig.recv.is_some() {
            own.push("self".to_string());
        }
        used.retain(|(n, _)| !own.contains(n));
        // (a capitalized name its own `let` binds is its local: `let T0 = ..; (T0, T1)` in serde's tuple impls, with
        // `#[allow(non_snake_case)]`. The free-variable walk reads a bare capitalized name in a pattern as a path)
        let upper_lets: Vec<String> = crate::ast_closure::let_bindings(body).into_iter().map(|(_, n)| n).filter(|n| n.starts_with(|c: char| c.is_ascii_uppercase())).collect();
        used.retain(|(n, _)| !upper_lets.contains(n));
        if let Some((n, at)) = used.iter().find(|(n, _)| !items.contains(n.as_str()) && outer.contains(n)) {
            return Some((sig.name.clone(), n.clone(), *at));
        }
        let mut mine = outer.to_vec();
        locals_of(sig, body, &mut mine);
        body.stmts.iter().find_map(|st| match st {
            crate::parse::Stmt::Item(crate::parse::Item { kind: crate::parse::ItemKind::Fn(s2, Some(b2)), .. }) => capture(s2, b2, &mine, items),
            _ => None,
        })
    }
    // (where an item starts in the file as written: its first line up to its `{` (a fn's body, which the items
    // hoisted out of it leave), found in the line it was written on)
    let orig_at = |it: &SrcItem| -> Option<usize> {
        let text = text_of(it);
        let first = &text[..text.find('\n').unwrap_or(text.len())];
        let skip = first.find("*/").filter(|_| first.starts_with(crate::prep::HOIST_MARK)).map_or(0, |n| n + 2);
        let first = first[skip..].trim_start();
        let head = first[..first.find('{').unwrap_or(first.len())].trim_end();
        let line = crate::prep::hoisted_at(&marks, it.line).unwrap_or(it.line);
        let l = orig.split('\n').nth(line.checked_sub(1)?)?;
        Some(l.as_ptr() as usize - orig.as_ptr() as usize + l.find(head).filter(|_| !head.is_empty())?)
    };
    // (the fn written at `a` has what is written at `b` in its body: no `}` closes the body between them)
    let encloses = |a: usize, b: usize| {
        let Some(seg) = orig.as_bytes().get(a..b) else { return false };
        let mut d = 0i32;
        for &(k, s, _) in &crate::lexer::tokenize(seg) {
            if k == 1 && seg[s] == b'{' {
                d += 1;
            } else if k == 1 && seg[s] == b'}' {
                d -= 1;
                if d <= 0 {
                    return false;
                }
            }
        }
        d > 0
    };
    // (the `let`s of the fn written at `a` that are in scope at `b`: those before it in the blocks still open there, not
    // a sibling block's, `{ let drop = 1u8; }` beside `fn inner() { drop(0u8); }`. A `let`'s names start after its `;`,
    // not in its initializer; an `if let` / `while let`'s in its body only. None when the text is not read)
    let scoped_lets = |a: usize, b: usize| -> Option<Vec<String>> {
        let seg = orig.as_bytes().get(a..b)?;
        let t = crate::lexer::tokenize(seg);
        let w = |q: usize| if q < t.len() && t[q].0 == 0 { std::str::from_utf8(&seg[t[q].1..t[q].2]).unwrap_or("") } else { "" };
        let sym = |q: usize, c: u8| q < t.len() && t[q].0 == 1 && seg[t[q].1] == c;
        let mut open: Vec<Vec<String>> = vec![];
        // (a `let`'s names until its `;`, and an `if let`'s until its body opens: with the depth they were written at)
        let mut pending: Vec<(usize, Vec<String>)> = vec![];
        let mut cond: Option<(usize, Vec<String>)> = None;
        let mut q = 0;
        while q < t.len() {
            if sym(q, b'{') {
                let body = cond.take_if(|(d, _)| *d == open.len());
                open.push(body.map(|(_, ns)| ns).unwrap_or_default());
            } else if sym(q, b'}') {
                open.pop()?;
                pending.retain(|(d, _)| *d <= open.len());
            } else if sym(q, b';') {
                let depth = open.len();
                for (_, ns) in pending.extract_if(.., |(d, _)| *d == depth) {
                    open.last_mut()?.extend(ns);
                }
            } else if w(q) == "let" && !open.is_empty() {
                let conditional = q > 0 && (w(q - 1) == "if" || w(q - 1) == "while" || sym(q - 1, b'&'));
                let mut names = vec![];
                // (its pattern's names, to the `=`, `;` or type at the pattern's own level)
                let mut d = 0i32;
                q += 1;
                while q < t.len() {
                    let path = sym(q + 1, b':') && sym(q + 2, b':') || q > 0 && sym(q - 1, b':') && q > 1 && sym(q - 2, b':');
                    if d == 0 && (sym(q, b'=') || sym(q, b';') || (sym(q, b':') && !sym(q + 1, b':') && !(q > 0 && sym(q - 1, b':')))) {
                        break;
                    }
                    if sym(q, b'(') || sym(q, b'[') || sym(q, b'{') {
                        d += 1;
                    } else if sym(q, b')') || sym(q, b']') || sym(q, b'}') {
                        d -= 1;
                    } else if t[q].0 == 0 && !path && !(t[q].1 > 0 && seg[t[q].1 - 1] == b'\'') {
                        let n = w(q);
                        let binds = n.starts_with(|c: char| c.is_lowercase() || c == '_') && !matches!(n, "mut" | "ref" | "box" | "_");
                        if binds && !sym(q + 1, b'(') && !sym(q + 1, b'{') && !(sym(q + 1, b':') && d > 0) {
                            names.push(n.to_string());
                        }
                    }
                    q += 1;
                }
                if conditional {
                    cond = Some((open.len(), names));
                } else {
                    pending.push((open.len(), names));
                }
                continue;
            }
            q += 1;
        }
        Some(open.into_iter().flatten().collect())
    };
    // (the values a constant item holds: a const's or static's, an enum's discriminants, or (an impl's member) an
    // associated const's)
    let const_values = |it: &SrcItem| -> Vec<Expr> {
        let Ok(file) = crate::parse::parse_file(text_of(it)) else { return vec![] };
        let Some(item) = file.items.into_iter().next() else { return vec![] };
        match item.kind {
            crate::parse::ItemKind::Const(_, _, Some(e)) | crate::parse::ItemKind::Static(_, _, _, e) => vec![e],
            crate::parse::ItemKind::Enum(_, _, vs) => vs.into_iter().filter_map(|(_, _, d)| d).collect(),
            _ => vec![],
        }
    };
    for &h in &hoisted {
        let it = &items[h];
        let f = match parse_fn(src, it) {
            Ok(Some(f)) => Some(f),
            _ => None,
        };
        let values = if f.is_none() { const_values(it) } else { vec![] };
        if f.is_none() && values.is_empty() {
            continue;
        }
        // (the locals of the fns it is written in)
        let line = range(it).0;
        let mut outer: Vec<String> = vec![];
        for (g, gi) in items.iter().enumerate() {
            let (a, b) = range(gi);
            if g == h || !(a <= line && line <= b) || !text_of(gi).contains("fn ") {
                continue;
            }
            // (on the line a fn starts or ends on, its columns say whether it is around: `fn a(x: u8) {} fn b() {
            // fn c() {} }` on one line)
            // (when the text as written does not have them, a macro's expansion on one line, `closures! { .. }` with
            // its `const _: () = { fn invoke() .. }`, it is not known to be around: no E0434)
            if line == a || line == b {
                match (orig_at(gi), orig_at(it)) {
                    (Some(x), Some(y)) if encloses(x, y) => {}
                    _ => continue,
                }
            }
            let Ok(Some(gf)) = parse_fn(src, gi) else { continue };
            match orig_at(gi).zip(orig_at(it)).filter(|(x, y)| x < y).and_then(|(x, y)| scoped_lets(x, y)) {
                Some(lets) => {
                    if gf.sig.recv.is_some() {
                        outer.push("self".to_string());
                    }
                    for (p, _) in &gf.sig.params {
                        crate::ast_closure::pat_idents(p, &mut outer);
                    }
                    outer.extend(lets);
                }
                None => locals_of(&gf.sig, &gf.block, &mut outer),
            }
        }
        if outer.is_empty() {
            continue;
        }
        // (a constant's value: the first local it names, with `self` (which the walk does not give) where it is written)
        let in_const = f.is_none();
        let found_use = match &f {
            Some(f) => capture(&f.sig, &f.block, &outer, &item_names),
            None => values.iter().find_map(|e| {
                let mut used = crate::ast_closure::used_vars_at(e);
                if format!("{e:?}").contains("\"self\"") && !used.iter().any(|(n, _)| n == "self") {
                    used.push(("self".to_string(), Default::default()));
                }
                used.into_iter().find(|(n, _)| !item_names.contains(n.as_str()) && outer.contains(n)).map(|(n, at)| (String::new(), n, at))
            }),
        };
        let Some((fname, name, at)) = found_use else { continue };
        let text = text_of(it);
        // (rustc points at the use: the free one the walk found, at its line (from 1) and column (in characters, from
        // 0) in the item's text)
        let found = (at.line > 0)
            .then(|| {
                let l = text.split('\n').nth(at.line as usize - 1)?;
                let base = l.as_ptr() as usize - text.as_ptr() as usize;
                Some(base + l.char_indices().nth(at.col as usize).map_or(l.len(), |(i, _)| i))
            })
            .flatten()
            .and_then(|o| {
                let r = &text[o..];
                if r.starts_with(name.as_str()) {
                    return Some(o);
                }
                // (a format string's `{x}` / `{x:?}`: the name in it)
                let end = str_lit_len(r)?;
                ['}', ':'].iter().find_map(|c| r[..end].find(&format!("{{{name}{c}"))).map(|k| o + k + 1)
            });
        // (else (`self`) the first in that fn's body that is not a field or a path's segment)
        let off = match found {
            Some(o) => o,
            None => {
                let b = text.as_bytes();
                let t = crate::lexer::tokenize(b);
                let w = |k: usize| if k < t.len() && t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" };
                let sym = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && b[t[k].1] == c;
                // (in a fn's body; in a constant, after its `=`)
                let from = (1..t.len()).find(|&k| w(k) == fname && w(k - 1) == "fn").unwrap_or(0);
                let open = if in_const { (0..t.len()).find(|&k| sym(k, b'=') && !sym(k + 1, b'='))? } else { (from..t.len()).find(|&k| sym(k, b'{'))? };
                let Some(k) = (open..t.len()).find(|&k| w(k) == name && !(k > 0 && (sym(k - 1, b'.') || sym(k - 1, b':'))) && !sym(k + 1, b':') && !sym(k + 1, b'!')) else {
                    continue;
                };
                t[k].1
            }
        };
        let (line, col) = item_pos(src, orig, &marks, it, off);
        if in_const {
            return Some(format!("{line}:{col}: attempt to use a non-constant value in a constant (E0435)"));
        }
        return Some(format!("{line}:{col}: can't capture dynamic environment in a fn item (E0434)"));
    }
    None
}

/// The length of the string literal `r` starts with (`"..."`, `r"..."`, `r#"..."#`), None when it starts with none
fn str_lit_len(r: &str) -> Option<usize> {
    let b = r.as_bytes();
    let (raw, hashes, open) = match b.first()? {
        b'"' => (false, 0, 1),
        b'r' => {
            let h = b[1..].iter().take_while(|&&c| c == b'#').count();
            (b.get(1 + h) == Some(&b'"')).then_some((true, h, 2 + h))?
        }
        _ => return None,
    };
    let mut i = open;
    while i < b.len() {
        // (escapes only in a cooked string: `r"\"` ends at its second byte's quote)
        if !raw && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == b'"' && b[i + 1..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes {
            return Some(i + 1 + hashes);
        }
        i += 1;
    }
    None
}

/// The offset of a `pub` an item's text starts with, past its outer attributes and comments
fn leading_pub(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = 0;
    loop {
        // (whitespace and comments, nested ones too)
        i = skip_space(b, i);
        if b[i..].starts_with(b"#[") || b[i..].starts_with(b"#![") {
            // (an attribute: to its `]`, past the strings in it)
            let mut d = 0i32;
            while i < b.len() {
                match b[i] {
                    b'"' => {
                        i += 1;
                        while i < b.len() && b[i] != b'"' {
                            i += if b[i] == b'\\' { 2 } else { 1 };
                        }
                    }
                    b'[' => d += 1,
                    b']' => {
                        d -= 1;
                        if d == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        } else {
            break;
        }
    }
    (b[i..].starts_with(b"pub") && b.get(i + 3).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))).then_some(i)
}

/// In an enum's text: the offset of a `pub` on one of its variants or their fields (`V1 { pub f: i32 }`, `V2(pub i32)`)
fn enum_inner_pub(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let t = crate::lexer::tokenize(b);
    let w = |k: usize| if k < t.len() && t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" };
    let sym = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && b[t[k].1] == c;
    let e = (0..t.len()).find(|&k| w(k) == "enum")?;
    // (the body: its `{`, past the generics and a where clause)
    let open = (e..t.len()).find(|&k| sym(k, b'{'))?;
    // (`disc`: in a variant's discriminant, `A = { pub const N: isize = 1; N }`, whose items may be `pub`)
    let (mut d, mut disc) = (0i32, false);
    for k in open..t.len() {
        if sym(k, b'{') || sym(k, b'(') || sym(k, b'[') {
            d += 1;
        } else if sym(k, b'}') || sym(k, b')') || sym(k, b']') {
            d -= 1;
            if d == 0 {
                break;
            }
        } else if d == 1 && (sym(k, b'=') || sym(k, b',')) {
            disc = sym(k, b'=');
        } else if !disc && (d == 1 || d == 2) && w(k) == "pub" && k > 0 && (sym(k - 1, b'{') || sym(k - 1, b'(') || sym(k - 1, b',') || sym(k - 1, b']')) {
            return Some(t[k].1);
        }
    }
    None
}

/// The line and column, in the file as written (`orig`), of the byte at `off` in an item's text. An item hoisted out
/// of a fn body is given the line it was written on; on its first line, whose indent the hoisted text does not keep,
/// the column is found from that line as written
fn item_pos(src: &str, orig: &str, marks: &[(usize, usize)], it: &SrcItem, off: usize) -> (usize, usize) {
    let text = &src[it.start..it.end];
    let line = it.line + text[..off].matches('\n').count();
    // (columns count characters, as rustc's do)
    match crate::prep::hoisted_at(marks, line) {
        Some(at) => {
            let col = text[text[..off].rfind('\n').map_or(0, |n| n + 1)..off].chars().count() + 1;
            if text[..off].contains('\n') {
                return (at, col);
            }
            // (the item's first line, past a hoist mark, as it is in the line it was written on)
            let first = &text[..text.find('\n').unwrap_or(text.len())];
            let skip = first.find("*/").filter(|_| first.starts_with(crate::prep::HOIST_MARK)).map_or(0, |n| n + 2);
            // (up to 40 bytes, cut at a character's boundary)
            let end = (skip..=first.len().min(skip + 40)).rev().find(|&n| first.is_char_boundary(n)).unwrap_or(skip);
            let head = &first[skip..end];
            let l = orig.split('\n').nth(at.wrapping_sub(1)).unwrap_or("");
            let found = l.find(head).filter(|_| !head.trim().is_empty() && off >= skip);
            (at, found.map_or(col, |f| l[..f].chars().count() + text[skip..off].chars().count() + 1))
        }
        None => {
            let at = it.start + off;
            (line, src[src[..at].rfind('\n').map_or(0, |n| n + 1)..at].chars().count() + 1)
        }
    }
}

/// The top-level parts of a `<..>` of tokens that starts at `open` (its `<`): (first token, last token) of each, and
/// the index of its `>`. None when it does not close
fn angle_parts(t: &[(u8, usize, usize)], text: &[u8], open: usize) -> Option<(Vec<(usize, usize)>, usize)> {
    let sym = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && text[t[k].1] == c;
    let (mut d, mut braces, mut parts, mut from) = (0i32, 0i32, vec![], open + 1);
    for k in open..t.len() {
        if sym(k, b'{') {
            braces += 1;
            continue;
        }
        if sym(k, b'}') {
            braces -= 1;
            continue;
        }
        if braces > 0 {
            continue;
        }
        if sym(k, b'<') || sym(k, b'(') || sym(k, b'[') {
            d += 1;
        } else if (sym(k, b'>') && !(k > 0 && sym(k - 1, b'-'))) || sym(k, b')') || sym(k, b']') {
            d -= 1;
            if d == 0 {
                if from < k {
                    parts.push((from, k - 1));
                }
                return Some((parts, k));
            }
        } else if d == 1 && sym(k, b',') {
            if from < k {
                parts.push((from, k - 1));
            }
            from = k + 1;
        }
    }
    None
}

/// E0107 at an impl of one of the program's traits that gives it more generic (type / const) arguments than the
/// trait declares, or fewer than it requires: `impl Seq<bool> for u32` with `trait Seq {}`. Read from the trait
/// headers and the impl headers only (lifetimes and `Item = ..` bindings are not counted)
fn first_trait_arity_error(src: &str, orig: &str, items: &[SrcItem]) -> Option<String> {
    use crate::lexer::ItemKind;
    // (each trait's (required, declared) count; a name declared twice is left out)
    let mut decl: crate::fxhash::FxMap<&str, Option<(usize, usize)>> = Default::default();
    for it in items.iter().filter(|it| matches!(it.kind, ItemKind::TraitHeader)) {
        let text = &src[it.start..it.end];
        let b = text.as_bytes();
        let t = crate::lexer::tokenize(b);
        let w = |k: usize| if k < t.len() && t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" };
        let Some(k) = (0..t.len()).find(|&k| w(k) == "trait") else { continue };
        let name = w(k + 1);
        if name.is_empty() {
            continue;
        }
        let counts = if k + 2 < t.len() && t[k + 2].0 == 1 && b[t[k + 2].1] == b'<' {
            let Some((parts, _)) = angle_parts(&t, b, k + 2) else { continue };
            // (a lifetime, `'a`, whose token may start after its quote)
            let lifetime = |q: usize| b[t[q].1] == b'\'' || (t[q].1 > 0 && b[t[q].1 - 1] == b'\'');
            let tys: Vec<&(usize, usize)> = parts.iter().filter(|p| !lifetime(p.0)).collect();
            // (braces, a const expression's `{ 1 > 0 }`, are skipped whole)
            let top_eq = |p: &(usize, usize)| {
                let (mut d, mut braces) = (0i32, 0i32);
                (p.0..=p.1).any(|q| {
                    if t[q].0 == 1 {
                        match b[t[q].1] {
                            b'{' => braces += 1,
                            b'}' => braces -= 1,
                            _ if braces > 0 => {}
                            b'<' | b'(' | b'[' => d += 1,
                            b'>' if !(q > 0 && t[q - 1].0 == 1 && b[t[q - 1].1] == b'-') => d -= 1,
                            b')' | b']' => d -= 1,
                            b'=' => return d == 0,
                            _ => {}
                        }
                    }
                    false
                })
            };
            let defaults = tys.iter().filter(|p| top_eq(p)).count();
            (tys.len() - defaults, tys.len())
        } else {
            (0, 0)
        };
        decl.entry(name).and_modify(|e| *e = None).or_insert(Some(counts));
    }
    if decl.is_empty() {
        return None;
    }
    let plural = |n: usize| if n == 1 { "generic argument" } else { "generic arguments" };
    for it in items.iter().filter(|it| matches!(it.kind, ItemKind::ImplHeader)) {
        let text = &src[it.start..it.end];
        let b = text.as_bytes();
        let t = crate::lexer::tokenize(b);
        let w = |k: usize| if k < t.len() && t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" };
        let sym = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && b[t[k].1] == c;
        let Some(mut k) = (0..t.len()).find(|&k| w(k) == "impl") else { continue };
        k += 1;
        if sym(k, b'<') {
            let Some((_, close)) = angle_parts(&t, b, k) else { continue };
            k = close + 1;
        }
        // (the trait's path, up to `for`: its last segment and its arguments)
        if sym(k, b'!') {
            continue;
        }
        let mut name_at = None;
        while k < t.len() && w(k) != "for" && !sym(k, b'{') {
            if t[k].0 == 0 && w(k) != "dyn" {
                name_at = Some(k);
                k += 1;
                if sym(k, b'<') {
                    let Some((_, close)) = angle_parts(&t, b, k) else { break };
                    k = close + 1;
                }
                continue;
            }
            k += 1;
        }
        if w(k) != "for" {
            continue;
        }
        let Some(nk) = name_at else { continue };
        let Some(&Some((required, declared))) = decl.get(w(nk)) else { continue };
        let given = if sym(nk + 1, b'<') {
            let Some((parts, _)) = angle_parts(&t, b, nk + 1) else { continue };
            parts
                .iter()
                .filter(|p| !(b[t[p.0].1] == b'\'' || (t[p.0].1 > 0 && b[t[p.0].1 - 1] == b'\'')) && !(t[p.0].0 == 0 && p.1 > p.0 && sym(p.0 + 1, b'=') && !sym(p.0 + 2, b'=')))
                .count()
        } else {
            0
        };
        if given > declared || given < required {
            let what = if given == 0 {
                format!("missing generics for trait `{}`", w(nk))
            } else {
                let takes = if required == declared { format!("{declared}") } else if given > declared { format!("at most {declared}") } else { format!("at least {required}") };
                format!("trait takes {takes} {} but {given} {} {} supplied", plural(declared.max(required)), plural(given), if given == 1 { "was" } else { "were" })
            };
            // (rustc points at the trait's name)
            let (line, col) = item_pos(src, orig, &crate::prep::hoist_marks(src), it, t[nk].1);
            return Some(format!("{line}:{col}: {what} (E0107)"));
        }
    }
    None
}

/// E0107 at a use of one of the program's generic associated types with the wrong number of arguments: none at all
/// (`Self::Item` / `Tr<Item = X>` for `type Item<'a>`: "missing generics"), too few / many lifetimes, or too few / many
/// types and consts. Only uses whose trait is known are read: `Self::A` in the trait or in an impl of it, `<T as Tr>::A`,
/// `<Self>::A`, and the binding `Tr<A = ..>`. Nothing is tokenized unless a trait declares a generic associated type
fn first_gat_arity_error(src: &str, orig: &str, items: &[SrcItem]) -> Option<String> {
    use crate::lexer::ItemKind;
    let id = |&(a, b): &(u32, u32)| &src[a as usize..b as usize];
    // (the trait an item's `Self` implements or is: a trait's members, an impl's members of `impl Tr for X`)
    let trait_of_header = |h: usize| -> Option<&str> {
        let it = items.get(h)?;
        match it.kind {
            ItemKind::TraitHeader => it.idents.iter().position(|x| id(x) == "trait").and_then(|k| it.idents.get(k + 1)).map(id),
            ItemKind::ImplHeader => {
                let text = &src[it.start..it.end];
                let b = text.as_bytes();
                let t = crate::lexer::tokenize(b);
                let w = |k: usize| if k < t.len() && t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" };
                let sym = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && b[t[k].1] == c;
                let f = (0..t.len()).find(|&k| w(k) == "for")?;
                // (the last segment before `for`, past its own `<..>`)
                let mut k = f.checked_sub(1)?;
                if sym(k, b'>') {
                    let mut d = 0i32;
                    loop {
                        if sym(k, b'>') && !(k > 0 && sym(k - 1, b'-')) {
                            d += 1;
                        } else if sym(k, b'<') {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        k = k.checked_sub(1)?;
                    }
                    k = k.checked_sub(1)?;
                }
                (t[k].0 == 0).then(|| w(k))
            }
            _ => None,
        }
    };
    // (the generic associated types: (trait, name) -> (lifetimes, types and consts))
    let mut gats: crate::fxhash::FxMap<(&str, &str), (usize, usize)> = Default::default();
    for it in items {
        let ItemKind::TraitMember(h) = it.kind else { continue };
        let Some(k) = it.idents.iter().take(4).position(|x| id(x) == "type") else { continue };
        let Some(&(_, ne)) = it.idents.get(k + 1) else { continue };
        if !src[ne as usize..it.end].trim_start().starts_with('<') {
            continue;
        }
        let Some(tr) = trait_of_header(h as usize) else { continue };
        let text = &src[ne as usize..it.end];
        let b = text.as_bytes();
        let t = crate::lexer::tokenize(b);
        let Some((parts, _)) = angle_parts(&t, b, 0) else { continue };
        let lifetime = |q: usize| b[t[q].1] == b'\'' || (t[q].1 > 0 && b[t[q].1 - 1] == b'\'');
        let lts = parts.iter().filter(|p| lifetime(p.0)).count();
        gats.insert((tr, id(&it.idents[k + 1])), (lts, parts.len() - lts));
    }
    if gats.is_empty() {
        return None;
    }
    let names: Vec<&str> = gats.keys().map(|k| k.1).collect();
    let marks = crate::prep::hoist_marks(src);
    for (i, it) in items.iter().enumerate() {
        if !it.idents.iter().any(|x| names.contains(&id(x))) {
            continue;
        }
        let self_trait = match it.kind {
            ItemKind::TraitMember(h) | ItemKind::ImplMember(h) => trait_of_header(h as usize),
            ItemKind::TraitHeader => trait_of_header(i),
            _ => None,
        };
        let text = &src[it.start..it.end];
        let b = text.as_bytes();
        let t = crate::lexer::tokenize(b);
        let w = |k: usize| if k < t.len() && t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" };
        let sym = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && b[t[k].1] == c;
        // (the opener of the group a closing `>` / `)` / `]` at k ends, reading back)
        let opener = |mut k: usize| -> Option<usize> {
            let mut d = 0i32;
            loop {
                if (sym(k, b'>') && !(k > 0 && sym(k - 1, b'-'))) || sym(k, b')') || sym(k, b']') {
                    d += 1;
                } else if sym(k, b'<') || sym(k, b'(') || sym(k, b'[') {
                    d -= 1;
                    if d == 0 {
                        return Some(k);
                    }
                }
                k = k.checked_sub(1)?;
            }
        };
        for k in 1..t.len() {
            if t[k].0 != 0 || !names.contains(&w(k)) || w(k - 1) == "type" {
                continue;
            }
            let name = w(k);
            // (whose trait: `Self::A` / `<Self>::A` / `<T as Tr>::A`, or the binding `Tr<.., A = ..>`)
            let mut tr = None;
            if k >= 3 && sym(k - 1, b':') && sym(k - 2, b':') {
                if w(k - 3) == "Self" && !(k >= 5 && sym(k - 4, b':') && sym(k - 5, b':')) {
                    tr = self_trait;
                } else if sym(k - 3, b'>') {
                    if let Some(o) = opener(k - 3) {
                        if o + 2 == k - 3 && w(o + 1) == "Self" {
                            tr = self_trait;
                        } else {
                            // (the `as` at the group's own level: not one in `<<T as A>::X as Tr>`'s inner group)
                            let mut d = 0i32;
                            let as_at = (o + 1..k - 3).find(|&q| {
                                if sym(q, b'<') {
                                    d += 1;
                                } else if sym(q, b'>') && !sym(q - 1, b'-') {
                                    d -= 1;
                                }
                                d == 0 && w(q) == "as"
                            });
                            // (the trait after it: its path's last segment, `<U as m::Tr<u8>>::A`)
                            if let Some(a) = as_at {
                                let mut q = a + 1;
                                while t[q].0 == 0 && sym(q + 1, b':') && sym(q + 2, b':') && q + 3 < k - 3 && t[q + 3].0 == 0 {
                                    q += 3;
                                }
                                if q < k - 3 && t[q].0 == 0 && (q + 1 == k - 3 || sym(q + 1, b'<')) {
                                    tr = Some(w(q));
                                }
                            }
                        }
                    }
                }
            } else if sym(k - 1, b'<') || sym(k - 1, b',') {
                // (a binding: `A = ..` / `A<..> = ..`, not `==` / `=>`)
                let mut e = k + 1;
                if sym(e, b'<') {
                    let Some((_, close)) = angle_parts(&t, b, e) else { continue };
                    e = close + 1;
                }
                let eq = sym(e, b'=') && !(sym(e + 1, b'=') || sym(e + 1, b'>'));
                if !eq {
                    continue;
                }
                // (the `<` the list is in, reading back over balanced groups, and the trait's name before it)
                let mut q = k - 1;
                let open = loop {
                    if sym(q, b'<') {
                        break Some(q);
                    }
                    if sym(q, b';') || sym(q, b'{') || sym(q, b'}') || sym(q, b'(') || sym(q, b'[') {
                        break None;
                    }
                    if (sym(q, b'>') && !(q > 0 && sym(q - 1, b'-'))) || sym(q, b')') || sym(q, b']') {
                        let Some(o) = opener(q) else { break None };
                        q = o;
                    }
                    let Some(n) = q.checked_sub(1) else { break None };
                    q = n;
                };
                if let Some(o) = open.filter(|&o| o > 0 && t[o - 1].0 == 0) {
                    tr = Some(w(o - 1));
                }
            }
            let Some(tr) = tr else { continue };
            let Some(&(lts, tys)) = gats.get(&(tr, name)) else { continue };
            // (its arguments, `Self::A<'a>` or `Self::A::<'a>`)
            let args = if sym(k + 1, b'<') {
                Some(k + 1)
            } else {
                (sym(k + 1, b':') && sym(k + 2, b':') && sym(k + 3, b'<')).then_some(k + 3)
            };
            // (a call of an associated fn of the name, `Self::A()` / `Self::A::<u8>()`: the value namespace)
            let next = match args {
                Some(open) => angle_parts(&t, b, open).map(|(_, c)| c + 1),
                None => Some(k + 1),
            };
            if next.is_some_and(|q| sym(q, b'(')) {
                continue;
            }
            let (glts, gtys) = if let Some(open) = args {
                let Some((parts, _)) = angle_parts(&t, b, open) else { continue };
                let lifetime = |q: usize| b[t[q].1] == b'\'' || (t[q].1 > 0 && b[t[q].1 - 1] == b'\'');
                let binding = |p: &(usize, usize)| t[p.0].0 == 0 && p.1 > p.0 && sym(p.0 + 1, b'=') && !sym(p.0 + 2, b'=');
                let l = parts.iter().filter(|p| lifetime(p.0)).count();
                (Some(l), parts.iter().filter(|p| !lifetime(p.0) && !binding(p)).count())
            } else {
                (None, 0)
            };
            let what = match glts {
                None => format!("missing generics for associated type `{tr}::{name}`"),
                Some(g) if g != lts => format!("associated type takes {lts} lifetime argument{} but {g} lifetime argument{} supplied", if lts == 1 { "" } else { "s" }, if g == 1 { " was" } else { "s were" }),
                Some(_) if gtys != tys => format!("associated type takes {tys} generic argument{} but {gtys} generic argument{} supplied", if tys == 1 { "" } else { "s" }, if gtys == 1 { " was" } else { "s were" }),
                _ => continue,
            };
            let (line, col) = item_pos(src, orig, &marks, it, t[k].1);
            return Some(format!("{line}:{col}: {what} (E0107)"));
        }
    }
    None
}

/// One trait bound of a bound list (`Super<X = u8> + Zed + 'a`), from the token at `k`: (its first token, its name's
/// token, the parts of its `<..>`, the token after it). None at a lifetime, `?Sized`, `Fn(..)` (whose sugar binds
/// `Output`) or anything else that is not a path
fn bound_at(t: &[(u8, usize, usize)], b: &[u8], mut k: usize) -> Option<(usize, usize, Vec<(usize, usize)>, usize)> {
    let sym = |q: usize, c: u8| q < t.len() && t[q].0 == 1 && b[t[q].1] == c;
    let word = |q: usize| q < t.len() && t[q].0 == 0;
    // (`for<'a> Tr<'a>`)
    if word(k) && &b[t[k].1..t[k].2] == b"for" && sym(k + 1, b'<') {
        k = angle_parts(t, b, k + 1)?.1 + 1;
    }
    let at = k;
    if sym(k, b':') && sym(k + 1, b':') {
        k += 2;
    }
    if !word(k) || (t[k].1 > 0 && b[t[k].1 - 1] == b'\'') {
        return None;
    }
    while word(k) && sym(k + 1, b':') && sym(k + 2, b':') && word(k + 3) {
        k += 3;
    }
    let name = k;
    let mut parts = vec![];
    k += 1;
    // (`Iterator::<Item = u8>`)
    if sym(k, b':') && sym(k + 1, b':') && sym(k + 2, b'<') {
        k += 2;
    }
    if sym(k, b'<') {
        let (p, c) = angle_parts(t, b, k)?;
        parts = p;
        k = c + 1;
    } else if sym(k, b'(') {
        return None;
    }
    Some((at, name, parts, k))
}

/// The associated types a `<..>`'s parts bind (`Item = u8`, `Item: Copy`); a supertrait's binding whose value names
/// `Self` (`Base<Output = <Self as Helper>::Target>`) is left out with `self_free`, as dyn does not know `Self`
fn assoc_bindings<'a>(t: &[(u8, usize, usize)], text: &'a str, parts: &[(usize, usize)], self_free: bool) -> Vec<&'a str> {
    let b = text.as_bytes();
    let sym = |q: usize, c: u8| q < t.len() && t[q].0 == 1 && b[t[q].1] == c;
    let mut bound = vec![];
    for &(a, z) in parts {
        let binds = sym(a + 1, b'=') && !sym(a + 2, b'=') || sym(a + 1, b':') && !sym(a + 2, b':');
        if t[a].0 == 0 && a < z && binds && !(t[a].1 > 0 && b[t[a].1 - 1] == b'\'') && (!self_free || !(a + 2..=z).any(|q| t[q].0 == 0 && &text[t[q].1..t[q].2] == "Self")) {
            bound.push(&text[t[a].1..t[a].2]);
        }
    }
    bound
}

/// The arguments of a trait as shown_args shows it (`Sup<A, Vec<u8>>`: `A` and `Vec<u8>`), none without `<..>`
fn type_args(shown: &str) -> Vec<&str> {
    let (Some(a), Some(z)) = (shown.find('<'), shown.rfind('>')) else { return vec![] };
    let inner = &shown[a + 1..z.max(a + 1)];
    let (mut out, mut d, mut from) = (vec![], 0i32, 0);
    for (i, c) in inner.char_indices() {
        match c {
            '<' | '(' | '[' => d += 1,
            '>' if !inner[..i].ends_with('-') => d -= 1,
            ')' | ']' => d -= 1,
            ',' if d == 0 => {
                out.push(inner[from..i].trim());
                from = i + 1;
            }
            _ => {}
        }
    }
    out.push(inner[from..].trim());
    out
}

/// `s` with each identifier that is one of `params` replaced by the argument at its place (one past `args` stays)
fn subst_idents(s: &str, params: &[&str], args: &[&str]) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let j = i + b[i..].iter().position(|c| !(c.is_ascii_alphanumeric() || *c == b'_')).unwrap_or(b.len() - i);
            let word = &s[i..j];
            let lifetime = i > 0 && b[i - 1] == b'\'';
            // (a segment after `::` names an associated item, `<u8 as Id>::T`, not a parameter)
            let path_segment = i >= 2 && &b[i - 2..i] == b"::";
            match params.iter().position(|p| *p == word).and_then(|k| args.get(k)).filter(|_| !lifetime && !path_segment) {
                Some(a) => out.push_str(a),
                None => out.push_str(word),
            }
            i = j;
        } else {
            let j = i + s[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&s[i..j]);
            i = j;
        }
    }
    out
}

/// A `<..>`'s type and const arguments as rustc shows them (`<(), Vec<u8>>`), or "" without any
fn shown_args(t: &[(u8, usize, usize)], text: &str, parts: &[(usize, usize)]) -> String {
    let b = text.as_bytes();
    let sym = |q: usize, c: u8| q < t.len() && t[q].0 == 1 && b[t[q].1] == c;
    let mut shown = vec![];
    for &(a, z) in parts {
        let binds = sym(a + 1, b'=') && !sym(a + 2, b'=') || sym(a + 1, b':') && !sym(a + 2, b':');
        let lifetime = t[a].1 > 0 && b[t[a].1 - 1] == b'\'';
        if !(t[a].0 == 0 && a < z && binds && !lifetime) {
            shown.push(text[t[a].1 - usize::from(lifetime)..t[z].2].split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    if shown.is_empty() { String::new() } else { format!("<{}>", shown.join(", ")) }
}

/// E0191 at a trait object written in a type (`type Foo = dyn Tr;`, `fn f(x: &dyn Tr)`) that leaves an associated
/// type of its trait, or of a supertrait, without a value: the program's traits and a few of std's. A generic
/// associated type makes the trait not dyn compatible instead (E0038), and `where Self: Sized` exempts a type
fn first_dyn_unspecified_assoc(src: &str, orig: &str, items: &[SrcItem]) -> Option<String> {
    use crate::lexer::ItemKind;
    // (the `dyn`s of the program: identifiers the lexer found, so none in a string or a comment)
    let sb = src.as_bytes();
    let dyns: Vec<(usize, usize, Option<(u32, u32)>)> = items
        .iter()
        .enumerate()
        .flat_map(|(i, it)| {
            let ids = &it.idents;
            (0..ids.len()).filter(move |&k| ids[k].1 - ids[k].0 == 3 && &sb[ids[k].0 as usize..ids[k].1 as usize] == b"dyn").map(move |k| (i, ids[k].0 as usize, ids.get(k + 1).copied()))
        })
        .collect();
    if dyns.is_empty() {
        return None;
    }
    // (the auto traits, which an object may add to its principal trait: `dyn Send + Iterator`)
    const AUTO: &[&str] = &["Send", "Sync", "Unpin", "UnwindSafe", "RefUnwindSafe"];
    // (the object's principal trait from the token after `dyn`: the first bound not an auto trait or a lifetime; and
    // whether the bounds read may go on past the window's end: one runs to it, or a `<` is not closed in it)
    fn principal(t: &[(u8, usize, usize)], b: &[u8]) -> (Option<(usize, usize, Vec<(usize, usize)>, usize)>, bool) {
        let sym = |q: usize, c: u8| q < t.len() && t[q].0 == 1 && b[t[q].1] == c;
        let mut k = 1;
        while k < t.len() {
            if t[k].0 == 0 && t[k].1 > 0 && b[t[k].1 - 1] == b'\'' {
                k += 1;
            } else {
                match bound_at(t, b, k) {
                    Some(bd) if !AUTO.iter().any(|a| a.as_bytes() == &b[t[bd.1].1..t[bd.1].2]) => {
                        let open = bd.3 >= t.len();
                        return (Some(bd), open);
                    }
                    Some(bd) => k = bd.3,
                    None => {
                        let opens = (k..t.len()).filter(|&q| sym(q, b'<')).count();
                        let closes = (k..t.len()).filter(|&q| sym(q, b'>') && !sym(q - 1, b'-')).count();
                        return (None, opens > closes);
                    }
                }
            }
            if !sym(k, b'+') {
                return (None, k >= t.len());
            }
            k += 1;
        }
        (None, true)
    }
    // (std's traits with associated types: (name, associated types, supertraits))
    const STD: &[(&str, &[&str], &[&str])] = &[
        ("Iterator", &["Item"], &[]),
        ("DoubleEndedIterator", &[], &["Iterator"]),
        ("ExactSizeIterator", &[], &["Iterator"]),
        ("FusedIterator", &[], &["Iterator"]),
        ("IntoIterator", &["Item", "IntoIter"], &[]),
        ("Deref", &["Target"], &[]),
        ("DerefMut", &[], &["Deref"]),
        ("Future", &["Output"], &[]),
    ];
    // (the program's traits: name -> (associated types, supertraits as (name, shown, the types the bound binds), how
    // many type and const parameters it requires and takes, and their names), or None when one has a generic
    // associated type or two traits share the name)
    type Sup<'s> = (&'s str, String, Vec<&'s str>);
    type Trait<'s> = (Vec<&'s str>, Vec<Sup<'s>>, (usize, usize), Vec<&'s str>);
    let id = |&(a, b): &(u32, u32)| &src[a as usize..b as usize];
    // (a trait as rustc shows it: without the flattened modules' `a__` prefix and the renamer's `__u_`)
    let plain = |n: &str, args: &str| format!("{}{}", n.rsplit("__").next().unwrap_or(n).trim_start_matches("u_"), args.replace("__u_", ""));
    let mut traits: crate::fxhash::FxMap<&str, Option<Trait>> = Default::default();
    let mut header_of: crate::fxhash::FxMap<usize, &str> = Default::default();
    // (the defaults of a trait's type and const parameters, for a trait with any)
    let mut defaults_of: crate::fxhash::FxMap<&str, Vec<Option<String>>> = Default::default();
    for (i, it) in items.iter().enumerate() {
        if !matches!(it.kind, ItemKind::TraitHeader) {
            continue;
        }
        let Some(k) = it.idents.iter().position(|x| id(x) == "trait") else { continue };
        let Some(&(_, ne)) = it.idents.get(k + 1) else { continue };
        let name = id(&it.idents[k + 1]);
        header_of.insert(i, name);
        if traits.contains_key(name) {
            traits.insert(name, None);
            continue;
        }
        // (`trait Tr {`: no parameters and no supertraits, nothing to read)
        if !src[ne as usize..it.end].contains(['<', ':']) {
            traits.insert(name, Some((vec![], vec![], (0, 0), vec![])));
            continue;
        }
        let text = &src[it.start..it.end];
        let b = text.as_bytes();
        let t = crate::lexer::tokenize(b);
        let sym = |q: usize, c: u8| q < t.len() && t[q].0 == 1 && b[t[q].1] == c;
        let w = |q: usize| if q < t.len() && t[q].0 == 0 { &text[t[q].1..t[q].2] } else { "" };
        let mut sup = vec![];
        let mut params = (0, 0);
        let mut pnames = vec![];
        let mut pdefaults: Vec<Option<String>> = vec![];
        // (a bound list from q (`Super<X = u8> + Zed + 'a`), up to `where`, `{`, or with `clause` a `,` that ends a
        // where clause: where it ends, and its traits)
        let read_bounds = |mut q: usize, clause: bool| {
            let mut sup = vec![];
            let end = |q: usize, d: i32| q >= t.len() || w(q) == "where" || sym(q, b'{') || (clause && d == 0 && sym(q, b','));
            while !end(q, 0) {
                match bound_at(&t, b, q) {
                    Some((_, n, parts, after)) => {
                        sup.push((w(n), plain(w(n), &shown_args(&t, text, &parts)), assoc_bindings(&t, text, &parts, true)));
                        q = after;
                    }
                    None => q += 1,
                }
                // (to the next `+` of the list, past a group: `Fn(A, B)`)
                let mut d = 0i32;
                while !end(q, d) && !(d == 0 && sym(q, b'+')) {
                    if sym(q, b'(') || sym(q, b'[') || sym(q, b'<') {
                        d += 1;
                    } else if sym(q, b')') || sym(q, b']') || (sym(q, b'>') && !sym(q - 1, b'-')) {
                        d -= 1;
                    }
                    q += 1;
                }
                q += usize::from(sym(q, b'+'));
            }
            (q, sup)
        };
        if let Some(mut q) = (0..t.len()).find(|&q| w(q) == "trait" && w(q + 1) == name).map(|q| q + 2) {
            if sym(q, b'<') {
                // (`<'a, T, U = u8, const N: usize>`: a default is an `=` at the parameter's own level)
                let Some((ps, c)) = angle_parts(&t, b, q) else { continue };
                for &(a, z) in &ps {
                    if t[a].1 > 0 && b[t[a].1 - 1] == b'\'' {
                        continue;
                    }
                    pnames.push(if w(a) == "const" { w(a + 1) } else { w(a) });
                    let mut d = 0i32;
                    let default = (a..=z).find(|&r| {
                        if sym(r, b'<') {
                            d += 1;
                        } else if sym(r, b'>') && !(r > 0 && sym(r - 1, b'-')) {
                            d -= 1;
                        }
                        d == 0 && sym(r, b'=')
                    });
                    pdefaults.push(default.filter(|&r| r < z).map(|r| text[t[r + 1].1..t[z].2].split_whitespace().collect::<Vec<_>>().join(" ")));
                    params = (params.0 + usize::from(default.is_none()), params.1 + 1);
                }
                q = c + 1;
            }
            if sym(q, b':') && !sym(q + 1, b':') {
                let (e, list) = read_bounds(q + 1, false);
                sup = list;
                q = e;
            }
            // (`where Self: Base` is a supertrait too)
            if w(q) == "where" {
                q += 1;
                while q < t.len() && !sym(q, b'{') {
                    if w(q) == "Self" && sym(q + 1, b':') && !sym(q + 2, b':') {
                        let (e, more) = read_bounds(q + 2, true);
                        sup.extend(more);
                        q = e;
                    } else {
                        let mut d = 0i32;
                        while q < t.len() && !sym(q, b'{') && !(d == 0 && sym(q, b',')) {
                            if sym(q, b'(') || sym(q, b'[') || sym(q, b'<') {
                                d += 1;
                            } else if sym(q, b')') || sym(q, b']') || (sym(q, b'>') && !sym(q - 1, b'-')) {
                                d -= 1;
                            }
                            q += 1;
                        }
                    }
                    q += usize::from(sym(q, b','));
                }
            }
        }
        if pdefaults.iter().any(Option::is_some) {
            defaults_of.insert(name, pdefaults);
        }
        traits.insert(name, Some((vec![], sup, params, pnames)));
    }
    for it in items {
        let ItemKind::TraitMember(h) = it.kind else { continue };
        let Some(&tr) = header_of.get(&(h as usize)) else { continue };
        let Some(k) = it.idents.iter().take(6).position(|x| id(x) == "type") else { continue };
        let Some(&(_, ne)) = it.idents.get(k + 1) else { continue };
        let rest = &src[ne as usize..it.end];
        if rest.trim_start().starts_with('<') {
            traits.insert(tr, None);
            continue;
        }
        // (`where Self: Sized`, or a bound that implies it (`where Self: Other` with `trait Other: Sized`): one that
        // bounds `Self` at all is left alone)
        if rest.find("where").is_some_and(|w| rest[w..].contains("Self")) {
            continue;
        }
        if let Some(Some(entry)) = traits.get_mut(tr) {
            entry.0.push(id(&it.idents[k + 1]));
        }
    }
    // (a trait's type and const arguments with its defaults for the ones left out, a default naming an earlier
    // parameter taking that one's argument (`trait Base<T, U = T>`: `Base<u8>` is `Base<u8, u8>`); None when there is
    // nothing to fill or a left-out one has no default)
    let filled_args = |tn: &str, tys: &[&str]| -> Option<Vec<String>> {
        let ds = defaults_of.get(tn)?;
        if tys.len() >= ds.len() {
            return None;
        }
        let ps: &[&str] = match traits.get(tn) {
            Some(Some((_, _, _, ps))) => ps,
            _ => &[],
        };
        let mut all: Vec<String> = tys.iter().map(|a| a.to_string()).collect();
        for d in &ds[tys.len()..] {
            let d = d.as_deref()?;
            let k = all.len().min(ps.len());
            let refs: Vec<&str> = all.iter().map(String::as_str).collect();
            all.push(subst_idents(d, &ps[..k], &refs[..k]));
        }
        Some(all)
    };
    for (i, off, next) in dyns {
        let it = &items[i];
        // (`dyn Handler` of a trait with no associated types and no supertraits, `dyn Any` of a trait this does not
        // know: settled by the name after `dyn`, when it is the whole path, without reading tokens)
        if let Some(n) = next.filter(|&(a, b)| src[off + 3..a as usize].trim().is_empty() && sb.get(b as usize) != Some(&b':')).map(|x| id(&x)).filter(|n| !AUTO.contains(n)) {
            let simple = match traits.get(n) {
                Some(Some((a, sups, (req, _), _))) => a.is_empty() && sups.is_empty() && *req == 0,
                Some(None) => true,
                None => !STD.iter().any(|s| s.0 == n),
            };
            if simple {
                continue;
            }
        }
        // (the tokens from the `dyn` on, as far as its bound goes: a window that grows while the bound's `<..>` is
        // not closed in it)
        let mut len = 48;
        let (text, t, bound) = loop {
            let mut end = (off + len).min(it.end);
            while !src.is_char_boundary(end) {
                end -= 1;
            }
            let text = &src[off..end];
            let t = crate::lexer::tokenize(text.as_bytes());
            let (bound, open) = principal(&t, text.as_bytes());
            if !open || end == it.end || len >= 1 << 16 {
                break (text, t, bound);
            }
            len *= 4;
        };
        let b = text.as_bytes();
        let w = |q: usize| if q < t.len() && t[q].0 == 0 { &text[t[q].1..t[q].2] } else { "" };
        {
            let Some((q, n, parts, _)) = bound else { continue };
            let name = w(n);
            let bound = assoc_bindings(&t, text, &parts, false);
            // (the type and const arguments: a wrong count is E0107 instead, as rustc)
            let sym = |r: usize, c: u8| r < t.len() && t[r].0 == 1 && b[t[r].1] == c;
            let args = parts
                .iter()
                .filter(|&&(a, z)| !(t[a].1 > 0 && b[t[a].1 - 1] == b'\'') && !(t[a].0 == 0 && a < z && (sym(a + 1, b'=') && !sym(a + 2, b'=') || sym(a + 1, b':') && !sym(a + 2, b':'))))
                .count();
            // (`std::iter::Iterator` is std's even when the program has a trait of the name)
            let first_seg = if w(q).is_empty() { w(q + 2) } else { w(q) };
            let std_path = n > q && matches!(first_seg, "std" | "core" | "alloc");
            // (the usual case first: a trait without supertraits whose every associated type the object binds, or one
            // left to another error (a wrong count of arguments, a generic associated type, not a trait this knows))
            let in_std = STD.iter().find(|s| s.0 == name);
            // (a wrong count of the program's trait's type and const arguments: E0107 at its name, as rustc)
            if let Some(Some((_, _, (req, all), _))) = traits.get(name).filter(|_| !(std_path && in_std.is_some())) {
                let (required, declared) = (*req, *all);
                if args < required || args > declared {
                    let plural = |n: usize| if n == 1 { "generic argument" } else { "generic arguments" };
                    // (`dyn T` without `<..>`; `dyn T<A = u8>` is "takes 1 .. but 0")
                    let what = if parts.is_empty() {
                        format!("missing generics for trait `{}`", plain(name, ""))
                    } else {
                        let takes = if required == declared { format!("{declared}") } else if args > declared { format!("at most {declared}") } else { format!("at least {required}") };
                        format!("trait takes {takes} {} but {args} {} {} supplied", plural(declared.max(required)), plural(args), if args == 1 { "was" } else { "were" })
                    };
                    let (line, col) = item_pos(src, orig, &crate::prep::hoist_marks(src), it, off - it.start + t[n].1);
                    return Some(format!("{line}:{col}: {what} (E0107)"));
                }
            }
            let settled = match traits.get(name) {
                Some(None) => true,
                Some(Some((a, sups, (req, all), _))) if !(std_path && in_std.is_some()) => args < *req || args > *all || sups.is_empty() && a.iter().all(|x| bound.contains(x)),
                _ => in_std.is_none_or(|s| args > 0 || s.2.is_empty() && s.1.iter().all(|x| bound.contains(x))),
            };
            if settled {
                continue;
            }
            let shown = shown_args(&t, text, &parts);
            // (breadth first from the trait: a binding written on the object goes to the first trait that has the
            // name; a supertrait's binding to that supertrait, by whichever path it is reached (`A2: Service<Future =
            // F> + A1` with `A1: Service<Response = i32>` binds both))
            let mut pending = bound;
            // (std's trait beside the program's of the name: rustc shows its path)
            let path: String = if std_path && (traits.contains_key(name) || traits.contains_key(format!("__u_{name}").as_str())) { text[t[q].1..t[n].2].split_whitespace().collect() } else { plain(name, "") };
            let mut queue: Vec<Sup> = vec![(name, format!("{path}{}", plain("", &shown)), vec![])];
            let mut order: Vec<(&str, String, Vec<&str>, String)> = vec![];
            // (an instantiation's key: its arguments with the trait's defaults for the ones left out, so `Base` and
            // `Base<u8>` of `trait Base<T = u8>` are one)
            let key = |tn: &str, ts: &str| -> String {
                let (lts, tys): (Vec<&str>, Vec<&str>) = type_args(ts).into_iter().partition(|a| a.starts_with('\''));
                match filled_args(tn, &tys) {
                    Some(all) => format!("{}<{}>", &ts[..ts.find('<').unwrap_or(ts.len())], lts.into_iter().map(str::to_string).chain(all).collect::<Vec<_>>().join(", ")),
                    None => ts.to_string(),
                }
            };
            // (the bindings each instantiation gets: `Sup<i32>` and `Sup<u32>` are two)
            let mut bound_by: crate::fxhash::FxMap<String, Vec<&str>> = Default::default();
            let mut seen: Vec<String> = vec![];
            let mut known = true;
            let mut n = 0;
            while n < queue.len() && n < 64 {
                let (tn, ts, local) = queue[n].clone();
                n += 1;
                let k = key(tn, &ts);
                bound_by.entry(k.clone()).or_default().extend(local);
                if seen.contains(&k) {
                    continue;
                }
                seen.push(k.clone());
                let in_std = STD.iter().find(|s| s.0 == tn);
                let (assoc, sups): (Vec<&str>, Vec<Sup>) = match traits.get(tn) {
                    Some(Some((_, _, (req, all), _))) if n == 1 && !(std_path && in_std.is_some()) && (args < *req || args > *all) => {
                        known = false;
                        break;
                    }
                    Some(Some((a, s, _, ps))) if !(std_path && n == 1 && in_std.is_some()) => {
                        // (a supertrait as this instantiation has it: `trait Dyn<A, B>: Sup<B>` of `Dyn<i32, u32>` is
                        // `Sup<u32>`)
                        // (by the type and const arguments, defaults filled: a lifetime is not one of `ps`)
                        let args: Vec<&str> = type_args(&k).into_iter().filter(|a| !a.starts_with('\'')).collect();
                        let s = s.iter().map(|(sn, sts, sb)| (*sn, if ps.is_empty() || args.is_empty() { sts.clone() } else { subst_idents(sts, ps, &args) }, sb.clone())).collect();
                        (a.clone(), s)
                    }
                    Some(None) => {
                        known = false;
                        break;
                    }
                    _ => match in_std {
                        Some(_) if n == 1 && args > 0 => {
                            known = false;
                            break;
                        }
                        Some(s) => (s.1.to_vec(), s.2.iter().map(|x| (*x, x.to_string(), vec![])).collect()),
                        None if n == 1 => {
                            known = false;
                            break;
                        }
                        None => (vec![], vec![]),
                    },
                };
                order.push((tn, ts, assoc, k));
                queue.extend(sups);
            }
            // (a walk cut short leaves the types of the traits it did not reach unknown)
            if n < queue.len() {
                known = false;
            }
            // (a binding of a name the trait does not have itself, but two of its supertraits do, binds neither: E0222)
            if let Some((_, _, own, _)) = order.first() {
                pending.retain(|p| own.contains(p) || order[1..].iter().filter(|o| o.2.contains(p)).count() < 2);
            }
            let mut missing: Vec<(&str, String)> = vec![];
            for (_, ts, assoc, k) in &order {
                for &a in assoc {
                    if bound_by.get(k).is_some_and(|b| b.contains(&a)) {
                        continue;
                    }
                    if let Some(p) = pending.iter().position(|x| *x == a) {
                        pending.remove(p);
                        continue;
                    }
                    missing.push((a, ts.clone()));
                }
            }
            if !known || missing.is_empty() {
                continue;
            }
            // (as rustc prints a trait: without the trailing arguments that are its defaults, `Base<u8>` of `trait
            // Base<T = u8>` is `Base`)
            for (_, ts) in missing.iter_mut() {
                let Some(tn) = defaults_of.keys().find(|n| ts.starts_with(**n) && ts[n.len()..].starts_with('<')).copied() else { continue };
                let all = type_args(ts);
                let lts = all.iter().take_while(|a| a.starts_with('\'')).count();
                let mut keep = all.len();
                // (each trailing argument against its default as the arguments before it fill it)
                while keep > lts && filled_args(tn, &all[lts..keep - 1]).is_some_and(|f| f.get(keep - lts - 1).is_some_and(|d| d == all[keep - 1])) {
                    keep -= 1;
                }
                if keep < all.len() {
                    let base = &ts[..ts.find('<').unwrap_or(ts.len())];
                    *ts = if keep == 0 { base.to_string() } else { format!("{base}<{}>", all[..keep].join(", ")) };
                }
            }
            // (one trait: its types in the order it declares them; several: each `X` in `Tr`, sorted)
            let what = if missing.iter().all(|m| m.1 == missing[0].1) {
                let names: Vec<String> = missing.iter().map(|m| format!("`{}`", m.0)).collect();
                let list = match names.split_last() {
                    Some((l, rest)) if !rest.is_empty() => format!("{} and {l}", rest.join(", ")),
                    _ => names[0].clone(),
                };
                format!("type{} {list} in `{}`", if names.len() == 1 { "" } else { "s" }, missing[0].1)
            } else {
                let mut each: Vec<String> = missing.iter().map(|m| format!("`{}` in `{}`", m.0, m.1)).collect();
                each.sort();
                format!("types {}", each.join(", "))
            };
            let (line, col) = item_pos(src, orig, &crate::prep::hoist_marks(src), it, off - it.start + t[q].1);
            return Some(format!("{line}:{col}: the value of the associated {what} must be specified (E0191)"));
        }
    }
    None
}

/// アイテムの関数を自前の構文木で読む (本体がなければ None)。引数のパターンは本体の先頭の let にする
/// `#![deny(unused_imports)]`: the first name a top-level `use` brings in that no other item mentions. Traits
/// (std's usual ones and the program's) are left alone: their methods use them without naming them. `lint_denied`:
/// the crate denies the lint (or its group) itself; `warn_denied`: it denies `warnings`, which turns a warn into an error
fn first_unused_import(src: &str, items: &[SrcItem], lint_denied: bool, warn_denied: bool) -> Option<(usize, usize, String)> {
    const TRAITS: &[&str] = &[
        "Read", "Write", "BufRead", "Seek", "Hash", "Hasher", "BuildHasher", "FromStr", "Display", "Debug", "Iterator", "IntoIterator",
        "DoubleEndedIterator", "ExactSizeIterator", "FromIterator", "Extend", "Deref", "DerefMut", "Borrow", "BorrowMut", "AsRef", "AsMut",
        "Error", "Any", "TryFrom", "TryInto", "From", "Into", "ToString", "ToOwned", "Ord", "PartialOrd", "Eq", "PartialEq", "Add", "Sub",
        "Mul", "Div", "Rem", "Neg", "Not", "AddAssign", "SubAssign", "MulAssign", "DivAssign", "Index", "IndexMut", "Drop", "Clone", "Copy",
        "Default", "Fn", "FnMut", "FnOnce", "Future", "IntoFuture", "Sum", "Product", "Step", "Termination", "Pointer", "LowerHex", "UpperHex",
        "Octal", "Binary", "LowerExp", "UpperExp", "Send", "Sync", "Sized", "Unpin", "SliceIndex", "Pattern", "ToSocketAddrs", "OsStrExt",
        "OsStringExt", "PermissionsExt", "MetadataExt", "FileExt", "CommandExt", "ExitStatusExt", "AsRawFd", "FromRawFd", "IntoRawFd",
    ];
    let (Some(first), Some(last)) = (items.first(), items.last()) else { return None };
    let region = &src[first.start..last.end];
    let b = region.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let word = |k: usize| -> &str { if toks[k].0 == 0 { &region[toks[k].1..toks[k].2] } else { "" } };
    // (names mentioned outside `use` items, and the program's own traits)
    let mut used: crate::fxhash::FxSet<&str> = Default::default();
    let mut traits: crate::fxhash::FxSet<&str> = Default::default();
    let mut in_use = false;
    for k in 0..toks.len() {
        let w = word(k);
        if w == "use" {
            in_use = true;
        }
        if in_use {
            if toks[k].0 == 1 && b[toks[k].1] == b';' {
                in_use = false;
            }
            // (a path's first segment in a `use`: after `use`, `{` or `,`, and followed by `::`)
            let sym = |q: usize, c: u8| q < toks.len() && toks[q].0 == 1 && b[toks[q].1] == c;
            if toks[k].0 == 0 && w != "use" && k > 0 && (word(k - 1) == "use" || sym(k - 1, b'{') || sym(k - 1, b',')) && sym(k + 1, b':') && sym(k + 2, b':') {
                used.insert(w);
            }
            continue;
        }
        if k > 0 && word(k - 1) == "trait" {
            traits.insert(w);
        }
        if toks[k].0 == 0 {
            used.insert(w);
        }
    }
    for it in items {
        if !matches!(it.kind, crate::lexer::ItemKind::Normal) {
            continue;
        }
        let text = &src[it.start..it.end];
        // (past its outer attributes, `#[deprecated] #[cfg(not(FALSE))] use std::fs;`: a false `cfg` is stripped
        // before; the last of its lint attributes that names the lint or its group sets its level, and the last that
        // names `warnings` whether a warn is an error)
        let mut t = text.trim_start();
        let (mut level, mut warnings) = (None, None);
        if t.starts_with('#') {
            let tb = text.as_bytes();
            let tt = crate::lexer::tokenize(tb);
            let ts = |q: usize, c: u8| q < tt.len() && tt[q].0 == 1 && tb[tt[q].1] == c;
            let tw = |q: usize| if q < tt.len() && tt[q].0 == 0 { &text[tt[q].1..tt[q].2] } else { "" };
            let mut q = 0;
            while ts(q, b'#') && ts(q + 1, b'[') {
                // (`#[allow(a, unused_imports)]` / `#[warn(unused)]` / `#[deny(unused_imports)]`: the lint or its group;
                // `#[allow(warnings)]`)
                let l = tw(q + 2);
                if matches!(l, "allow" | "expect" | "warn" | "deny" | "forbid") && ts(q + 3, b'(') {
                    let mut r = q + 4;
                    while r < tt.len() && !ts(r, b')') {
                        match tw(r) {
                            "unused_imports" | "unused" => level = Some(l),
                            "warnings" => warnings = Some(l),
                            _ => {}
                        }
                        r += 1;
                    }
                }
                let (mut d, mut r) = (0i32, q + 1);
                while r < tt.len() {
                    if ts(r, b'[') || ts(r, b'(') {
                        d += 1;
                    } else if ts(r, b']') || ts(r, b')') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    r += 1;
                }
                if r >= tt.len() {
                    break;
                }
                t = text[tt[r].2..].trim_start();
                q = r + 1;
            }
        }
        // (a warn is an error only where `warnings` is denied: by the item's own level of it, or else the crate's)
        let warn_is_error = warnings.map_or(warn_denied, |w| matches!(w, "deny" | "forbid"));
        let lint_off = match level {
            Some("allow" | "expect") => true,
            Some("warn") => !warn_is_error,
            Some(_) => false,
            None => !lint_denied && !warn_is_error,
        };
        if !t.starts_with("use ") || lint_off || t.contains('*') {
            continue;
        }
        let body = t["use ".len()..].trim_end().trim_end_matches(';').trim();
        // (the names it brings in: `a::b::C`, `a::C as D`, `a::{B, C as D}` (one level of braces))
        let (prefix, list): (&str, Vec<&str>) = match body.find('{') {
            Some(i) if body.ends_with('}') => (&body[..i], body[i + 1..body.len() - 1].split(',').map(str::trim).filter(|x| !x.is_empty()).collect()),
            Some(_) => continue,
            None => ("", vec![body]),
        };
        let line = it.line + text[..text.len() - t.len()].matches('\n').count();
        for entry in &list {
            if entry.contains('{') || entry.contains('}') {
                break;
            }
            let (path, alias) = match entry.split_once(" as ") {
                Some((p, a)) => (p.trim(), Some(a.trim())),
                None => (entry.trim(), None),
            };
            let name = alias.unwrap_or_else(|| path.rsplit("::").next().unwrap_or(path));
            if name == "_" || name == "self" || name.is_empty() || TRAITS.contains(&name) || traits.contains(name) {
                continue;
            }
            if !used.contains(name) {
                let shown = if list.len() == 1 && prefix.is_empty() { (*entry).to_string() } else { (*entry).to_string() };
                return Some((line, 1, format!("unused import: `{shown}`")));
            }
        }
    }
    None
}

thread_local! {
    /// (first_unreachable: the program has a module named `process`, so `process::exit` may not be std's)
    static USER_PROCESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `#![deny(unreachable_code)]`: the first statement of a block after one that always diverges (`return`,
/// `break`, `continue`, `panic!` and its kin, a `loop` with no `break`), and whether it is a statement or the
/// block's value
fn first_unreachable(b: &crate::parse::Block, user_process: bool) -> Option<(crate::parse::Pos, &'static str)> {
    use crate::parse::{Expr, Stmt};
    USER_PROCESS.with(|c| c.set(user_process));
    fn diverges(e: &Expr) -> bool {
        match e {
            Expr::Return(..) | Expr::Break(..) | Expr::Continue(..) => true,
            Expr::Macro(p, _, _) => matches!(p.segs.last().map(|s| s.name.as_str()), Some("panic" | "unreachable" | "todo" | "unimplemented")),
            Expr::Loop(_, body) => !format!("{body:?}").contains("Break("),
            Expr::Paren(x) | Expr::Unary(_, x, _) | Expr::Ref(_, x) | Expr::Cast(x, _) | Expr::Field(x, _, _) => diverges(x),
            // (an operand or an argument that diverges: evaluated first, the rest never runs; not the right of `&&` /
            // `||`, which may not run)
            Expr::Binary(op, l, r, _) => diverges(l) || (!matches!(op.as_str(), "&&" | "||") && diverges(r)),
            Expr::Call(f, args) => diverges(f) || args.iter().any(diverges) || matches!(&**f, Expr::Path(None, p) if p.segs.len() >= 2 && p.segs[p.segs.len() - 2].name == "process" && matches!(p.segs[p.segs.len() - 1].name.as_str(), "exit" | "abort")
                && ((p.segs.len() >= 3 && p.segs[p.segs.len() - 3].name == "std") || !USER_PROCESS.with(|c| c.get()))),
            Expr::MethodCall(r, _, _, args, _) => diverges(r) || args.iter().any(diverges),
            // (both branches; a `match` with no arm (on an empty enum) or whose every arm diverges)
            Expr::If(c, t, Some(els)) => diverges(c) || (block_diverges(t) && diverges(els)),
            Expr::If(c, _, None) => diverges(c),
            Expr::Match(x, arms) => diverges(x) || arms.iter().all(|a| diverges(&a.body)),
            Expr::Block(None, false, b) => block_diverges(b),
            _ => false,
        }
    }
    fn block_diverges(b: &crate::parse::Block) -> bool {
        b.stmts.iter().any(|s| match s {
            Stmt::Expr(e, _) => diverges(e),
            Stmt::Let(_, _, Some(e), None, _) => diverges(e),
            _ => false,
        })
    }
    fn block(b: &crate::parse::Block) -> Option<(crate::parse::Pos, &'static str)> {
        let n = b.stmts.len();
        for (i, s) in b.stmts.iter().enumerate() {
            let e = match s {
                Stmt::Expr(e, _) => Some(e),
                Stmt::Let(_, _, Some(e), _, _) => Some(e),
                _ => None,
            };
            if let Some(e) = e {
                if let Some(r) = inner(e) {
                    return Some(r);
                }
            }
            let div = match s {
                Stmt::Expr(e, _) => diverges(e),
                Stmt::Let(_, _, Some(e), None, _) => diverges(e),
                _ => false,
            };
            if div {
                // (the next statement that is not an item)
                for (j, t) in b.stmts.iter().enumerate().skip(i + 1) {
                    match t {
                        Stmt::Item(_) => continue,
                        Stmt::Let(_, _, _, _, p) => return Some((*p, "statement")),
                        Stmt::Expr(e, semi) => {
                            let what = if !*semi && j + 1 == n { "expression" } else { "statement" };
                            return Some((acg::expr_start(e), what));
                        }
                    }
                }
                return None;
            }
        }
        None
    }
    fn inner(e: &Expr) -> Option<(crate::parse::Pos, &'static str)> {
        // (the arguments after one that diverges are unreachable expressions; when the last one diverges, the call)
        let args = |all: &[&Expr], call: crate::parse::Pos| -> Option<(crate::parse::Pos, &'static str)> {
            for (i, a) in all.iter().enumerate() {
                if let Some(r) = inner(a) {
                    return Some(r);
                }
                if diverges(a) {
                    return Some(match all.get(i + 1) {
                        Some(next) => (acg::expr_start(next), "expression"),
                        None => (call, "call"),
                    });
                }
            }
            None
        };
        match e {
            Expr::Block(_, _, b) | Expr::Loop(_, b) => block(b),
            Expr::If(c, t, els) => inner(c).or_else(|| block(t)).or_else(|| els.as_deref().and_then(inner)),
            Expr::While(_, c, b) => inner(c).or_else(|| block(b)),
            Expr::For(_, _, x, b) => inner(x).or_else(|| block(b)),
            Expr::Call(f, xs) => {
                let all: Vec<&Expr> = std::iter::once(&**f).chain(xs.iter()).collect();
                args(&all, acg::expr_start(e))
            }
            Expr::MethodCall(r, _, _, xs, sp) => {
                let all: Vec<&Expr> = std::iter::once(&**r).chain(xs.iter()).collect();
                args(&all, sp.start)
            }
            // (an operand that diverges: the right is never evaluated, or (the right diverging) the operation)
            Expr::Binary(op, l, r, sp) => {
                if let Some(x) = inner(l) {
                    return Some(x);
                }
                if diverges(l) {
                    return Some((acg::expr_start(r), "expression"));
                }
                if let Some(x) = inner(r) {
                    return Some(x);
                }
                (!matches!(op.as_str(), "&&" | "||") && diverges(r)).then_some((sp.start, "expression"))
            }
            Expr::Paren(x) | Expr::Unary(_, x, _) | Expr::Ref(_, x) | Expr::Cast(x, _) | Expr::Field(x, _, _) => inner(x),
            Expr::Match(x, arms) => inner(x).or_else(|| arms.iter().find_map(|a| inner(&a.body))),
            _ => None,
        }
    }
    block(b)
}

/// "malformed `X` attribute input" (E0539 / E0565): a built-in attribute of a fixed shape written in another one.
/// Read are the file's inner attributes (its head) and the attributes in front of the items (no full-text scan)
/// E0454 / E0459 / E0648: the error of a link's or an export's name (`#[link(name = "")]`, `#[link_name = "a\0"]`,
/// `#[export_name = ""]`), of an attribute's name and input
#[cold]
fn attr_name_msg(name: &str, input: &str) -> Option<String> {
    match (name, input.strip_prefix('='), input.strip_prefix('(').and_then(|r| r.strip_suffix(')'))) {
        ("link", _, Some(list)) if link_kinds_ok(list) => link_name_msg(list),
        ("derive", _, Some(_)) => Some("expected path to a trait, found literal (E0777)".to_string()),
        ("link_name" | "export_name", Some(v), _) => match str_value(v.trim()) {
            Some(v) if v.is_empty() && name == "link_name" => Some("link name must not be empty (E0454)".to_string()),
            Some(v) if v.is_empty() => Some("`export_name` may not be empty".to_string()),
            Some(v) if v.contains('\0') && name == "link_name" => Some("link name may not contain null characters (E0648)".to_string()),
            Some(v) if v.contains('\0') => Some("`export_name` may not contain null characters (E0648)".to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// Whether each `kind` of a `#[link(..)]` (its list's text) is one rustc knows
fn link_kinds_ok(list: &str) -> bool {
    attr_entries(list).iter().all(|e| match e.split_once('=') {
        Some((k, x)) if k.trim() == "kind" => str_value(x).is_some_and(|v| matches!(v.as_str(), "static" | "dylib" | "framework" | "raw-dylib" | "link-arg")),
        _ => true,
    })
}

/// The error of a `#[link(..)]`'s name (its list's text): none (E0459, unless it names a wasm import module), empty
/// (E0454), or with a NUL (E0648)
fn link_name_msg(list: &str) -> Option<String> {
    let entries = attr_entries(list);
    let name = entries.iter().find_map(|e| e.split_once('=').filter(|(k, _)| k.trim() == "name").map(|(_, v)| v.trim()));
    match name.map(str_value) {
        None if entries.iter().any(|e| e.split_once('=').is_some_and(|(k, _)| k.trim() == "wasm_import_module")) => None,
        None => Some("`#[link]` attribute requires a `name = \"string\"` argument (E0459)".to_string()),
        Some(Some(v)) if v.is_empty() => Some("link name must not be empty (E0454)".to_string()),
        Some(Some(v)) if v.contains('\0') => Some("link name may not contain null characters (E0648)".to_string()),
        _ => None,
    }
}

fn malformed_attr(src0: &str, split: &str, items: &[SrcItem], standalone: bool, edition: u16) -> Option<String> {
    // (a crate with no dependency (so no proc macro) knows only the built-in attributes and its tools' paths)
    // (both looked up only when such an attribute is there: not a search of every file)
    let unknown = |name: &str| {
        const BUILTIN: &[&str] = &[
            "cfg", "cfg_attr", "test", "ignore", "should_panic", "bench", "derive", "automatically_derived", "macro_export", "macro_use",
            "proc_macro", "proc_macro_derive", "proc_macro_attribute", "allow", "warn", "deny", "forbid", "expect", "deprecated", "must_use",
            "link", "link_name", "link_ordinal", "no_link", "repr", "export_name", "link_section", "no_mangle", "used", "crate_name",
            "crate_type", "no_builtins", "no_main", "no_std", "no_implicit_prelude", "recursion_limit", "type_length_limit", "windows_subsystem",
            "inline", "cold", "no_builtins", "target_feature", "track_caller", "instruction_set", "doc", "path", "non_exhaustive", "panic_handler",
            "global_allocator", "default_lib_allocator", "feature", "unsafe", "naked", "debugger_visualizer", "collapse_debuginfo", "coverage",
            "optimize", "rustfmt", "clippy", "unstable", "stable", "rustc_const_stable", "rustc_const_unstable", "register_tool", "macro_escape",
            "may_dangle", "thread_local", "ffi_pure", "ffi_const", "linkage", "omit_gdb_pretty_printer_section", "patchable_function_entry",
            "fundamental", "marker", "lang", "start", "custom_mir", "const_trait", "do_not_recommend", "on_unimplemented", "pointee",
            "type_const", "define_opaque", "align", "loop_match", "const_continue", "sanitize", "no_sanitize", "export_stable", "rustc_align",
            "async_trait", "allow_internal_unstable", "allow_internal_unsafe", "compiler_builtins", "needs_allocator", "panic_runtime",
            "needs_panic_runtime", "profiler_runtime", "no_core", "prelude_import", "rustc_std_internal_symbol", "unstable_feature_bound",
            "rustc_main", "test_runner", "reexport_test_harness_main", "default", "main", "plugin", "plugin_registrar",
            "zust_test_entry",
        ];
        // (`rustc_*`: rustc's internal attributes; without `#![feature(rustc_attrs)]` the gate's E0658, at the same
        // attribute)
        if name.contains("::") || name.is_empty() || name.starts_with("rustc_") {
            return false;
        }
        // (`#[default]` is the derive(Default) helper, `#[main]` was removed)
        if name == "main" || name == "default" && !src0.contains("Default") {
            return true;
        }
        !BUILTIN.contains(&name)
    };
    // (an attribute's name and its input: None = a word, Some('(') = a list, Some('=') = `= value`)
    fn check(name: &str, input: &str) -> bool {
        // (comments are whitespace: `#![windows_subsystem = /* why */ "console"]`)
        let input = crate::cfg::blank_comments(input);
        let input = input.trim();
        let (form, inner) = if input.is_empty() {
            (' ', "")
        } else if let Some(r) = input.strip_prefix('(') {
            ('(', r.strip_suffix(')').unwrap_or(r).trim())
        } else if let Some(r) = input.strip_prefix('=') {
            ('=', r.trim())
        } else {
            return true;
        };
        let idents = |v: &str| v.split(',').all(|x| {
            let x = x.trim();
            !x.is_empty() && x.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        // (`key = "value"` entries of known keys, each at most once: `deprecated(since = "1.0", note = "..")`)
        let keyed = |v: &str, keys: &[&str], value: &dyn Fn(&str, &str) -> bool| {
            let mut seen: Vec<&str> = vec![];
            attr_entries(v).iter().all(|e| match e.split_once('=') {
                Some((k, x)) => {
                    let k = k.trim();
                    let ok = keys.contains(&k) && !seen.contains(&k) && value(k, x.trim());
                    seen.push(k);
                    ok
                }
                None => false,
            })
        };
        match name {
            "crate_type" | "recursion_limit" | "type_length_limit" | "crate_name" => form == '=' && str_lit(inner),
            // (`link_name = "x"`, `export_name = "x"`, `link_section = ".x"`; `link_ordinal(N)`, an integer)
            // (E0454 / E0648: a link name or export name that is empty or has a NUL)
            "link_name" | "export_name" | "link_section" => form == '=' && str_lit(inner) && (name == "link_section" || str_value(inner).is_none_or(|v| !v.is_empty() && !v.contains('\0'))),
            "link_ordinal" => form == '(' && inner.trim().starts_with(|c: char| c.is_ascii_digit()) && inner.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "inline" => form == ' ' || (form == '(' && matches!(inner, "always" | "never")),
            "must_use" => form == ' ' || (form == '=' && str_lit(inner)),
            "cold" | "track_caller" | "non_exhaustive" | "no_mangle" | "automatically_derived" | "no_implicit_prelude" => form == ' ',
            // (each hint a name, or a name with its list: `repr(C, align(8))`, not `repr("C")`)
            "repr" => form == '(' && !inner.is_empty() && !inner.contains('=') && attr_entries(inner).iter().all(|e| e.starts_with(|c: char| c.is_alphabetic() || c == '_')),
            "deprecated" => form == ' ' || (form == '=' && str_lit(inner)) || (form == '(' && keyed(inner, &["since", "note", "suggestion"], &|_, x| str_lit(x))),
            // (the value's text, not its spelling: `r"console"` is `"console"`)
            "windows_subsystem" => form == '=' && str_value(inner).is_some_and(|v| matches!(v.as_str(), "console" | "windows")),
            // (`link(name = "m", kind = "static")`: a kind rustc knows)
            "link" => form == '(' && !inner.is_empty() && !inner.contains("cfg()") && link_kinds_ok(inner) && link_name_msg(inner).is_none(),
            "allow" | "warn" | "deny" | "forbid" | "expect" => form == '(',
            // (E0777: `derive("Clone")`, a literal for a trait)
            "derive" => form != '(' || !attr_entries(inner).iter().any(|e| e.trim_start().starts_with(['"', '\'']) || e.trim_start().starts_with(|c: char| c.is_ascii_digit())),
            "macro_use" => form == ' ' || (form == '(' && idents(inner)),
            "cfg" => form == '(' && attr_entries(inner).len() == 1 && cfg_pred(inner),
            "cfg_attr" => form == '(' && inner.contains(',') && attr_entries(inner).first().is_some_and(|p| cfg_pred(p)),
            _ => true,
        }
    }
    // (the attributes at the start of a text: `#[name input]` / `#![name input]`, with their offsets)
    fn leading(t: &str) -> Vec<(usize, &str, &str)> {
        let mut out = vec![];
        let mut i = 0;
        loop {
            let rest = &t[i..];
            let ws = rest.len() - rest.trim_start().len();
            let r = &rest[ws..];
            // (a comment between attributes: `#[inline]\n//~^ ERROR ..\n#[target_feature(..)]`)
            if r.starts_with("//") {
                i += ws + r.find('\n').map_or(r.len(), |n| n + 1);
                continue;
            }
            if r.starts_with("/*") {
                let Some(n) = r.find("*/") else { break };
                i += ws + n + 2;
                continue;
            }
            let open = if r.starts_with("#![") { 3 } else if r.starts_with("#[") { 2 } else { break };
            // (the matching `]`, past nested brackets and string literals)
            let mut depth = 0i32;
            let mut in_str = false;
            let mut end = None;
            for (k, c) in r.char_indices().skip(open) {
                match c {
                    '"' => in_str = !in_str,
                    '[' | '(' if !in_str => depth += 1,
                    ')' if !in_str => depth -= 1,
                    ']' if !in_str && depth == 0 => {
                        end = Some(k);
                        break;
                    }
                    ']' if !in_str => depth -= 1,
                    _ => {}
                }
            }
            let Some(e) = end else { break };
            // (the name past whitespace and comments: `#[ inline]`)
            let body = crate::cfg::trim_trivia(&r[open..e]);
            let n_end = body.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(body.len());
            out.push((i + ws, &body[..n_end], &body[n_end..]));
            i += ws + e + 1;
        }
        out
    }
    // (a `deprecated` value that is no literal (`note = test`, `note = concat!(..)`): rustc's parse message; any other
    // wrong shape: malformed input)
    let report = |text: &str, base_line: usize, at: usize, name: &str, input: &str| {
        let line = base_line + text[..at].matches('\n').count();
        let input = input.trim();
        let found = (name == "deprecated").then(|| match input.strip_prefix('=') {
            Some(v) => crate::cfg::attr_value(v).err(),
            None => {
                let list = input.strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or("");
                attr_entries(list).iter().find_map(|e| e.split_once('=').and_then(|(_, v)| crate::cfg::attr_value(v).err()))
            }
        });
        // (E0454 / E0459 / E0648: a link's or an export's name)
        if let Some(m) = attr_name_msg(name, input) {
            return format!("{line}:1: {m}");
        }
        match found.flatten() {
            Some(f) => format!("{line}:1: {}", crate::cfg::not_a_literal(&f)),
            // (`#[inline = ".."]`: the deny-by-default `ill_formed_attribute_input`, with the forms it takes)
            None if name == "inline" && input.starts_with('=') => format!("{line}:1: valid forms for the attribute are `inline`, `inline(always)`, and `inline(never)`"),
            None => format!("{line}:1: malformed `{name}` attribute input"),
        }
    };
    // (the file's inner attributes)
    let mut head_end = src0.len().min(8192);
    while !src0.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let head = &src0[..head_end];
    // (the file's inner attributes, in order, with their offsets: past whitespace, comments and a shebang, each to its
    // matching `]` across lines (`#![crate_type =\n "proc-macro"]`), up to the first item; not one in a comment)
    let inner_at: Vec<(usize, &str, &str)> = {
        let (hb, mut i, mut out) = (head.as_bytes(), 0, vec![]);
        if head.starts_with("#!") && !head.starts_with("#![") {
            i = head.find('\n').unwrap_or(head.len());
        }
        loop {
            while i < hb.len() && hb[i].is_ascii_whitespace() {
                i += 1;
            }
            if hb[i..].starts_with(b"//") || hb[i..].starts_with(b"/*") {
                i = lit_end(hb, i).unwrap_or(hb.len());
                continue;
            }
            if !hb[i..].starts_with(b"#![") {
                break;
            }
            let (mut d, mut k, mut end) = (0i32, i + 2, None);
            while k < hb.len() {
                if let Some(e) = lit_end(hb, k) {
                    k = e;
                    continue;
                }
                match hb[k] {
                    b'[' | b'(' | b'{' => d += 1,
                    b']' | b')' | b'}' => {
                        d -= 1;
                        if d == 0 {
                            end = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            let Some(e) = end else { break };
            // (the name past whitespace and comments: `#![ crate_type = "proc-macro"]`)
            let body = crate::cfg::trim_trivia(&head[i + 3..e]);
            let n_end = body.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(body.len());
            out.push((i, &body[..n_end], &body[n_end..]));
            i = e + 1;
        }
        out
    };
    for &(at, name, input) in &inner_at {
        // (the attribute macros an item takes, not a crate: `#![derive(Debug)]`)
        if matches!(name, "derive" | "test" | "test_case" | "bench" | "global_allocator") {
            let n = head[..at].matches('\n').count() + 1;
            return Some(format!("{n}:1: `{name}` attribute cannot be used at crate level"));
        }
        if !check(name, input) {
            return Some(report(head, 1, at, name, input));
        }
        // (a limit's value is read as a usize: `#![recursion_limit = "-100"]`, `""` and one too large are errors)
        if matches!(name, "recursion_limit" | "type_length_limit" | "move_size_limit" | "pattern_complexity_limit") {
            let v = input.trim_start().strip_prefix('=').map(str::trim).and_then(|v| v.strip_prefix('"')?.strip_suffix('"'));
            if v.is_some_and(|v| v.parse::<usize>().is_err()) {
                let n = head[..at].matches('\n').count() + 1;
                return Some(format!("{n}:1: `limit` must be a non-negative integer"));
            }
        }
        // (an inner attribute rustc does not know, `#![some_nonexistent_attribute]`; the crate-level ones of rustc's
        // that the list of item attributes leaves out are known)
        if standalone && unknown(name) && !matches!(name, "move_size_limit" | "pattern_complexity_limit" | "crate_id" | "no_start") {
            let n = head[..at].matches('\n').count() + 1;
            return Some(format!("{n}:1: cannot find attribute `{name}` in this scope"));
        }
    }
    // (the unsafe attributes, which `#[unsafe(..)]` may wrap; `naked` needs it in every edition, the others in 2024, or
    // where `unsafe_attr_outside_unsafe` is denied)
    const UNSAFE: &[&str] = &["no_mangle", "export_name", "link_section", "naked", "ffi_pure", "ffi_const", "force_target_feature"];
    let inner: Vec<(&str, &str)> = inner_at.iter().map(|&(_, n, inp)| (n, inp)).collect();
    // (`#![crate_type = "proc-macro"]`)
    let proc_macro_crate = inner.iter().any(|&(n, inp)| n == "crate_type" && crate::cfg::blank_comments(inp).trim().strip_prefix('=').and_then(str_value).is_some_and(|v| v == "proc-macro"));
    // (a lint's level at the crate: the last of its inner attributes that names it, or a group of it)
    let crate_level = |names: &[&str]| -> Option<&str> {
        let mut level = None;
        for &(n, inp) in &inner {
            if matches!(n, "deny" | "forbid" | "warn" | "allow" | "expect") {
                let list = inp.trim().strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or("");
                if attr_entries(list).iter().any(|e| names.contains(&e.trim())) {
                    level = Some(n);
                }
            }
        }
        level
    };
    // (before 2024 a lint: denied at the crate, and not allowed again on an item, `#[allow(unsafe_attr_outside_unsafe)]
    // mod generated { .. }`)
    // (any `allow(..)` / `expect(..)` list that names it, among other lints or not: `allow(dead_code,
    // unsafe_attr_outside_unsafe)`)
    let lowered = || {
        ["allow(", "expect("].iter().any(|l| {
            src0.match_indices(l).any(|(i, _)| {
                let list = &src0[i + l.len()..];
                let list = &list[..list.find(')').unwrap_or(list.len())];
                attr_entries(list).iter().any(|e| matches!(*e, "unsafe_attr_outside_unsafe" | "rust_2024_compatibility"))
            })
        })
    };
    let outside_unsafe_denied = edition >= 2024 || matches!(crate_level(&["unsafe_attr_outside_unsafe", "rust_2024_compatibility"]), Some("deny" | "forbid")) && !lowered();
    // (`unused_attributes` denied at the crate, by itself or a group)
    let unused_attributes_denied = matches!(crate_level(&["unused_attributes", "unused", "warnings"]), Some("deny" | "forbid"));
    // (a `macro_rules!`'s attributes, read from the file as written: expansion takes the definition out of the items.
    // Those `unused_attributes` reports there, where the crate denies it)
    if unused_attributes_denied && src0.contains("macro_rules!") {
        let lines: Vec<&str> = src0.split('\n').collect();
        for (k, l) in lines.iter().enumerate() {
            if !l.trim_start().starts_with("macro_rules!") {
                continue;
            }
            // (the attribute and comment lines right above it)
            let mut a = k;
            while a > 0 && { let p = lines[a - 1].trim_start(); p.starts_with("#[") || p.starts_with("//") } {
                a -= 1;
            }
            let block = lines[a..k].join("\n");
            for (at, name, _) in leading(&block) {
                if matches!(name, "macro_use" | "path" | "cold" | "no_mangle") {
                    let line = a + 1 + block[..at].matches('\n').count();
                    return Some(format!("{line}:1: the `{name}` attribute cannot be used on macro defs"));
                }
            }
        }
    }
    // (whether the source has an attribute at all: the scans of attributes inside items and anywhere are not run
    // otherwise)
    let any_attr = has_byte(src0.as_bytes(), b'#');
    for it in items {
        let text = &split[it.start..it.end];
        // (an attribute inside the item: a variant's, a parameter's; reported after the item's own)
        // (read where it can be: an enum's body, by its first words, and a fn's signature with a `#` in it, not the fn's
        // body)
        let nested = any_attr.then(|| {
            let word = |k: usize| it.idents.get(k).map_or("", |&(a, e)| split.get(a as usize..e as usize).unwrap_or(""));
            // (an extern block's items' attributes, `extern "C" { #[link_ordinal(1)] fn f(); }`: those at each item's start)
            if (0..6).any(|k| word(k) == "extern") && text.contains("#[") {
                if let Some(b) = text.find('{') {
                    let mut k = b + 1;
                    loop {
                        if let Some(&(at, name, _)) = leading(&text[k..]).iter().find(|&&(_, n, inp)| matches!(n, "link_name" | "link_ordinal") && !check(n, inp)) {
                            let inp = leading(&text[k..]).iter().find(|&&(a, _, _)| a == at).map_or("", |x| x.2);
                            return Some((k + at, attr_name_msg(name, inp.trim()).unwrap_or_else(|| format!("malformed `{name}` attribute input"))));
                        }
                        if let Some(&(at, _, _)) = leading(&text[k..]).iter().find(|&&(_, n, _)| n == "derive") {
                            return Some((k + at, "`derive` may only be applied to `struct`s, `enum`s and `union`s (E0774)".to_string()));
                        }
                        match text[k..].find(';') {
                            Some(e) => k += e + 1,
                            None => break,
                        }
                    }
                }
            }
            if (0..3).any(|k| word(k) == "enum") {
                return text.contains("#[").then(|| nested_attr_misuse(text, &unknown, unused_attributes_denied)).flatten();
            }
            let (mut sig_end, mut hash) = (text.len(), false);
            for (i, &c) in text.as_bytes().iter().enumerate() {
                if c == b'{' {
                    sig_end = i;
                    break;
                }
                hash |= c == b'#';
            }
            hash.then(|| nested_attr_misuse(&text[..sig_end], &unknown, unused_attributes_denied)).flatten()
        });
        let nested = nested.flatten();
        let nested = nested.map(|(at, msg)| format!("{}:1: {msg}", it.line + text[..at].matches('\n').count()));
        if !text.trim_start().starts_with("#[") {
            if nested.is_some() {
                return nested;
            }
            continue;
        }
        let attrs = leading(text);
        for &(at, name, input) in &attrs {
            let line = || it.line + text[..at].matches('\n').count();
            // (`#[test]` on what is no free fn, `--test` or not, at the attribute: rustc's test macro; a macro call's is
            // a warning)
            if name == "test" && input.trim().is_empty() && !test_target(text, it.kind) {
                let col = match text[..at].rfind('\n') {
                    Some(p) => at - p,
                    None => it.start - split[..it.start].rfind('\n').map_or(0, |p| p + 1) + at + 1,
                };
                return Some(format!("{}:{col}: the `test` attribute may only be used on a free function", line()));
            }
            // (`#[unsafe(name ..)]`: the attribute inside it, which must be an unsafe one)
            let (name, input) = match input.trim().strip_prefix('(').filter(|_| name == "unsafe") {
                Some(r) => {
                    // (the name past whitespace and comments: `#[unsafe(/* safety */ no_mangle)]`)
                    let inner = crate::cfg::trim_trivia(r.strip_suffix(')').unwrap_or(r));
                    let n_end = inner.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(inner.len());
                    let (n, rest) = (&inner[..n_end], &inner[n_end..]);
                    if n == "unsafe" {
                        return Some(format!("{}:1: expected identifier, found keyword `unsafe`", line()));
                    }
                    // (a name the crate does not know: that first, as rustc)
                    if unknown(n) && !standalone {
                        continue;
                    }
                    if standalone && unknown(n) && !text.contains("<<<<<<<") {
                        return Some(format!("{}:1: cannot find attribute `{n}` in this scope", line()));
                    }
                    if !UNSAFE.contains(&n) {
                        return Some(format!("{}:1: `{n}` is not an unsafe attribute", line()));
                    }
                    (n, rest)
                }
                None => {
                    if (name == "naked" || outside_unsafe_denied && matches!(name, "no_mangle" | "export_name" | "link_section")) && !text[at..].starts_with("#![") {
                        return Some(format!("{}:1: unsafe attribute used without unsafe", line()));
                    }
                    // (`#[derive(unsafe(Debug))]`: a derive's entries are paths)
                    if name == "derive" && input.contains("unsafe") {
                        let list = input.trim().strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or("");
                        let keyword = |x: &&str| x.trim_start().strip_prefix("unsafe").is_some_and(|r| !r.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
                        if attr_entries(list).iter().any(keyword) {
                            return Some(format!("{}:1: expected identifier, found keyword `unsafe`", line()));
                        }
                    }
                    (name, input)
                }
            };
            if !check(name, input) {
                return Some(report(text, it.line, at, name, input));
            }
            // (a parse error, as a merge's diff marker, comes first in rustc)
            if standalone && unknown(name) && !text.contains("<<<<<<<") {
                let line = it.line + text[..at].matches('\n').count();
                return Some(format!("{line}:1: cannot find attribute `{name}` in this scope"));
            }
        }
        // (an attribute on an item of a kind it does not apply to: `#[inline] struct S;`)
        let after = attrs.last().map(|&(at, _, _)| {
            let r = &text[at..];
            let e = r.find(']').map(|k| at + k + 1).unwrap_or(text.len());
            // (past comment lines between the attributes and the item)
            let mut r = text[e..].trim_start();
            while r.starts_with("//") {
                r = r[r.find('\n').unwrap_or(r.len())..].trim_start();
            }
            r
        });
        let Some(after) = after else {
            if nested.is_some() {
                return nested;
            }
            continue;
        };
        let mut ws = after.split_whitespace().filter(|w| !matches!(*w, "pub" | "unsafe" | "async" | "const" | "extern" | "default") && !w.starts_with("pub(") && !w.starts_with('"'));
        let Some(kw) = ws.next() else { continue };
        let kw = if after.starts_with("const ") && !after.starts_with("const fn") { "const" } else { kw };
        let kind = match kw {
            "fn" => "functions",
            "struct" => "structs",
            "enum" => "enums",
            "union" => "unions",
            "trait" => "traits",
            "impl" => "inherent impl blocks",
            "mod" => "modules",
            "static" => "statics",
            "const" => "constants",
            "type" => "type aliases",
            "use" => "use statements",
            "macro_rules!" => "macro defs",
            _ => {
                if nested.is_some() {
                    return nested;
                }
                continue;
            }
        };
        for &(at, name, input) in &attrs {
            // (E0774: a derive is a struct's, an enum's or a union's)
            if name == "derive" && !matches!(kw, "struct" | "enum" | "union") {
                let line = it.line + text[..at].matches('\n').count();
                return Some(format!("{line}:1: `derive` may only be applied to `struct`s, `enum`s and `union`s (E0774)"));
            }
            // (a proc macro: a fn of a `proc-macro` crate)
            if matches!(name, "proc_macro" | "proc_macro_derive" | "proc_macro_attribute") && !proc_macro_crate {
                let line = it.line + text[..at].matches('\n').count();
                return Some(if kw == "fn" {
                    format!("{line}:1: the `{name}` attribute is only usable with crates of the `proc-macro` crate type")
                } else {
                    format!("{line}:1: the `{name}` attribute cannot be used on {kind}")
                });
            }
            let line = || it.line + text[..at].matches('\n').count();
            // (`repr(packed)` is a struct's or a union's; rustc names a repr by its hint)
            let hint = input.trim().strip_prefix('(').and_then(|r| r.strip_suffix(')')).map(str::trim).unwrap_or("");
            // (an integer repr, `repr(u8)`, also one of a list, `repr(C, u8)`: an enum's only)
            let int_hint = (name == "repr").then(|| attr_entries(hint).into_iter().find(|h| int_repr(h))).flatten();
            let shown = match int_hint {
                Some(h) => format!("repr({h})"),
                None if name == "repr" && !hint.is_empty() && !hint.contains(',') => format!("repr({hint})"),
                None => name.to_string(),
            };
            // (an error wherever it is: the attributes rustc rejects on an item of another kind)
            let ok = match name {
                "inline" | "track_caller" | "target_feature" => kw == "fn",
                // (a foreign fn's or static's only: no item here is one)
                "link_ordinal" => false,
                "repr" if int_hint.is_some() => kw == "enum",
                "repr" if hint.starts_with("packed") => matches!(kw, "struct" | "union"),
                "repr" => matches!(kw, "struct" | "enum" | "union"),
                "non_exhaustive" => matches!(kw, "struct" | "enum"),
                "used" => kw == "static",
                "debugger_visualizer" => kw == "mod",
                _ => true,
            };
            if !ok {
                return Some(format!("{}:1: the `{shown}` attribute cannot be used on {kind}", line()));
            }
            // (`#[inline]` on a trait's fn without a body: `unused_attributes`, an error where the crate denies it)
            if name == "inline" && kw == "fn" && matches!(it.kind, ItemKind::TraitMember(_)) && text.trim_end().ends_with(';') && unused_attributes_denied {
                return Some(format!("{}:1: the `inline` attribute cannot be used on required trait methods", line()));
            }
            // (`no_mangle` on a const: the deny-by-default `no_mangle_const_items`)
            if name == "no_mangle" && kw == "const" {
                return Some(format!("{}:1: const items should never be `#[no_mangle]`", line()));
            }
            // (only a warning (`unused_attributes`) elsewhere: an error where the crate denies it)
            let warned = match name {
                "cold" => kw != "fn",
                "macro_use" => kw != "mod",
                "no_mangle" => !matches!(kw, "fn" | "static"),
                _ => false,
            };
            if warned && unused_attributes_denied {
                return Some(format!("{}:1: the `{name}` attribute cannot be used on {kind}", line()));
            }
        }
        if nested.is_some() {
            return nested;
        }
    }
    if standalone && any_attr { unknown_attr_anywhere(src0, &unknown) } else { None }
}

/// Whether a byte is in a text: 32 bytes at a time, as four words whose bytes are compared at once (a zero byte of
/// `w ^ x..x`); for a text without the byte, a pass at a fraction of `memchr`'s cost per byte
fn has_byte(b: &[u8], x: u8) -> bool {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let pat = LO * x as u64;
    let zero = |w: u64| {
        let v = w ^ pat;
        v.wrapping_sub(LO) & !v & HI
    };
    let mut chunks = b.chunks_exact(32);
    for c in &mut chunks {
        let w = |i: usize| u64::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3], c[i + 4], c[i + 5], c[i + 6], c[i + 7]]);
        if zero(w(0)) | zero(w(8)) | zero(w(16)) | zero(w(24)) != 0 {
            return true;
        }
    }
    chunks.remainder().contains(&x)
}

/// An integer type a `repr` may name: an enum's
fn int_repr(h: &str) -> bool {
    matches!(h, "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize")
}

/// An attribute inside an item that rustc rejects where it is (at its offset in the text, with rustc's message): on an
/// enum's variant, one of a fn's (`the `inline` attribute cannot be used on enum variants`; one only `unused_attributes`
/// warns of, where the crate denies it), or a built-in attribute on a fn's parameter other than the lint attributes
/// and `cfg` / `cfg_attr`
fn nested_attr_misuse(text: &str, unknown: &dyn Fn(&str) -> bool, unused_denied: bool) -> Option<(usize, String)> {
    let b = text.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let n = toks.len();
    let t = |k: usize| -> &[u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let matching = |o: usize| -> Option<usize> {
        let mut d = 0i32;
        for k in o..n {
            match t(k) {
                b"(" | b"[" | b"{" => d += 1,
                b")" | b"]" | b"}" => {
                    d -= 1;
                    if d == 0 {
                        return Some(k);
                    }
                }
                _ => {}
            }
        }
        None
    };
    // (past the item's own attributes)
    let mut k = 0;
    while t(k) == b"#" && t(k + 1) == b"[" {
        k = matching(k + 1)? + 1;
    }
    // (the keyword: `enum` or `fn`, past visibility and qualifiers)
    let kw = loop {
        match t(k) {
            b"pub" if t(k + 1) == b"(" => k = matching(k + 1)? + 1,
            b"pub" | b"const" | b"async" | b"unsafe" | b"default" | b"safe" => k += 1,
            b"extern" if toks.get(k + 1).is_some_and(|x| x.0 == 2) => k += 2,
            b"extern" => k += 1,
            b"enum" | b"fn" if is_id(k + 1) => break t(k),
            _ => return None,
        }
    };
    // (the body of an enum, the parameters of a fn: past the name and the generics)
    let open = if kw == b"enum" { b"{".as_slice() } else { b"(".as_slice() };
    let mut j = k + 2;
    let mut angle = 0i32;
    while j < n {
        match t(j) {
            b"<" => angle += 1,
            b">" if t(j - 1) != b"-" => angle -= 1,
            x if angle == 0 && x == open => break,
            b"(" | b"[" | b"{" => j = matching(j)?,
            b";" => return None,
            _ => {}
        }
        j += 1;
    }
    let close = matching(j)?;
    // (the attributes at the group's own level)
    let mut k = j + 1;
    while k < close {
        if matches!(t(k), b"(" | b"[" | b"{") {
            k = matching(k)? + 1;
            continue;
        }
        if t(k) == b"#" && t(k + 1) == b"[" && is_id(k + 2) && t(k + 3) != b":" {
            let e = matching(k + 1)?;
            let name = String::from_utf8_lossy(t(k + 2)).into_owned();
            let at = toks[k].1;
            if kw == b"enum" {
                let hint = (name == "repr" && t(k + 3) == b"(").then(|| matching(k + 3)).flatten();
                let hint = hint.map(|c| (k + 4..c).map(|x| String::from_utf8_lossy(t(x)).into_owned()).collect::<String>());
                let shown = match &hint {
                    Some(h) => match attr_entries(h).into_iter().find(|h| int_repr(h)) {
                        Some(i) => format!("repr({i})"),
                        None => format!("repr({})", h.replace(',', ", ")),
                    },
                    None => name.clone(),
                };
                let error = matches!(name.as_str(), "inline" | "track_caller" | "target_feature" | "repr" | "used" | "link_ordinal" | "debugger_visualizer");
                let warned = matches!(name.as_str(), "cold" | "no_mangle" | "macro_use" | "export_name" | "link_section" | "must_use");
                if error || warned && unused_denied {
                    return Some((at, format!("the `{shown}` attribute cannot be used on enum variants")));
                }
            // (not a `rustc_*` one: its feature gate first, `#[rustc_splat]`)
            } else if !unknown(&name) && !name.starts_with("rustc_") && !matches!(name.as_str(), "allow" | "cfg" | "cfg_attr" | "deny" | "expect" | "forbid" | "warn") {
                return Some((at, "allow, cfg, cfg_attr, deny, expect, forbid, and warn are the only allowed built-in attributes in function parameters".to_string()));
            }
            k = e + 1;
            continue;
        }
        k += 1;
    }
    None
}

/// "cannot find attribute `x` in this scope" of an attribute anywhere in the code, not only on an item: on a field,
/// a variant, a parameter, a statement, a `macro_rules!`, a module. Not one in a comment or a literal, in a macro's
/// body or input (what the expansion reads), a name an import binds (`use derive as my_derive;`), nor any where the
/// crate has a derive not std's, whose helpers it could be. Read only where an attribute's name is unknown at all
fn unknown_attr_anywhere(src: &str, unknown: &dyn Fn(&str) -> bool) -> Option<String> {
    // (the candidates: `#[name` / `# [name` whose name is no built-in attribute)
    let cands: Vec<usize> = src
        .match_indices('#')
        .filter(|&(i, _)| {
            let Some(r) = crate::cfg::trim_trivia(&src[i + 1..]).strip_prefix('[') else { return false };
            let r = crate::cfg::trim_trivia(r);
            let n = &r[..r.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(r.len())];
            unknown(n)
        })
        .map(|(i, _)| i)
        .collect();
    // (a parse error comes first in rustc: a merge's diff marker)
    if cands.is_empty() || src.contains("<<<<<<<") {
        return None;
    }
    let b = src.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let n = toks.len();
    let t = |k: usize| -> &[u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let mut close = vec![usize::MAX; n];
    let mut stack: Vec<usize> = vec![];
    for k in 0..n {
        if toks[k].0 == 1 {
            match t(k) {
                b"(" | b"[" | b"{" => stack.push(k),
                b")" | b"]" | b"}" => {
                    if let Some(o) = stack.pop() {
                        close[o] = k;
                    }
                }
                _ => {}
            }
        }
    }
    // (the macros' bodies and inputs: `m!(..)`, `macro_rules! m { .. }`; the names imports bind; a derive not std's)
    let mut macros: Vec<(usize, usize)> = vec![];
    let mut bound: FxSet<&[u8]> = FxSet::default();
    for k in 0..n {
        if is_id(k) && t(k + 1) == b"!" && t(k.wrapping_sub(1)) != b"$" {
            let o = if is_id(k + 2) { k + 3 } else { k + 2 };
            if matches!(t(o), b"(" | b"[" | b"{") && close[o] != usize::MAX {
                macros.push((toks[o].1, toks[close[o]].2));
            }
        }
        if t(k) == b"use" {
            let mut j = k + 1;
            while j < n && t(j) != b";" {
                if is_id(j) {
                    bound.insert(t(j));
                }
                j += 1;
            }
        }
        if t(k) == b"derive" && t(k + 1) == b"(" && close[k + 1] != usize::MAX {
            let std = |d: &[u8]| matches!(d, b"Debug" | b"Clone" | b"Copy" | b"PartialEq" | b"Eq" | b"PartialOrd" | b"Ord" | b"Hash" | b"Default");
            // (the last segment of each path in the list: `std::fmt::Debug`)
            if (k + 2..close[k + 1]).any(|j| is_id(j) && !matches!(t(j + 1), b":") && !matches!(t(j), b"std" | b"core") && !std(t(j))) {
                return None;
            }
        }
    }
    for at in cands {
        // (a token there: not in a comment or a literal; not in a macro)
        let Ok(k) = toks.binary_search_by_key(&at, |x| x.1) else { continue };
        if t(k) != b"#" || t(k.wrapping_sub(1)) == b"$" || macros.iter().any(|&(a, e)| a < at && at < e) {
            continue;
        }
        let o = k + 1;
        if t(o) != b"[" || !is_id(o + 1) {
            continue;
        }
        let name = t(o + 1);
        // (one with nothing after it, `{ #[foo] }`, is rustc's parse error first)
        if t(o + 2) == b":" || bound.contains(name) || close[o] == usize::MAX || matches!(t(close[o] + 1), b"}" | b"") {
            continue;
        }
        let name = String::from_utf8_lossy(name);
        if unknown(&name) {
            let line = src[..at].matches('\n').count() + 1;
            return Some(format!("{line}:1: cannot find attribute `{name}` in this scope"));
        }
    }
    None
}

/// E0038: a fn of the program whose parameters or result name `dyn Tr` of a trait that is not dyn compatible (rustc
/// checks the types a signature writes, called or not): at the fn's line
/// (called only for a program whose own text has `dyn`)
fn dyn_in_signature(prog: &Program, src: &str, items: &[SrcItem]) -> Option<String> {
    fn dyns(prog: &Program, t: Ty, depth: u32, out: &mut Vec<&'static String>) {
        if depth > 8 {
            return;
        }
        match t {
            Ty::Dyn(d) => out.push(d),
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => dyns(prog, *i, depth + 1, out),
            Ty::Struct(sid) => {
                for &a in &prog.structs[sid as usize].args {
                    dyns(prog, a, depth + 1, out);
                }
            }
            // (a fn pointer's parameters and result: `fn(&dyn Bad)`)
            Ty::Fn(f) => {
                for &a in f.params.iter().chain([&f.ret]) {
                    dyns(prog, a, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut found = vec![];
    for (fi, sig) in prog.sigs.iter().enumerate() {
        let Some(ii) = prog.item_of.get(fi).copied().flatten().filter(|&ii| ii < prog.prelude_from) else { continue };
        found.clear();
        for &t in sig.params.iter().chain(std::iter::once(&sig.ret)) {
            dyns(prog, t, 0, &mut found);
        }
        for d in &found {
            // (the principal trait: `dyn Tr<u8> + Send`'s `Tr`)
            let tr = d.split('+').map(str::trim).find(|b| !matches!(*b, "Send" | "Sync" | "Unpin") && !b.starts_with('\'')).unwrap_or("");
            let tr = tr.split('<').next().unwrap_or("").trim();
            // (a trait the program knows; dyn_incompatible reads the methods of the program's own ones)
            if !prog.traits.contains_key(tr) || crate::ast_summary::parse_fn_bound(tr).is_some() {
                continue;
            }
            if let Some(m) = acg::dyn_incompatible(prog, tr, &prog.dyn_methods(tr), true) {
                // (the line of `fn`, past the item's attributes and doc comments)
                let it = &items[ii as usize];
                let past = src[it.start..it.end].lines().take_while(|l| { let l = l.trim_start(); l.starts_with('#') || l.starts_with("//") }).count();
                return Some(format!("{}:1: {m}", it.line + past));
            }
        }
    }
    None
}

/// E0428 for two of the program's items of one name and namespace that the build keeps in separate tables: a struct
/// / enum, a trait or a type alias (types), a unit / tuple struct and a const / static / fn (values). At the later
/// of the two. Costs a lookup for each of the program's structs, traits and aliases
/// A bare variant name the program's own enum took from a prelude enum's (Program::build): the prelude's again where
/// the program's imports bring that one into scope and not its own (`use std::ops::ControlFlow::*;` beside its own
/// `enum Local { Break(u8) }`), and neither where they bring both (rustc's E0659 at a use). Read from the text of the
/// program's `use` items: an import of `E::*`, `E::v` or `E::{.., v, ..}`. When the two enums have one name, the
/// prelude's is the one imported through `std::` / `core::` / `alloc::`
fn imported_prelude_variants(prog: &mut Program, src: &str, items: &[SrcItem]) {
    // (each `use` item's text without white space, and whether its path starts at std / core / alloc)
    let uses: Vec<(String, bool)> = items
        .iter()
        .filter_map(|it| {
            let t: String = src.get(it.start..it.end)?.chars().filter(|c| !c.is_whitespace()).collect();
            let t = t.strip_prefix("pub(crate)").or_else(|| t.strip_prefix("pub")).unwrap_or(&t).strip_prefix("use")?.to_string();
            let t = t.strip_prefix("::").unwrap_or(&t).to_string();
            let from_std = ["std::", "core::", "alloc::"].iter().any(|p| t.starts_with(p));
            Some((t, from_std))
        })
        .collect();
    if uses.is_empty() {
        return;
    }
    let imports = |e: &str, v: &str, std_side: Option<bool>| {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        uses.iter().filter(|(_, s)| std_side.is_none_or(|w| w == *s)).any(|(t, _)| {
            t.match_indices(&format!("{e}::")).any(|(i, m)| {
                if t[..i].chars().next_back().is_some_and(word) {
                    return false;
                }
                let rest = &t[i + m.len()..];
                let leaf = |x: &str| x == "*" || x == v;
                match rest.strip_prefix('{') {
                    Some(r) => r.split('}').next().unwrap_or("").split(',').any(leaf),
                    None => leaf(rest.split(|c: char| !word(c) && c != '*').next().unwrap_or("")),
                }
            })
        })
    };
    let shadowed = std::mem::take(&mut prog.variant_shadowed);
    for (v, pre) in &shadowed {
        let Some(Some(own)) = prog.variant_names.get(v).copied() else { continue };
        let pname = prog.structs[pre.0 as usize].name.as_str();
        let pname = pname.strip_prefix("__p_").unwrap_or(pname);
        let oname = prog.structs[own.0 as usize].name.as_str();
        let same = pname == oname;
        let pre_in = imports(pname, v, same.then_some(true));
        let own_in = imports(oname, v, same.then_some(false));
        match (pre_in, own_in) {
            (true, false) => {
                prog.variant_names.insert(v.clone(), Some(*pre));
            }
            (true, true) => {
                prog.variant_names.insert(v.clone(), None);
            }
            _ => {}
        }
    }
    prog.variant_shadowed = shadowed;
}

/// What Program::build_with_edition reads, as a fingerprint: the declarations (Analysis::iface_fp: each item's hash,
/// or its summary's for an fn, whose body the build does not read), the items' kinds, and which items are on the same
/// line (an item's line is read only for an error's place, and to tell impls of a trait on one line apart:
/// `unbounded_blankets`); and the build's options
fn prog_fp(iface_fp: u64, items: &[SrcItem], need_main: bool, user: usize, edition: u16, deny_bare: bool) -> Key {
    let mut h = KeyHasher::new();
    h.add(&iface_fp);
    h.add(&items.len());
    h.add(&need_main);
    h.add(&user);
    h.add(&edition);
    h.add(&deny_bare);
    let mut first: FxMap<usize, u32> = FxMap::default();
    for (i, it) in items.iter().enumerate() {
        let (k, of) = match it.kind {
            ItemKind::Normal => (0u8, 0),
            ItemKind::ImplHeader => (1, 0),
            ItemKind::ImplMember(o) => (2, o),
            ItemKind::TraitHeader => (3, 0),
            ItemKind::TraitMember(o) => (4, o),
            ItemKind::InnerAttr => (5, 0),
        };
        h.add(&k);
        h.add(&of);
        h.add(first.entry(it.line).or_insert(i as u32));
    }
    h.finish()
}

fn cross_kind_duplicate<'p>(prog: &'p Program, src: &str, items: &[SrcItem], sums: &'p [Summary], user: usize) -> Option<String> {
    let user = user.min(sums.len());
    let mut first: Option<(usize, &'p str)> = None;
    let mut found = |a: usize, b: usize, n: &'p str| {
        let l = items[a].line.max(items[b].line);
        if first.is_none_or(|(l0, _)| l < l0) {
            first = Some((l, n));
        }
    };
    for (ii, sm) in sums[..user].iter().enumerate() {
        if let Some(tr) = sm.trait_decl.as_deref() {
            if let Some(&sid) = prog.struct_index.get(tr) {
                if let Some(&si) = prog.struct_item.get(sid as usize).filter(|&&si| (si as usize) < user) {
                    found(si as usize, ii, tr);
                }
            }
        }
        // (a type alias and a struct / enum / trait)
        for (n, _, _) in &sm.aliases {
            if let Some(&si) = prog.struct_index.get(n).and_then(|&sid| prog.struct_item.get(sid as usize)).filter(|&&si| (si as usize) < user) {
                found(si as usize, ii, n);
            } else if prog.traits.contains_key(n) {
                if let Some(ti) = sums[..user].iter().position(|t| t.trait_decl.as_deref() == Some(n.as_str())) {
                    found(ti, ii, n);
                }
            }
        }
        for (n, fields) in &sm.structs {
            // (a struct with named fields is a type only: no lookup)
            if fields.first().is_some_and(|(f, _)| f != "0") {
                continue;
            }
            let item_of = |k: u32| if (k as usize) < user { Some(k as usize) } else { None };
            let other = prog
                .const_index
                .get(n)
                .and_then(|&c| item_of(prog.consts[c as usize].item))
                .or_else(|| prog.index.get(n).and_then(|&f| prog.item_of.get(f as usize).copied().flatten()).and_then(item_of));
            let Some(o) = other else { continue };
            if o == ii || sm.unions.iter().any(|u| u == n) {
                continue;
            }
            // (a unit / tuple struct is a value too: `struct X;`, `struct X(u8);`, not `struct X {}`)
            let value = match fields.first() {
                Some((f, _)) => f == "0",
                None => {
                    let text = src.get(items[ii].start..items[ii].end).unwrap_or("");
                    // (past the item's attributes and the comments among them, whose text may say `struct X;`)
                    let mut text = text.trim_start();
                    loop {
                        if text.starts_with("#[") || text.starts_with("#![") {
                            let o = text.find('[').map_or(text.len(), |o| o + 1);
                            text = text[(o + attr_end(&text[o..])).min(text.len())..].trim_start();
                        } else if text.starts_with("//") {
                            text = text.find('\n').map_or("", |e| text[e..].trim_start());
                        } else if text.starts_with("/*") {
                            text = text.find("*/").map_or("", |e| text[e + 2..].trim_start());
                        } else {
                            break;
                        }
                    }
                    text.find("struct ").is_some_and(|k| {
                        let rest = text[k + 7..].trim_start();
                        let rest = rest.strip_prefix(n.as_str()).unwrap_or(rest).trim_start();
                        let rest = if rest.starts_with('<') { rest.find('>').map_or("", |e| &rest[e + 1..]) } else { rest };
                        rest.trim_start().starts_with([';', '('])
                    })
                }
            };
            if value {
                found(ii, o, n);
            }
        }
    }
    // (the name as written: without the flattened modules' `a__` prefix and the renamer's `__u_`)
    first.map(|(l, n)| format!("{l}:1: the name `{}` is defined multiple times (E0428)", n.rsplit("__").next().unwrap_or(n).trim_start_matches("u_")))
}

/// What defines a name in a module, for the code of two definitions of one name: E0428 two items, E0252 two imports,
/// E0255 an import and an item, E0259 two extern crates, E0260 an extern crate and an item, E0254 an extern crate and
/// an import
#[derive(Clone, Copy, PartialEq, Eq)]
enum Def {
    Item,
    Use,
    Crate,
    /// the `extern crate std;` (`core` under `#![no_std]`) the compiler puts in the crate root
    Injected,
}

/// The names the `extern crate` items the configuring blanked bind: `extern crate a as b;` binds a and b
fn extern_names(externs: &[(usize, String)]) -> impl Iterator<Item = &str> {
    externs.iter().flat_map(|(_, text)| {
        let w: Vec<&str> = text.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
        let from = w.iter().position(|&x| x == "crate").map_or(w.len(), |i| i + 1);
        w.into_iter().skip(from).filter(|&x| x != "as" && x != "r")
    })
}

/// A visibility as written: `pub` (or `pub(crate)`), none (or `pub(self)`), `pub(super)`, `pub(in path)`
#[derive(Clone)]
enum PVis<'a> {
    Pub,
    /// `pub(crate)`: as `pub` to a module of the crate; narrower for a re-export (E0364)
    Crate,
    Priv,
    Super,
    In(Vec<&'a [u8]>),
}

/// An item of a module, as `private_path` reads it
struct PItem<'a> {
    name: &'a [u8],
    /// what rustc calls it: "module", "struct", "unit struct", "tuple struct", "enum", ...
    kind: &'static str,
    vis: PVis<'a>,
    /// the module it is, for a `mod m { .. }`
    child: usize,
    /// the token of its name
    at: usize,
}

/// The paths of a `use` tree that starts at token `k`, each as the tokens of its segments (`use a::{b, c::d}`: [a, b],
/// [a, c, d]; a glob or a `self` gives the module's path, and `globs` which leaves those are: 1 a `self`, 2 a glob),
/// and the token past the tree
fn use_leaves(
    b: &[u8],
    toks: &[(u8, usize, usize)],
    close: &[usize],
    k: usize,
    prefix: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
    globs: &mut Vec<u8>,
) -> usize {
    let t = |k: usize| -> &[u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let depth = prefix.len();
    let mut k = k;
    let end = loop {
        if t(k) == b"{" {
            let Some(&e) = close.get(k).filter(|&&e| e != usize::MAX) else { break k };
            let mut j = k + 1;
            while j < e {
                j = use_leaves(b, toks, close, j, prefix, out, globs);
                if t(j) != b"," {
                    break;
                }
                j += 1;
            }
            break e + 1;
        }
        if t(k) == b"*" {
            out.push(prefix.clone());
            globs.push(2);
            break k + 1;
        }
        if !is_id(k) {
            break k;
        }
        let s = if t(k) == b"r" && t(k + 1) == b"#" && is_id(k + 2) { k + 2 } else { k };
        if t(s + 1) == b":" && t(s + 2) == b":" {
            prefix.push(s);
            k = s + 3;
            continue;
        }
        if t(s) == b"self" {
            out.push(prefix.clone());
            globs.push(1);
        } else {
            let mut p = prefix.clone();
            p.push(s);
            out.push(p);
            globs.push(0);
        }
        break if t(s + 1) == b"as" { s + 3 } else { s + 1 };
    };
    prefix.truncate(depth);
    end
}

/// E0603: a path through the program's own inline modules (`a::b::f()`, `use crate::a::f;`, `super::m::C`) to a
/// module or item that is not visible where the path is written: "function `f` is private", at that segment, the
/// first in the text. A path is read only as far as it names the program's own modules and items: one that starts at
/// another crate, an import, a module of a fn body or a name some import binds, or that reaches a name only an import
/// or a macro defines, is left alone. Not a path in an attribute or a macro's definition (resolved where it expands).
/// E0432 too: a `use` path through the program's modules to a name its module does not define ("unresolved import
/// `a::b`", the path to that segment), or on through an item that is no module (`use m::S::C`, `use m::f::{self}`,
/// `use m::A::*` of a type alias), where nothing else could bind the name there: no glob import, no `use` or
/// `extern crate` of it, no item-level macro call or definition, no `extern` block, no module of another file
/// `fpriv` gets the fields a module may not read, for the type check's E0616 (`s.x` of a struct of another module);
/// `late` an error to report only where the type check finds none (E0451)
fn private_path(
    src: &str,
    toks: &[(u8, usize, usize)],
    externs: &[(usize, String)],
    injected: bool,
    fpriv: &mut Option<Box<crate::program::FieldPriv>>,
    late: &mut Option<String>,
) -> Option<String> {
    let b = src.as_bytes();
    let n = toks.len();
    let t = |k: usize| -> &[u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let raw = |k: usize| if t(k) == b"r" && t(k + 1) == b"#" && is_id(k + 2) { k + 2 } else { k };
    let mut close = vec![usize::MAX; n];
    let mut stack: Vec<usize> = vec![];
    for k in 0..n {
        if toks[k].0 == 1 {
            match b[toks[k].1] {
                b'(' | b'[' | b'{' => stack.push(k),
                b')' | b']' | b'}' => {
                    if let Some(o) = stack.pop() {
                        close[o] = k;
                    }
                }
                _ => {}
            }
        }
    }
    // (modules: the tokens of the body, the parent; the crate root first)
    let mut mods: Vec<(usize, usize, usize)> = vec![(0, n, usize::MAX)];
    let mut items: Vec<Vec<PItem>> = vec![];
    // (in each module, the item-level ranges a path is not read in: a `macro_rules!` and a macro call)
    let mut skips: Vec<Vec<(usize, usize)>> = vec![];
    // (the names `use` binds anywhere, and the names of more than one `mod`: a path that starts there is left alone)
    let mut bound: FxSet<&[u8]> = FxSet::default();
    // (in each module, the names its imports and `extern crate`s bind (`as` names too), and whether something else
    // could define a name there: a glob import, an item-level macro, an `extern` block, a `macro`)
    let mut binds: Vec<FxSet<&[u8]>> = vec![];
    let mut open_ended: Vec<bool> = vec![];
    // (for E0659: each module's imports, whether a macro or an `extern` block may define names in it, and where a
    // module-level `use` item's imports start in its module's list: by the `use` token)
    let mut imps: Vec<Vec<PImport>> = vec![];
    let mut opaque: Vec<bool> = vec![];
    let mut imp_at: FxMap<usize, usize> = FxMap::default();
    // (a module of another file, `mod m;`, or a macro exported to the crate root: E0432 is not reported)
    let unknown_mods = (0..n.saturating_sub(2)).any(|k| t(k) == b"mod" && is_id(k + 1) && t(k + 2) == b";" && !matches!(t(k.wrapping_sub(1)), b"." | b"$"))
        || src.contains("macro_export");
    let mut mod_names: FxMap<&[u8], u32> = FxMap::default();
    for k in 0..n.saturating_sub(1) {
        if t(k) == b"mod" && is_id(k + 1) {
            *mod_names.entry(t(raw(k + 1))).or_insert(0) += 1;
        }
    }
    while items.len() < mods.len() {
        let m = items.len();
        let (lo, hi, _) = mods[m];
        let (mut its, mut sk): (Vec<PItem>, Vec<(usize, usize)>) = (vec![], vec![]);
        let (mut bn, mut ended): (FxSet<&[u8]>, bool) = (FxSet::default(), false);
        let (mut im, mut opq): (Vec<PImport>, bool) = (vec![], false);
        // (the crate root of 2015 has the injected `std` / `core`; the `extern crate` items the configuring blanked
        // bind their names, here taken as bound in every module)
        if m == 0 && injected {
            bn.extend([&b"std"[..], b"core"]);
        }
        bn.extend(extern_names(externs).map(str::as_bytes));
        let mut k = lo;
        let mut vis = PVis::Priv;
        while k < hi {
            if t(k) == b"#" && (t(k + 1) == b"[" || (t(k + 1) == b"!" && t(k + 2) == b"[")) {
                let o = if t(k + 1) == b"[" { k + 1 } else { k + 2 };
                k = close[o].saturating_add(1);
                continue;
            }
            if t(k) == b"pub" {
                vis = PVis::Pub;
                k += 1;
                if t(k) == b"(" && close[k] != usize::MAX {
                    let e = close[k];
                    vis = match t(k + 1) {
                        b"self" => PVis::Priv,
                        b"super" => PVis::Super,
                        b"in" => PVis::In((k + 2..e).filter(|&j| is_id(j)).map(t).collect()),
                        b"crate" => PVis::Crate,
                        _ => PVis::Pub,
                    };
                    k = e + 1;
                }
                continue;
            }
            if matches!(t(k), b"unsafe" | b"async" | b"default") || t(k) == b"auto" && t(k + 1) == b"trait" || t(k) == b"const" && matches!(t(k + 1), b"fn" | b"unsafe" | b"extern" | b"async") {
                k += 1;
                continue;
            }
            if t(k) == b"extern" && toks.get(k + 1).is_some_and(|x| x.0 == 2) && matches!(t(k + 2), b"fn" | b"unsafe") {
                k += 2;
                continue;
            }
            let mut item: Option<(usize, &'static str)> = None;
            match t(k) {
                b"mod" if is_id(k + 1) && t(k + 2) == b"{" && close[k + 2] < hi => {
                    its.push(PItem { name: t(raw(k + 1)), kind: "module", vis: vis.clone(), child: mods.len(), at: raw(k + 1) });
                    mods.push((k + 3, close[k + 2], m));
                }
                b"struct" if is_id(k + 1) => {
                    let mut j = k + 2;
                    if t(j) == b"<" {
                        let mut d = 0i32;
                        while j < hi {
                            match t(j) {
                                b"<" => d += 1,
                                b">" => {
                                    d -= 1;
                                    if d == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        j += 1;
                    }
                    item = Some((k + 1, match t(j) { b";" => "unit struct", b"(" => "tuple struct", _ => "struct" }));
                }
                b"enum" if is_id(k + 1) => item = Some((k + 1, "enum")),
                b"trait" if is_id(k + 1) => item = Some((k + 1, "trait")),
                b"type" if is_id(k + 1) => item = Some((k + 1, "type alias")),
                b"union" if is_id(k + 1) && t(k + 1) != b"union" => item = Some((k + 1, "union")),
                b"fn" if is_id(k + 1) => item = Some((k + 1, "function")),
                b"const" if is_id(k + 1) && t(k + 1) != b"_" => item = Some((k + 1, "constant")),
                b"static" if is_id(k + 1) => item = Some((if t(k + 1) == b"mut" { k + 2 } else { k + 1 }, "static")),
                b"use" => {
                    let (mut prefix, mut leaves, mut kinds) = (vec![], vec![], vec![]);
                    let mut j = k + 1;
                    if t(j) == b":" && t(j + 1) == b":" {
                        j += 2;
                    }
                    let end = use_leaves(b, &toks, &close, j, &mut prefix, &mut leaves, &mut kinds);
                    imp_at.insert(k, im.len());
                    for (l, &g) in leaves.iter().zip(&kinds) {
                        let name = match (g, l.last()) {
                            (2, _) | (_, None) => &b""[..],
                            (_, Some(&x)) if t(x + 1) == b"as" => t(raw(x + 2)),
                            (_, Some(&x)) => t(x),
                        };
                        im.push(PImport { name, segs: l.clone(), glob: g, vis: vis.clone(), root: injected || j != k + 1 });
                        if let Some(&last) = l.last() {
                            bound.insert(t(last));
                            if g != 2 {
                                bn.insert(t(last));
                            }
                        }
                        ended |= g == 2;
                    }
                    // (`use a::b as c;`: c)
                    for x in k..end.min(hi) {
                        if t(x) == b"as" && is_id(x + 1) {
                            bn.insert(t(raw(x + 1)));
                        }
                    }
                }
                // (`extern crate a;` / `extern crate a as b;`)
                b"extern" if t(k + 1) == b"crate" && is_id(k + 2) => {
                    bn.insert(t(raw(k + 2)));
                    if t(k + 3) == b"as" && is_id(k + 4) {
                        bn.insert(t(raw(k + 4)));
                    }
                }
                // (an `extern` block's items, a `macro`: not read)
                b"extern" if t(k + 1) == b"{" || toks.get(k + 1).is_some_and(|x| x.0 == 2) && t(k + 2) == b"{" => {
                    ended = true;
                    opq = true;
                }
                b"macro" if is_id(k + 1) => {
                    ended = true;
                    opq = true;
                }
                // (`macro_rules! m { .. }` / `m! { .. }` / `m!(..);`: not read)
                _ if is_id(k) && t(k + 1) == b"!" => {
                    let o = if is_id(k + 2) { k + 3 } else { k + 2 };
                    opq |= t(k) != b"macro_rules";
                    if close.get(o).is_some_and(|&e| e != usize::MAX) {
                        sk.push((k, close[o]));
                    }
                    ended = true;
                }
                _ => {}
            }
            if let Some((at, kind)) = item {
                its.push(PItem { name: t(raw(at)), kind, vis: vis.clone(), child: usize::MAX, at: raw(at) });
            }
            vis = PVis::Priv;
            // to the item's end: a `;` or a body `{ .. }`
            let mut j = if item.is_some() || t(k) == b"mod" { k + 1 } else { k };
            loop {
                if j >= hi {
                    break;
                }
                match t(j) {
                    b";" => break,
                    b"{" => {
                        j = close[j];
                        if t(j + 1) != b";" && !matches!(t(j + 1), b"." | b"?" | b"+" | b"-" | b"*" | b"/") {
                            break;
                        }
                    }
                    b"(" | b"[" => j = close[j],
                    _ => {}
                }
                if j == usize::MAX {
                    return None;
                }
                j += 1;
            }
            k = j.saturating_add(1);
        }
        items.push(its);
        skips.push(sk);
        binds.push(bn);
        open_ended.push(ended);
        imps.push(im);
        opaque.push(opq);
    }
    let parent = |m: usize| mods[m].2;
    let within = |mut a: usize, b: usize| loop {
        if a == b {
            break true;
        }
        if a == usize::MAX || a == 0 {
            break false;
        }
        a = parent(a);
    };
    // (the module a path of segments names, from the crate root; None if it names none)
    let module_at = |segs: &[&[u8]], from: usize| -> Option<usize> {
        let (mut cur, mut s) = match segs.first() {
            Some(&b"crate") => (0, 1),
            Some(&b"self") => (from, 1),
            _ => (from, 0),
        };
        while segs.get(s) == Some(&&b"super"[..]) {
            cur = parent(cur);
            if cur == usize::MAX {
                return None;
            }
            s += 1;
        }
        for seg in &segs[s..] {
            cur = items[cur].iter().find(|x| x.name == *seg && x.child != usize::MAX)?.child;
        }
        Some(cur)
    };
    let visible = |def: usize, vis: &PVis, from: usize| match vis {
        PVis::Pub | PVis::Crate => true,
        PVis::Priv => within(from, def),
        PVis::Super => match parent(def) {
            usize::MAX => within(from, def),
            p => within(from, p),
        },
        // (`pub(in path)`: from the crate root in 2015, from the module later; one that names no module is taken as
        // `pub`)
        PVis::In(p) => match module_at(p, if injected { 0 } else { def }) {
            Some(x) => within(from, x),
            None => true,
        },
    };
    // (the first segment a path cannot reach: its token and rustc's word for it)
    // (E0432 at segment i: the path to it)
    let unresolved = |segs: &[usize], i: usize| -> (usize, String) {
        let p: Vec<String> = segs[..=i].iter().map(|&x| String::from_utf8_lossy(t(x)).into_owned()).collect();
        (segs[i], format!("unresolved import `{}` (E0432)", p.join("::")))
    };
    // (whether only the module's own items define its names)
    let closed = |cur: usize, name: &[u8]| !unknown_mods && !open_ended[cur] && !binds[cur].contains(name);
    let check = |m: usize, segs: &[usize], from_root: bool, in_use: bool, glob: u8| -> Option<(usize, String)> {
        let first = t(segs[0]);
        let (mut cur, mut s) = match first {
            b"crate" => (0, 1),
            b"self" => (m, 1),
            b"super" => (m, 0),
            _ if from_root => (0, 0),
            _ => (m, 0),
        };
        while s < segs.len() && t(segs[s]) == b"super" {
            cur = parent(cur);
            if cur == usize::MAX {
                return None;
            }
            s += 1;
        }
        if s >= segs.len() || s == 0 && (bound.contains(first) || mod_names.get(first).is_some_and(|&c| c > 1)) {
            return None;
        }
        for i in s..segs.len() {
            let name = t(segs[i]);
            // (E0433: `super` / `crate` past the start, `self` past the start and before the end: `crate::x::super::f`)
            if matches!(name, b"super" | b"crate") || name == b"self" && i + 1 < segs.len() {
                let tail = if name == b"self" { " or last position" } else { "" };
                return Some((segs[i], format!("`{}` in paths can only be used in start position{tail} (E0433)", String::from_utf8_lossy(name))));
            }
            // (a glob's or a `self`'s module, `use m::f::{self}`, is no item: it must be a module, as the segments
            // before it)
            let last = i + 1 == segs.len() && glob == 0;
            let defs: Vec<&PItem> = items[cur].iter().filter(|x| x.name == name).collect();
            // (a `use` path from a module of the program: past the first segment of a relative path, which may name
            // another crate)
            let anchored = in_use && (i > 0 || from_root);
            if defs.is_empty() && anchored && closed(cur, name) {
                return Some(unresolved(segs, i));
            }
            // (a name defined here or nowhere: what an import or a macro brings is not followed)
            if defs.is_empty() || i == s && s == 0 && !from_root && !defs.iter().any(|x| x.child != usize::MAX) {
                return None;
            }
            // (on through an item that has no names of its own to import: a fn, a struct, a type alias, ..; not an
            // enum's variants nor a trait's items)
            if !last
                && anchored
                && !defs.iter().any(|x| x.child != usize::MAX || matches!(x.kind, "enum" | "trait"))
                && closed(cur, name)
            {
                return Some(unresolved(segs, i));
            }
            if i == s && s == 0 && !defs.iter().any(|x| x.child != usize::MAX) {
                return None;
            }
            if !last {
                // (a module goes on; a type ends the path, its associated items are not read: a struct there is the type,
                // `a::Foo::new`)
                let d = defs.iter().find(|x| x.child != usize::MAX).or_else(|| defs.iter().find(|x| matches!(x.kind, "struct" | "unit struct" | "tuple struct" | "enum" | "trait" | "type alias" | "union")))?;
                if !visible(cur, &d.vis, m) {
                    return Some((segs[i], format!("{} `{}` is private (E0603)", if d.kind.ends_with(" struct") { "struct" } else { d.kind }, String::from_utf8_lossy(name))));
                }
                if d.child == usize::MAX {
                    return None;
                }
                cur = d.child;
                continue;
            }
            if defs.iter().any(|d| visible(cur, &d.vis, m)) {
                return None;
            }
            let d = defs[0];
            // (what rustc calls a struct where the path is written: a value, a constructor, or the type)
            let mut word = d.kind;
            if word.ends_with(" struct") {
                // (the token before the path, past a leading `::` and any `&` / `mut`)
                let mut j = segs[0] as isize - if t(segs[0].wrapping_sub(1)) == b":" && t(segs[0].wrapping_sub(2)) == b":" { 3 } else { 1 };
                let tk = |j: isize| if j < 0 { &b""[..] } else { t(j as usize) };
                while matches!(tk(j), b"&" | b"mut") {
                    j -= 1;
                }
                let (p, pp) = (tk(j), tk(j - 1));
                // (`S { field: m::U }`: a struct literal's field takes a value; not a struct's or a variant's field)
                let enclosing = |mut j: isize| -> isize {
                    let mut d = 0i32;
                    while j >= 0 {
                        match tk(j) {
                            b")" | b"]" | b"}" => d += 1,
                            b"(" | b"[" | b"{" if d == 0 => return j,
                            b"(" | b"[" | b"{" => d -= 1,
                            _ => {}
                        }
                        j -= 1;
                    }
                    -1
                };
                let defines = |o: isize| o >= 1 && (matches!(tk(o - 2), b"struct" | b"union" | b"enum") || tk(o - 1) == b">");
                let field_value = p == b":" && pp != b":" && is_id((j - 1) as usize) && matches!(tk(j - 2), b"{" | b",") && {
                    let o = enclosing(j - 1);
                    o >= 1 && tk(o) == b"{" && is_id((o - 1) as usize) && !defines(o) && !matches!(tk(o - 2), b"{" | b",") || {
                        // (a variant's fields: `enum E { V { a: m::U } }`)
                        o >= 2 && tk(o) == b"{" && is_id((o - 1) as usize) && matches!(tk(o - 2), b"{" | b",") && {
                            let e = enclosing(o - 1);
                            !(e >= 2 && tk(e) == b"{" && tk(e - 2) == b"enum")
                        }
                    }
                };
                let typed = !field_value && (matches!(p, b"<" | b"as" | b"impl" | b"for" | b"dyn") || p == b":" && pp != b":" || p == b">" && pp == b"-");
                word = if in_use || typed {
                    "struct"
                } else if word == "tuple struct" {
                    "tuple struct constructor"
                } else {
                    word
                };
            }
            return Some((segs[i], format!("{word} `{}` is private (E0603)", String::from_utf8_lossy(name))));
        }
        None
    };
    // (E0659: read where some module has more than one glob import, and every module is in this text)
    let globs = imps.iter().flatten().filter(|x| x.glob == 2).count();
    let amb_on = globs >= 2 && !unknown_mods && !src.contains("ambiguous_glob_imports");
    // (E0451: read where a struct or union of a module other than the crate root has a field that is not `pub`; the
    // names of those)
    let mut hidden_structs: FxSet<&[u8]> = FxSet::default();
    if !unknown_mods {
        for (m, its) in items.iter().enumerate().skip(1) {
            for it in its {
                if matches!(it.kind, "struct" | "union") && struct_fields(b, toks, &close, it.at, mods[m].1).iter().any(|f| !matches!(f.1, PVis::Pub | PVis::Crate)) {
                    hidden_structs.insert(it.name);
                }
            }
        }
    }
    let priv_on = !hidden_structs.is_empty();
    // (E0624: the inherent impls of modules other than the crate root that have a member that is not `pub`; the names
    // trait fns have, whose calls may be the trait's)
    let (impls, trait_fns) = if mods.len() > 1 && !unknown_mods && src.contains("impl") { impl_members(b, toks, &close, &mods) } else { (vec![], FxSet::default()) };
    let assoc_on = impls.iter().any(|r| r.m != 0 && r.members.iter().any(|x| !matches!(x.2, PVis::Pub | PVis::Crate)));
    // (E0603 of an import: the names imports of modules other than the crate root bind that are not seen everywhere in
    // the crate; E0364 / E0365: whether any import re-exports)
    let imp_priv_names: FxSet<&[u8]> = if unknown_mods {
        FxSet::default()
    } else {
        imps.iter().skip(1).flatten().filter(|x| x.glob != 2 && !x.name.is_empty() && !matches!(x.vis, PVis::Pub | PVis::Crate)).map(|x| x.name).collect()
    };
    let reexp_on = !unknown_mods && mods.len() > 1 && imps.iter().flatten().any(|x| x.glob == 0 && !matches!(x.vis, PVis::Priv));
    // (E0603 / E0423 / E0532: the tuple structs of modules other than the crate root with a field that is not `pub`,
    // whose constructor is as visible as that field)
    let ctor_names: FxSet<&[u8]> = if unknown_mods {
        FxSet::default()
    } else {
        items
            .iter()
            .skip(1)
            .flatten()
            .filter(|it| it.kind == "tuple struct" && tuple_fields(b, toks, &close, it.at).is_some_and(|v| v.iter().any(|x| !matches!(x, PVis::Pub | PVis::Crate))))
            .map(|it| it.name)
            .collect()
    };
    // (E0432 of an import cycle or a glob of a module into itself: a `pub use` of a path through the program's modules,
    // or a glob of `self` / `super` / `crate`)
    let cycle_on = !unknown_mods && imps.iter().flatten().any(|x| x.glob == 0 && x.segs.len() >= 2 && !matches!(x.vis, PVis::Priv))
        || !unknown_mods && imps.iter().flatten().any(|x| x.glob == 2 && x.segs.first().is_some_and(|&f| matches!(t(f), b"self" | b"super" | b"crate")));
    let mut amb = (amb_on || priv_on || assoc_on || !imp_priv_names.is_empty() || reexp_on || cycle_on || !ctor_names.is_empty()).then(|| Ambiguity {
        b,
        toks,
        close: &close,
        mods: &mods,
        items: &items,
        imps: &imps,
        opaque: &opaque,
        injected,
        busy: vec![],
        busy_imp: vec![],
        steps: 0,
        targets: FxMap::default(),
        named: items
            .iter()
            .map(|its| {
                let mut ix: FxMap<&[u8], Vec<u32>> = FxMap::default();
                for (i, it) in its.iter().enumerate() {
                    ix.entry(it.name).or_default().push(i as u32);
                }
                ix
            })
            .collect(),
        cycles: vec![],
        unknown: false,
    });
    // (the names a glob may bring: the program's items, enum variants and import names; for E0451 alone, the names a
    // literal's path starts with: those structs' and the modules')
    let mut amb_names: FxSet<&[u8]> = FxSet::default();
    if !amb_on {
        amb_names.extend(hidden_structs.iter().copied());
        amb_names.extend(items.iter().flatten().filter(|x| x.child != usize::MAX).map(|x| x.name));
    } else if let Some(a) = &amb {
        for (m, its) in items.iter().enumerate() {
            for it in its {
                amb_names.insert(it.name);
                if it.kind == "enum" {
                    amb_names.extend(a.variants_at(it.at, mods[m].1).into_iter().map(|k| t(k)));
                }
            }
        }
        amb_names.extend(imps.iter().flatten().map(|x| x.name).filter(|x| !x.is_empty()));
    }
    // (E0624: each private member of an inherent impl, by its type and name: what it is and the module it is visible
    // within; a member two impls give different visibilities is left out. The type check's share: by the type's name,
    // where no other item has it, the fns, not of a name a trait fn has)
    let mut assoc: FxMap<(u32, u32, &[u8]), Option<(&'static str, usize)>> = FxMap::default();
    let mut methods: FxMap<String, Vec<(String, u32)>> = FxMap::default();
    if let (true, Some(a)) = (assoc_on, amb.as_mut()) {
        let mut count: FxMap<&[u8], u32> = FxMap::default();
        for it in items.iter().flatten() {
            *count.entry(it.name).or_insert(0) += 1;
        }
        for r in &impls {
            if r.self_path.is_empty() {
                continue;
            }
            let APath::Defs(d) = a.path(r.m, &r.self_path, false, 1) else { continue };
            let types: Vec<ADef> = d.iter().filter(|x| x.0 & 1 != 0).map(|x| x.1).collect();
            let &[x] = &types[..] else { continue };
            let it = &items[x.m as usize][x.i as usize];
            if x.v != u32::MAX || !matches!(it.kind, "struct" | "unit struct" | "tuple struct" | "enum" | "union") {
                continue;
            }
            for &(nt, kind, ref vis) in &r.members {
                let scope = match vis {
                    PVis::Pub | PVis::Crate => None,
                    PVis::Priv => Some(r.m),
                    PVis::Super => Some(if r.m == 0 { 0 } else { mods[r.m].2 }),
                    PVis::In(p) => a.module_at(p, if injected { 0 } else { r.m }),
                };
                let key = (x.m, x.i, t(nt));
                let v = scope.map(|sc| (kind, sc));
                match assoc.get(&key) {
                    Some(old) if *old != v => {
                        assoc.insert(key, None);
                    }
                    _ => {
                        assoc.entry(key).or_insert(v);
                    }
                }
            }
        }
        for (&(m, i, name), v) in &assoc {
            let it = &items[m as usize][i as usize];
            if let Some(("associated function", sc)) = v {
                if count.get(it.name) == Some(&1) && !trait_fns.contains(name) && *sc != 0 {
                    methods.entry(String::from_utf8_lossy(it.name).into_owned()).or_default().push((String::from_utf8_lossy(name).into_owned(), *sc as u32));
                }
            }
        }
    }
    let assoc_names: FxSet<&[u8]> = assoc.keys().map(|k| k.2).collect();
    if priv_on || !methods.is_empty() {
        let mut fp = field_priv(b, toks, &close, &mods, &items, injected);
        fp.methods = methods;
        *fpriv = Some(Box::new(fp));
    }
    let amb_msg = |k: usize| Some((k, format!("`{}` is ambiguous (E0659)", String::from_utf8_lossy(t(k)))));
    // (E0433: a path with more leading `super`s than its module has parents: "too many leading `super` keywords within
    // `crate::outer::inner`", at the first one past the crate root)
    let mod_path = |m: usize| -> String {
        let mut names: Vec<String> = vec![];
        let mut cur = m;
        while cur != 0 && cur != usize::MAX {
            let p = mods[cur].2;
            match items.get(p).and_then(|its| its.iter().find(|x| x.child == cur)) {
                Some(it) => names.push(String::from_utf8_lossy(it.name).into_owned()),
                None => break,
            }
            cur = p;
        }
        names.push("crate".to_string());
        names.reverse();
        names.join("::")
    };
    let super_depth = |m: usize, segs: &[usize]| -> Option<(usize, String)> {
        let mut i = usize::from(segs.first().is_some_and(|&x| t(x) == b"self"));
        let mut cur = m;
        while i < segs.len() && t(segs[i]) == b"super" {
            if cur == 0 {
                return Some((segs[i], format!("too many leading `super` keywords within `{}` (E0433)", mod_path(m))));
            }
            cur = mods[cur].2;
            i += 1;
        }
        None
    };
    // (each module's own tokens, past its child modules and its item-level macros, for the paths in them)
    let mut found: Option<(usize, String)> = None;
    let mut found_late: Option<(usize, String)> = None;
    let mut keep_late = |at: usize, msg: String| {
        if found_late.as_ref().is_none_or(|f| at < f.0) {
            found_late = Some((at, msg));
        }
    };
    let mut keep = |e: Option<(usize, String)>| {
        if let Some(e) = e {
            if found.as_ref().is_none_or(|f| e.0 < f.0) {
                found = Some(e);
            }
        }
    };
    for m in 0..mods.len() {
        let (lo, hi, _) = mods[m];
        let mut jumps: Vec<(usize, usize)> = skips[m].clone();
        jumps.extend(mods.iter().filter(|x| x.2 == m).map(|x| (x.0, x.1)));
        jumps.sort_unstable();
        // (the module's scope facts, read at its first site)
        let sc: std::cell::OnceCell<AScope> = std::cell::OnceCell::new();
        let has_glob = amb_on && imps[m].iter().any(|x| x.glob == 2);
        // (whether token k is a name a path may start with here, for E0659 and E0451: not a binding, a definition, or
        // in a macro's arguments)
        macro_rules! free {
            ($a:expr, $k:expr) => {{
                let k: usize = $k;
                let sc = sc.get_or_init(|| $a.scope(lo, hi, &jumps, &amb_names));
                let name = t(k);
                !sc.defs.contains(&k) && !sc.skip.iter().any(|&(a, e)| a < k && k < e) && !sc.covers.iter().any(|&(nm, a, e)| (nm == name || nm.is_empty()) && a <= k && k <= e)
            }};
        }
        let after_colons = |k: usize| t(k.wrapping_sub(1)) == b":" && t(k.wrapping_sub(2)) == b":";
        // (whether a path from token k followed by `{` may be a struct literal or pattern: not a definition's name, an
        // impl's or a fn's return type, nor the expression of an `if` / `while` / `match` / `for`)
        let literal_at = |k: usize| {
            let j = if after_colons(k) { k.wrapping_sub(3) } else { k.wrapping_sub(1) };
            let p = t(j);
            !(matches!(p, b"struct" | b"enum" | b"union" | b"trait" | b"impl" | b"for" | b"mod" | b"fn" | b"if" | b"while" | b"match" | b"in" | b"dyn" | b"where" | b"type")
                || p == b">" && t(j.wrapping_sub(1)) == b"-")
        };
        let mut cache: FxMap<&[u8], ADefs> = FxMap::default();
        let mut ji = 0;
        let mut k = lo;
        while k < hi {
            while ji < jumps.len() && jumps[ji].1 < k {
                ji += 1;
            }
            if ji < jumps.len() && jumps[ji].0 <= k {
                k = jumps[ji].1 + 1;
                continue;
            }
            // (an attribute; a module of a fn body, whose paths start there)
            if t(k) == b"#" && (t(k + 1) == b"[" || t(k + 1) == b"!" && t(k + 2) == b"[") {
                let o = if t(k + 1) == b"[" { k + 1 } else { k + 2 };
                k = close[o].saturating_add(1);
                continue;
            }
            if t(k) == b"mod" && is_id(k + 1) && t(k + 2) == b"{" && close[k + 2] != usize::MAX {
                k = close[k + 2] + 1;
                continue;
            }
            if t(k) == b"use" && !(t(k.wrapping_sub(1)) == b":" || t(k.wrapping_sub(1)) == b".") {
                let mut j = k + 1;
                let lead = t(j) == b":" && t(j + 1) == b":";
                if lead {
                    j += 2;
                }
                let (mut prefix, mut leaves, mut kinds) = (vec![], vec![], vec![]);
                let end = use_leaves(b, &toks, &close, j, &mut prefix, &mut leaves, &mut kinds);
                if !lead || injected {
                    for (l, &g) in leaves.iter().zip(&kinds).filter(|(l, _)| !l.is_empty()) {
                        keep(check(m, l, injected, true, g));
                        keep(super_depth(m, l));
                    }
                }
                // (E0603 of a private import on the path; E0364 / E0365: a re-export wider than what it names)
                if let (Some(a), Some(&j0)) = (amb.as_mut(), imp_at.get(&k)) {
                    for (li, (l, &g)) in leaves.iter().zip(&kinds).enumerate() {
                        let Some(imp) = imps[m].get(j0 + li) else { continue };
                        if l.is_empty() {
                            continue;
                        }
                        a.busy_imp.push((m, j0 + li));
                        if l.iter().any(|&x| imp_priv_names.contains(t(x))) {
                            keep(a.import_privacy(m, l, imp.root, false));
                        }
                        if reexp_on && g == 0 && !matches!(imp.vis, PVis::Priv) {
                            keep(a.reexport(m, l, imp.root, &imp.vis));
                        }
                        if cycle_on && g == 0 && !matches!(imp.vis, PVis::Priv) {
                            keep(a.import_cycle(m, j0 + li, l, imp.root));
                        }
                        // (a glob of the module into itself: `use self::*;`)
                        if g == 2 && a.glob_target(m, j0 + li) == Some((m, None)) {
                            let p: Vec<String> = l.iter().map(|&x| String::from_utf8_lossy(t(x)).into_owned()).collect();
                            keep(Some((l[0], format!("unresolved import `{}::*` (E0432)", p.join("::")))));
                        }
                        a.busy_imp.pop();
                    }
                }
                // (E0659: an import of the module, resolved without itself)
                if let (Some(a), Some(&j0), true) = (amb.as_mut(), imp_at.get(&k), amb_on) {
                    for (li, (l, &g)) in leaves.iter().zip(&kinds).enumerate() {
                        if l.is_empty() || imps[m].len() <= j0 + li {
                            continue;
                        }
                        a.busy_imp.push((m, j0 + li));
                        let r = a.path(m, l, imps[m][j0 + li].root, if g == 0 { 3 } else { 1 });
                        a.busy_imp.pop();
                        match r {
                            APath::Amb(i) => keep(amb_msg(l[i])),
                            APath::Defs(d) if ambiguous_in(&d, if g == 0 { 0 } else { 1 }) => keep(amb_msg(*l.last().unwrap())),
                            _ => {}
                        }
                    }
                }
                k = end.max(k + 1);
                continue;
            }
            // (a path's first segment: not after `::`, `.` or a macro's `$`)
            let after_colons = |k: usize| t(k.wrapping_sub(1)) == b":" && t(k.wrapping_sub(2)) == b":";
            if is_id(k) && t(k + 1) == b":" && t(k + 2) == b":" && is_id(k + 3) && !(after_colons(k) || matches!(t(k.wrapping_sub(1)), b"." | b"$" | b"'")) {
                // (a path can reach a module of the program only from one of their names, `crate`, `self` or `super`)
                let first = t(raw(k));
                let reads = matches!(first, b"crate" | b"self" | b"super") || mod_names.contains_key(first);
                let mut segs = if reads { vec![raw(k)] } else { vec![] };
                let mut j = k;
                while t(j + 1) == b":" && t(j + 2) == b":" && is_id(j + 3) {
                    j = raw(j + 3);
                    if reads {
                        segs.push(j);
                    }
                }
                if reads {
                    keep(check(m, &segs, false, false, 0));
                    keep(super_depth(m, &segs));
                }
                // (E0659: a segment of the path; E0451: a struct literal's or pattern's fields, where its last segment
                // names a struct with such a field; the scope facts last, as they are read only then)
                let x = j;
                let lit = priv_on && t(x + 1) == b"{" && hidden_structs.contains(t(x));
                let member = assoc_on && assoc_names.contains(t(x));
                let ctor = !ctor_names.is_empty() && ctor_names.contains(t(x)) && t(x + 1) == b"(";
                let via_import = !imp_priv_names.is_empty() && {
                    let mut y = raw(k);
                    let mut any = false;
                    while t(y + 1) == b":" && t(y + 2) == b":" && is_id(y + 3) {
                        y = raw(y + 3);
                        any |= imp_priv_names.contains(t(y));
                    }
                    any
                };
                if amb.is_some() && (amb_on || lit || member || via_import || ctor) {
                    let mut all = vec![raw(k)];
                    let mut y = raw(k);
                    while t(y + 1) == b":" && t(y + 2) == b":" && is_id(y + 3) {
                        y = raw(y + 3);
                        all.push(y);
                    }
                    if amb_on && amb.as_ref().is_some_and(|a| free!(a, raw(k))) {
                        let a = amb.as_mut().unwrap();
                        match a.path(m, &all, false, 3) {
                            APath::Amb(i) => keep(amb_msg(all[i])),
                            APath::Defs(d) if ambiguous_in(&d, a.site_ns(raw(k), x)) => keep(amb_msg(x)),
                            _ => {}
                        }
                    }
                    // (E0603: a private import on the path, `a::b::S` with `use x::S;` in b)
                    if via_import && amb.as_ref().is_some_and(|a| free!(a, raw(k))) {
                        let a = amb.as_mut().unwrap();
                        let value = !matches!(a.site_ns(raw(k), x), 1);
                        keep(a.import_privacy(m, &all, false, value));
                    }
                    // (E0603: `a::S(1)` of a tuple struct with a field the module may not see)
                    if ctor && amb.as_ref().is_some_and(|a| free!(a, raw(k))) {
                        let a = amb.as_mut().unwrap();
                        if let Some(name) = a.ctor_hidden(m, &all) {
                            keep(Some((x, format!("tuple struct constructor `{}` is private (E0603)", String::from_utf8_lossy(name)))));
                        }
                    }
                    // (E0624: `a::S::new()`, `Foo::ID`: a private member of the type's inherent impl)
                    if member && all.len() >= 2 {
                        let a = amb.as_mut().unwrap();
                        if let APath::Defs(d) = a.path(m, &all[..all.len() - 1], false, 1) {
                            let types: Vec<ADef> = d.iter().filter(|x| x.0 & 1 != 0).map(|x| x.1).collect();
                            if let &[ty] = &types[..] {
                                if let Some(&Some((kind, sc))) = assoc.get(&(ty.m, ty.i, t(x))) {
                                    if !a.within(m, sc) && free!(a, raw(k)) {
                                        keep(Some((x, format!("{kind} `{}` is private (E0624)", String::from_utf8_lossy(t(x))))));
                                    }
                                }
                            }
                        }
                    }
                    if lit && literal_at(raw(k)) {
                        let r = amb.as_mut().and_then(|a| a.literal_privacy(m, &all, x + 1));
                        if r.is_some() && amb.as_ref().is_some_and(|a| free!(a, raw(k))) {
                            match r {
                                Some((at, msg, true)) => keep_late(at, msg),
                                Some((at, msg, false)) => keep(Some((at, msg))),
                                None => {}
                            }
                        }
                    }
                }
                k = j + 1;
                continue;
            }
            // (`::a::b`: from the crate root in 2015)
            if t(k) == b":" && t(k + 1) == b":" && is_id(k + 2) && !(is_id(k.wrapping_sub(1)) || matches!(t(k.wrapping_sub(1)), b">" | b"$" | b":")) {
                let mut segs = vec![raw(k + 2)];
                let mut j = segs[0];
                while t(j + 1) == b":" && t(j + 2) == b":" && is_id(j + 3) {
                    j = raw(j + 3);
                    segs.push(j);
                }
                if injected && segs.len() > 1 {
                    keep(check(m, &segs, true, false, 0));
                }
                if amb.as_ref().is_some_and(|a| amb_on && injected && free!(a, segs[0])) {
                    let a = amb.as_mut().unwrap();
                    match a.path(m, &segs, true, 3) {
                        APath::Amb(i) => keep(amb_msg(segs[i])),
                        APath::Defs(d) if ambiguous_in(&d, a.site_ns(k, j)) => keep(amb_msg(j)),
                        _ => {}
                    }
                }
                k = j + 1;
                continue;
            }
            // (E0423 / E0532: `S(1)` / `S(x) =>` of a tuple struct with a field the module may not see)
            if !ctor_names.is_empty()
                && is_id(k)
                && t(k + 1) == b"("
                && ctor_names.contains(t(k))
                && !after_colons(k)
                && !matches!(t(k.wrapping_sub(1)), b"." | b"$" | b"'" | b"#" | b"fn" | b"struct" | b"enum" | b"union" | b"impl" | b"for" | b"dyn" | b"->" | b"<")
                && amb.as_ref().is_some_and(|a| free!(a, k))
            {
                let a = amb.as_mut().unwrap();
                if a.ctor_hidden(m, &[k]).is_some() {
                    // (a pattern: `let S(x) =`, `S(x) =>`, a parameter's `S(x): S`, an or-pattern)
                    let e = close[k + 1];
                    let after = t(e.wrapping_add(1));
                    let pattern = t(k.wrapping_sub(1)) == b"let" || matches!(after, b"|" | b"if" | b"in") || after == b":" && t(e + 2) != b":" || after == b"=" && t(e + 2) != b"=";
                    let msg = if pattern { "cannot match against a tuple struct which contains private fields (E0532)" } else { "cannot initialize a tuple struct which contains private fields (E0423)" };
                    keep(Some((k, msg.to_string())));
                }
            }
            // (E0451: a struct literal or pattern of one segment, `Foo { x: 1 }`)
            if priv_on
                && is_id(k)
                && t(k + 1) == b"{"
                && hidden_structs.contains(t(k))
                && !after_colons(k)
                && !matches!(t(k.wrapping_sub(1)), b"." | b"$" | b"'" | b"#")
                && literal_at(k)
            {
                let r = amb.as_mut().and_then(|a| a.literal_privacy(m, &[k], k + 1));
                if r.is_some() && amb.as_ref().is_some_and(|a| free!(a, k)) {
                    match r {
                        Some((at, msg, true)) => keep_late(at, msg),
                        Some((at, msg, false)) => keep(Some((at, msg))),
                        None => {}
                    }
                }
            }
            // (E0659: a name a glob of the module brings, where a path of one segment names it)
            if has_glob && is_id(k) && amb_names.contains(t(k)) && amb.as_ref().is_some_and(|a| free!(a, k)) {
                let (p, pp, nx) = (t(k.wrapping_sub(1)), t(k.wrapping_sub(2)), t(k + 1));
                let defines = matches!(p, b"fn" | b"struct" | b"enum" | b"trait" | b"type" | b"union" | b"mod" | b"const" | b"static" | b"crate" | b"macro_rules" | b"macro")
                    || p == b"mut" && pp != b"&"
                    || p == b"ref"
                    || p == b"!" && pp == b"macro_rules";
                // (nor a field's name, a macro's named argument (`format!("{x}", x = 1)`))
                let not_path = matches!(p, b"." | b"$" | b"'" | b"#")
                    || after_colons(k)
                    || matches!(nx, b"!" | b"@")
                    || nx == b":" && t(k + 2) != b":"
                    || p == b"," && nx == b"=" && !matches!(t(k + 2), b"=" | b">");
                if !defines && !not_path {
                    if let Some(a) = amb.as_mut() {
                        let name = t(k);
                        let d = match cache.get(name) {
                            Some(d) => d.clone(),
                            None => {
                                let d = a.lookup(m, name, m, 3);
                                cache.insert(name, d.clone());
                                d
                            }
                        };
                        if d.is_some_and(|d| ambiguous_in(&d, a.site_ns(k, k))) {
                            keep(amb_msg(k));
                        }
                    }
                }
            }
            k += 1;
        }
    }
    let place = |(at, msg): (usize, String)| {
        let off = toks[at].1;
        let line = b[..off].iter().filter(|&&c| c == b'\n').count() + 1;
        let col = off - src[..off].rfind('\n').map_or(0, |p| p + 1) + 1;
        format!("{line}:{col}: {msg}")
    };
    *late = found_late.map(place);
    found.map(place)
}


/// An inherent impl, as impl_members reads it: its module, its self type's path, and its fns and consts (the name's
/// token, rustc's word for it, its visibility)
struct ImplRec<'a> {
    m: usize,
    self_path: Vec<usize>,
    members: Vec<(usize, &'static str, PVis<'a>)>,
}

/// The inherent impls in each module's own text (in its fn bodies too: a block is no module), and the names of the fns
/// of traits and trait impls
fn impl_members<'a>(b: &'a [u8], toks: &[(u8, usize, usize)], close: &[usize], mods: &[(usize, usize, usize)]) -> (Vec<ImplRec<'a>>, FxSet<&'a [u8]>) {
    let t = |k: usize| -> &'a [u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let (mut out, mut trait_fns): (Vec<ImplRec<'a>>, FxSet<&'a [u8]>) = (vec![], FxSet::default());
    for (m, &(lo, hi, _)) in mods.iter().enumerate() {
        let mut kids: Vec<(usize, usize)> = mods.iter().filter(|x| x.2 == m && x.0 > 0).map(|x| (x.0, x.1)).collect();
        kids.sort_unstable();
        let mut ki = 0;
        let mut k = lo;
        while k < hi {
            while ki < kids.len() && kids[ki].1 < k {
                ki += 1;
            }
            if ki < kids.len() && kids[ki].0 <= k {
                k = kids[ki].1 + 1;
                continue;
            }
            let is_trait = t(k) == b"trait" && is_id(k + 1);
            let is_impl = t(k) == b"impl" && (k == lo || matches!(t(k - 1), b"}" | b";" | b"{" | b"]" | b"unsafe" | b"default"));
            if !is_trait && !is_impl {
                k += 1;
                continue;
            }
            // (the header, to the body's `{`: generics skipped; `for` makes it a trait impl)
            let mut j = k + 1;
            let mut d = 0i32;
            let mut path: Vec<usize> = vec![];
            let mut for_ = false;
            let mut done_path = false;
            if t(j) == b"<" && is_impl {
                while j < hi {
                    match t(j) {
                        b"<" => d += 1,
                        b">" if t(j - 1) != b"-" => {
                            d -= 1;
                            if d == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                j += 1;
                d = 0;
            }
            while j < hi && !(t(j) == b"{" && d == 0) {
                match t(j) {
                    b"<" => d += 1,
                    b">" if t(j - 1) != b"-" => d -= 1,
                    b"(" | b"[" if close[j] != usize::MAX => {
                        j = close[j];
                        done_path = true;
                    }
                    b"for" if d == 0 => for_ = true,
                    b"where" if d == 0 => done_path = true,
                    b";" if d == 0 => break,
                    _ => {}
                }
                if d == 0 && !done_path && !for_ && is_id(j) && (path.is_empty() || t(j - 1) == b":") {
                    path.push(j);
                } else if d == 0 && !matches!(t(j), b":" | b"<" | b">") && !is_id(j) {
                    done_path = true;
                }
                if d > 0 {
                    done_path = true;
                }
                j += 1;
            }
            let Some(&e) = close.get(j).filter(|&&e| t(j) == b"{" && e != usize::MAX) else {
                k = j + 1;
                continue;
            };
            let mut rec = ImplRec { m, self_path: path, members: vec![] };
            let mut x = j + 1;
            while x < e {
                while t(x) == b"#" && t(x + 1) == b"[" && close[x + 1] != usize::MAX {
                    x = close[x + 1] + 1;
                }
                let mut vis = PVis::Priv;
                if t(x) == b"pub" {
                    vis = PVis::Pub;
                    x += 1;
                    if t(x) == b"(" && close[x] != usize::MAX {
                        let c = close[x];
                        vis = match t(x + 1) {
                            b"self" => PVis::Priv,
                            b"super" => PVis::Super,
                            b"in" => PVis::In((x + 2..c).filter(|&q| is_id(q)).map(t).collect()),
                            b"crate" => PVis::Crate,
                        _ => PVis::Pub,
                        };
                        x = c + 1;
                    }
                }
                while matches!(t(x), b"default" | b"async" | b"unsafe" | b"safe") || t(x) == b"const" && matches!(t(x + 1), b"fn" | b"unsafe" | b"async" | b"extern") || t(x) == b"extern" && toks.get(x + 1).is_some_and(|q| q.0 == 2) {
                    x += if t(x) == b"extern" { 2 } else { 1 };
                }
                let item = match t(x) {
                    b"fn" if is_id(x + 1) => Some((x + 1, "associated function")),
                    b"const" if is_id(x + 1) && t(x + 2) == b":" => Some((x + 1, "associated constant")),
                    _ => None,
                };
                if let Some((nt, kind)) = item {
                    if is_trait || for_ {
                        if kind == "associated function" {
                            trait_fns.insert(t(nt));
                        }
                    } else {
                        rec.members.push((nt, kind, vis));
                    }
                }
                // (to the member's end: its `;` or its body)
                while x < e && t(x) != b";" {
                    if t(x) == b"{" && close[x] != usize::MAX {
                        x = close[x];
                        break;
                    }
                    if matches!(t(x), b"(" | b"[") && close[x] != usize::MAX {
                        x = close[x];
                    }
                    x += 1;
                }
                x += 1;
            }
            if is_impl && !for_ && !rec.members.is_empty() {
                out.push(rec);
            }
            k = e + 1;
        }
    }
    (out, trait_fns)
}

/// For E0616: the modules by their lines, and the fields of the structs and unions of modules other than the crate
/// root that are not `pub`, with the module each is visible within. Only a name no other item of the program has: the
/// type check knows a struct by its name, which the flattening of modules keeps where it is the only one
fn field_priv(b: &[u8], toks: &[(u8, usize, usize)], close: &[usize], mods: &[(usize, usize, usize)], items: &[Vec<PItem>], injected: bool) -> crate::program::FieldPriv {
    // (the lines of the modules' braces, counted in one pass over the text up to the last)
    let mut offs: Vec<usize> = mods.iter().skip(1).flat_map(|&(lo, hi, _)| [lo.saturating_sub(1), hi]).map(|k| toks.get(k).map_or(b.len(), |x| x.1)).collect();
    offs.sort_unstable();
    offs.dedup();
    let mut lines: FxMap<usize, u32> = FxMap::default();
    let (mut at, mut n) = (0usize, 1u32);
    for &o in &offs {
        n += b[at..o].iter().filter(|&&c| c == b'\n').count() as u32;
        at = o;
        lines.insert(o, n);
    }
    let line = |k: usize| -> u32 { lines[&toks.get(k).map_or(b.len(), |x| x.1)] };
    let mut fp = crate::program::FieldPriv::default();
    for (m, &(lo, hi, parent)) in mods.iter().enumerate() {
        fp.mods.push(if m == 0 { (0, u32::MAX, 0) } else { (line(lo.saturating_sub(1)), line(hi), parent as u32) });
    }
    let mut count: FxMap<&[u8], u32> = FxMap::default();
    for it in items.iter().flatten() {
        *count.entry(it.name).or_insert(0) += 1;
    }
    let parent = |m: usize| mods[m].2;
    for (m, its) in items.iter().enumerate().skip(1) {
        for it in its {
            if !matches!(it.kind, "struct" | "union") || count.get(it.name) != Some(&1) {
                continue;
            }
            let mut hidden = vec![];
            for (f, vis) in struct_fields(b, toks, close, it.at, mods[m].1) {
                let scope = match vis {
                    PVis::Pub | PVis::Crate => continue,
                    PVis::Priv => m,
                    PVis::Super => parent(m),
                    // (`pub(in path)`: the module it names, from the crate root in 2015)
                    PVis::In(p) => {
                        let (mut cur, mut s) = match p.first() {
                            Some(&b"crate") => (0, 1),
                            Some(&b"self") => (m, 1),
                            _ if injected => (0, 0),
                            _ => (m, 0),
                        };
                        while p.get(s) == Some(&&b"super"[..]) && cur != 0 {
                            cur = parent(cur);
                            s += 1;
                        }
                        let found = p[s..].iter().try_fold(cur, |c, seg| items[c].iter().find(|x| x.name == *seg && x.child != usize::MAX).map(|x| x.child));
                        match found {
                            Some(x) => x,
                            None => continue,
                        }
                    }
                };
                hidden.push((String::from_utf8_lossy(f).into_owned(), scope as u32));
            }
            if !hidden.is_empty() {
                fp.fields.insert(String::from_utf8_lossy(it.name).into_owned(), (if it.kind == "union" { "union" } else { "struct" }, hidden));
            }
        }
    }
    fp
}

/// The named fields of the struct or union whose name is token `at`, each with its visibility as written (none for a
/// tuple or unit struct)
fn struct_fields<'a>(b: &'a [u8], toks: &[(u8, usize, usize)], close: &[usize], at: usize, hi: usize) -> Vec<(&'a [u8], PVis<'a>)> {
    let t = |k: usize| -> &'a [u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let mut out = vec![];
    // (past the generics and a `where` clause, to the body)
    let mut k = at + 1;
    let mut d = 0i32;
    while k < hi {
        match t(k) {
            b"<" => d += 1,
            b">" if t(k - 1) != b"-" => d -= 1,
            b"{" if d == 0 => break,
            b"(" | b";" if d == 0 => return out,
            b"(" | b"[" if close[k] != usize::MAX => k = close[k],
            _ => {}
        }
        k += 1;
    }
    let Some(&e) = close.get(k).filter(|&&e| e != usize::MAX && k < hi) else { return out };
    k += 1;
    while k < e {
        while t(k) == b"#" && t(k + 1) == b"[" && close[k + 1] != usize::MAX {
            k = close[k + 1] + 1;
        }
        let mut vis = PVis::Priv;
        if t(k) == b"pub" {
            vis = PVis::Pub;
            k += 1;
            if t(k) == b"(" && close[k] != usize::MAX {
                let c = close[k];
                vis = match t(k + 1) {
                    b"self" => PVis::Priv,
                    b"super" => PVis::Super,
                    b"in" => PVis::In((k + 2..c).filter(|&j| is_id(j)).map(t).collect()),
                    b"crate" => PVis::Crate,
                        _ => PVis::Pub,
                };
                k = c + 1;
            }
        }
        let s = if t(k) == b"r" && t(k + 1) == b"#" && is_id(k + 2) { k + 2 } else { k };
        if is_id(s) && t(s + 1) == b":" {
            out.push((t(s), vis));
        }
        while k < e && t(k) != b"," {
            if matches!(t(k), b"(" | b"[" | b"{") && close[k] != usize::MAX {
                k = close[k];
            }
            k += 1;
        }
        k += 1;
    }
    out
}

/// The visibilities of the fields of the tuple struct whose name is token `at` (None for another struct)
fn tuple_fields<'a>(b: &'a [u8], toks: &[(u8, usize, usize)], close: &[usize], at: usize) -> Option<Vec<PVis<'a>>> {
    let t = |k: usize| -> &'a [u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let mut k = at + 1;
    if t(k) == b"<" {
        let mut d = 0i32;
        while k < toks.len() {
            match t(k) {
                b"<" => d += 1,
                b">" if t(k - 1) != b"-" => {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        k += 1;
    }
    let e = *close.get(k).filter(|&&e| t(k) == b"(" && e != usize::MAX)?;
    let mut out = vec![];
    k += 1;
    while k < e {
        while t(k) == b"#" && t(k + 1) == b"[" && close[k + 1] != usize::MAX {
            k = close[k + 1] + 1;
        }
        let mut vis = PVis::Priv;
        if t(k) == b"pub" {
            vis = PVis::Pub;
            if t(k + 1) == b"(" && close[k + 1] != usize::MAX && matches!(t(k + 2), b"crate" | b"self" | b"super" | b"in") {
                let c = close[k + 1];
                vis = match t(k + 2) {
                    b"self" => PVis::Priv,
                    b"super" => PVis::Super,
                    b"in" => PVis::In((k + 3..c).filter(|&j| is_id(j)).map(t).collect()),
                    _ => PVis::Crate,
                };
                k = c;
            }
        }
        if k < e {
            out.push(vis);
        }
        while k < e && t(k) != b"," {
            if matches!(t(k), b"(" | b"[" | b"{") && close[k] != usize::MAX {
                k = close[k];
            }
            k += 1;
        }
        k += 1;
    }
    Some(out)
}

/// An import of a module, as `private_path` reads it: `use a::b as c;` is (c, [a, b], 0), `use a::*;` (``, [a], 2),
/// `use a::{self};` (a, [a], 1)
struct PImport<'a> {
    name: &'a [u8],
    segs: Vec<usize>,
    glob: u8,
    vis: PVis<'a>,
    /// whether the path starts at the crate root (edition 2015, or a leading `::`)
    root: bool,
}

/// What a name means: the item of a module (`v` u32::MAX), or a variant of an enum item
#[derive(Clone, Copy, PartialEq, Eq)]
struct ADef {
    m: u32,
    i: u32,
    v: u32,
}

/// The definitions a name has in a module, each with its namespaces (1 types, 2 values); None: what it means is not
/// known (an import of another crate, a module a macro may add items to)
type ADefs = Option<Vec<(u8, ADef)>>;

/// What a path names: its last segment's definitions, or the segment that is ambiguous (E0659), or nothing known
enum APath {
    Defs(Vec<(u8, ADef)>),
    Amb(usize),
    Unknown,
}

/// E0659 for `private_path`: a name two glob imports of a module bring with different definitions (`use a::*; use
/// b::*;` of two `struct S`), where a path names it. A module's own items and its other imports take the name before
/// a glob does; the definitions are followed through imports and globs of the program's own modules and enums, and a
/// name of another crate or of a module with macro-made items is left alone
struct Ambiguity<'a, 'b> {
    b: &'a [u8],
    toks: &'b [(u8, usize, usize)],
    close: &'b [usize],
    mods: &'b [(usize, usize, usize)],
    items: &'b [Vec<PItem<'a>>],
    imps: &'b [Vec<PImport<'a>>],
    /// (a module with an item-level macro call, an `extern` block or a `macro`: its names are not all known)
    opaque: &'b [bool],
    injected: bool,
    /// (the lookups and imports being resolved: a cycle of globs finds nothing more)
    busy: Vec<(usize, &'a [u8], usize)>,
    busy_imp: Vec<(usize, usize)>,
    steps: u32,
    /// (what each glob import reads, once found)
    targets: FxMap<(usize, usize), Option<(usize, Option<usize>)>>,
    /// (each module's items by name)
    named: Vec<FxMap<&'a [u8], Vec<u32>>>,
    /// (the imports a lookup passed over as being resolved: a cycle; and whether a lookup met something it does not
    /// know (a glob of another crate's module, a module with macro-made items), for E0432 of a cycle)
    cycles: Vec<(usize, usize)>,
    unknown: bool,
}

/// what the namespaces of an item `private_path` reads are (1 types, 2 values)
fn kind_ns(kind: &str) -> u8 {
    match kind {
        "unit struct" | "tuple struct" => 3,
        "function" | "constant" | "static" => 2,
        _ => 1,
    }
}

impl<'a, 'b> Ambiguity<'a, 'b> {
    fn t(&self, k: usize) -> &'a [u8] {
        let b: &'a [u8] = self.b;
        self.toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e])
    }
    fn parent(&self, m: usize) -> usize {
        self.mods[m].2
    }
    fn within(&self, mut a: usize, b: usize) -> bool {
        loop {
            if a == b {
                return true;
            }
            if a == usize::MAX || a == 0 {
                return false;
            }
            a = self.parent(a);
        }
    }
    fn module_at(&self, segs: &[&[u8]], from: usize) -> Option<usize> {
        let (mut cur, mut s) = match segs.first() {
            Some(&b"crate") => (0, 1),
            Some(&b"self") => (from, 1),
            _ => (from, 0),
        };
        while segs.get(s) == Some(&&b"super"[..]) {
            cur = self.parent(cur);
            if cur == usize::MAX {
                return None;
            }
            s += 1;
        }
        for seg in &segs[s..] {
            cur = self.items[cur].iter().find(|x| x.name == *seg && x.child != usize::MAX)?.child;
        }
        Some(cur)
    }
    fn visible(&self, def: usize, vis: &PVis, from: usize) -> bool {
        match vis {
            PVis::Pub | PVis::Crate => true,
            PVis::Priv => self.within(from, def),
            PVis::Super => match self.parent(def) {
                usize::MAX => self.within(from, def),
                p => self.within(from, p),
            },
            PVis::In(p) => match self.module_at(p, if self.injected { 0 } else { def }) {
                Some(x) => self.within(from, x),
                None => true,
            },
        }
    }
    /// (a definition's own visibility: an item's, an enum's for its variants)
    fn def_visible(&self, d: ADef, from: usize) -> bool {
        self.visible(d.m as usize, &self.items[d.m as usize][d.i as usize].vis, from)
    }
    /// the definitions of `name` in module `cur` that module `viewer` may see, in the namespaces `want` (1 types, 2
    /// values)
    fn lookup(&mut self, cur: usize, name: &'a [u8], viewer: usize, want: u8) -> ADefs {
        self.steps += 1;
        if self.steps > 20_000 {
            return None;
        }
        if self.busy.contains(&(cur, name, viewer)) {
            return Some(vec![]);
        }
        let mut out: Vec<(u8, ADef)> = vec![];
        // (the namespaces not looked for count as taken)
        let mut shadow = !want & 3;
        let own: &[u32] = self.named[cur].get(name).map_or(&[], |v| &v[..]);
        for &i in own {
            let it = &self.items[cur][i as usize];
            if kind_ns(it.kind) & want != 0 {
                let ns = kind_ns(it.kind) & want;
                shadow |= ns;
                if self.visible(cur, &it.vis, viewer) {
                    out.push((ns, ADef { m: cur as u32, i, v: u32::MAX }));
                }
            }
        }
        if shadow == 3 {
            return Some(out);
        }
        self.busy.push((cur, name, viewer));
        let r = self.lookup_imports(cur, name, viewer, shadow, &mut out);
        self.busy.pop();
        r?;
        let mut merged: Vec<(u8, ADef)> = vec![];
        for (ns, d) in out {
            match merged.iter_mut().find(|x| x.1 == d) {
                Some(x) => x.0 |= ns,
                None => merged.push((ns, d)),
            }
        }
        Some(merged)
    }
    fn lookup_imports(&mut self, cur: usize, name: &'a [u8], viewer: usize, mut shadow: u8, out: &mut Vec<(u8, ADef)>) -> Option<()> {
        // (an import of the name: it takes the name before a glob does, in the namespaces it binds)
        let mut bound = 0u8;
        for j in 0..self.imps[cur].len() {
            if self.imps[cur][j].glob == 2 || self.imps[cur][j].name != name {
                continue;
            }
            if self.busy_imp.contains(&(cur, j)) {
                self.cycles.push((cur, j));
                continue;
            }
            let seen = self.visible(cur, &self.imps[cur][j].vis, viewer);
            for (ns, d) in self.resolve_import(cur, j)? {
                let ns = ns & !shadow;
                bound |= ns;
                if ns != 0 && seen {
                    out.push((ns, d));
                }
            }
        }
        shadow |= bound;
        // (a module a macro or an `extern` block may define names in: what its globs bring may be its own)
        if shadow == 3 {
            return Some(());
        }
        if self.opaque[cur] {
            self.unknown = true;
            return None;
        }
        for j in 0..self.imps[cur].len() {
            if self.imps[cur][j].glob != 2 || !self.visible(cur, &self.imps[cur][j].vis, viewer) || self.busy_imp.contains(&(cur, j)) {
                continue;
            }
            let target = match self.targets.get(&(cur, j)) {
                Some(&t) => t,
                None => {
                    self.busy_imp.push((cur, j));
                    let t = self.glob_target(cur, j);
                    self.busy_imp.pop();
                    self.targets.insert((cur, j), t);
                    t
                }
            };
            match target {
                Some((m, None)) => {
                    let Some(defs) = self.lookup(m, name, cur, 3 & !shadow) else { continue };
                    for (ns, d) in defs {
                        let ns = ns & !shadow;
                        if ns != 0 && self.def_visible(d, viewer) {
                            out.push((ns, d));
                        }
                    }
                }
                Some((m, Some(i))) => {
                    let hi = self.mods[m].1;
                    for (v, k) in self.variants_at(self.items[m][i].at, hi).into_iter().enumerate() {
                        if self.t(k) == name && 3 & !shadow != 0 {
                            out.push((3 & !shadow, ADef { m: m as u32, i: i as u32, v: v as u32 }));
                        }
                    }
                }
                None => self.unknown = true,
            }
        }
        Some(())
    }
    /// what import j of module `cur` binds its name to
    fn resolve_import(&mut self, cur: usize, j: usize) -> ADefs {
        self.busy_imp.push((cur, j));
        let segs = self.imps[cur][j].segs.clone();
        let (root, glob) = (self.imps[cur][j].root, self.imps[cur][j].glob);
        let r = self.path(cur, &segs, root, if glob == 1 { 1 } else { 3 });
        self.busy_imp.pop();
        match r {
            APath::Defs(d) if glob == 1 => Some(d.into_iter().filter(|x| self.items[x.1.m as usize][x.1.i as usize].child != usize::MAX && x.1.v == u32::MAX).map(|x| (1, x.1)).collect()),
            APath::Defs(d) => Some(d),
            _ => None,
        }
    }
    /// the module (or the enum, as its module and item) a glob import of module `cur` reads
    fn glob_target(&mut self, cur: usize, j: usize) -> Option<(usize, Option<usize>)> {
        let segs = self.imps[cur][j].segs.clone();
        let root = self.imps[cur][j].root;
        if segs.is_empty() {
            return root.then_some((0, None));
        }
        if segs.len() == 1 {
            match self.t(segs[0]) {
                b"crate" => return Some((0, None)),
                b"self" => return Some((cur, None)),
                b"super" => return (self.parent(cur) != usize::MAX).then(|| (self.parent(cur), None)),
                _ => {}
            }
        }
        if self.t(*segs.last()?) == b"super" || self.t(*segs.last()?) == b"self" {
            return None;
        }
        let APath::Defs(d) = self.path(cur, &segs, root, 1) else { return None };
        let types: Vec<ADef> = d.iter().filter(|x| x.0 & 1 != 0).map(|x| x.1).collect();
        let &[x] = &types[..] else { return None };
        let it = &self.items[x.m as usize][x.i as usize];
        if x.v != u32::MAX {
            return None;
        }
        if it.child != usize::MAX {
            Some((it.child, None))
        } else if it.kind == "enum" {
            Some((x.m as usize, Some(x.i as usize)))
        } else {
            None
        }
    }
    /// what the path of segment tokens `segs` written in module `cur` names (`root`: from the crate root), its last
    /// segment in the namespaces `want`
    fn path(&mut self, cur: usize, segs: &[usize], root: bool, want: u8) -> APath {
        let first = self.t(segs[0]);
        let (mut m, mut s) = match first {
            b"crate" => (0, 1),
            b"self" => (cur, 1),
            b"super" => (cur, 0),
            _ if root => (0, 0),
            _ => (cur, 0),
        };
        while s < segs.len() && self.t(segs[s]) == b"super" {
            m = self.parent(m);
            if m == usize::MAX {
                return APath::Unknown;
            }
            s += 1;
        }
        if s >= segs.len() {
            return APath::Unknown;
        }
        let mut i = s;
        loop {
            let name = self.t(segs[i]);
            if matches!(name, b"self" | b"super" | b"crate") {
                return APath::Unknown;
            }
            let Some(defs) = self.lookup(m, name, cur, if i + 1 == segs.len() { want } else { 1 }) else { return APath::Unknown };
            if i + 1 == segs.len() {
                return APath::Defs(defs);
            }
            let types: Vec<ADef> = defs.iter().filter(|x| x.0 & 1 != 0).map(|x| x.1).collect();
            match &types[..] {
                [] => return APath::Unknown,
                [x] => {
                    let it = &self.items[x.m as usize][x.i as usize];
                    if x.v != u32::MAX {
                        return APath::Unknown;
                    }
                    if it.child != usize::MAX {
                        m = it.child;
                        i += 1;
                        continue;
                    }
                    // (an enum's variant ends the path)
                    if it.kind == "enum" && i + 2 == segs.len() {
                        let last = self.t(segs[i + 1]);
                        let (em, ei) = (x.m as usize, x.i as usize);
                        return match self.variants_at(self.items[em][ei].at, self.mods[em].1).into_iter().position(|k| self.t(k) == last) {
                            Some(v) => APath::Defs(vec![(3, ADef { m: em as u32, i: ei as u32, v: v as u32 })]),
                            None => APath::Unknown,
                        };
                    }
                    return APath::Unknown;
                }
                _ => return APath::Amb(i),
            }
        }
    }
}

/// (of the definitions of a name: whether those of namespace `ns` are more than one, for E0659; `ns` 3: where the
/// path does not tell, each namespace that has one has more than one; 0: any, as an import binds each)
fn ambiguous_in(defs: &[(u8, ADef)], ns: u8) -> bool {
    let count = |bit: u8| defs.iter().filter(|x| x.0 & bit != 0).count();
    let (ty, val) = (count(1), count(2));
    match ns {
        0 => ty > 1 || val > 1,
        1 => ty > 1,
        2 => val > 1,
        _ => (ty > 1 || val > 1) && (ty != 1 && val != 1),
    }
}

/// What the tokens of a module's own text bind, for its E0659 sites: (a name, the tokens where it means something of
/// a block or a binding: an item or a `use` of a block (an empty name: a glob, any name), a generic parameter, a `mut`
/// / `ref` / `@` binding), the tokens that are no path (the variants an enum defines), and the ranges not read (the
/// arguments of a macro other than std's)
struct AScope<'a> {
    covers: Vec<(&'a [u8], usize, usize)>,
    defs: FxSet<usize>,
    skip: Vec<(usize, usize)>,
}

/// std's macros whose arguments are expressions (or patterns) of the caller
const EXPR_MACROS: &[&[u8]] = &[
    b"println", b"print", b"eprintln", b"eprint", b"format", b"format_args", b"write", b"writeln", b"panic", b"assert",
    b"assert_eq", b"assert_ne", b"debug_assert", b"debug_assert_eq", b"debug_assert_ne", b"vec", b"dbg", b"matches",
    b"unreachable", b"todo", b"unimplemented",
];

/// rustc's word for what an import names, in "struct import `S` is private" (`value`: where a path takes a value, a
/// unit struct is one)
fn import_word(kind: &str, variant: bool, value: bool) -> &'static str {
    match kind {
        _ if variant => "variant",
        "unit struct" if value => "unit struct",
        "module" => "module",
        "struct" | "unit struct" | "tuple struct" => "struct",
        "enum" => "enum",
        "trait" => "trait",
        "type alias" => "type alias",
        "union" => "union",
        "function" => "function",
        "constant" => "constant",
        "static" => "static",
        _ => "item",
    }
}

impl<'a, 'b> Ambiguity<'a, 'b> {
    /// E0603: a path written in module m that reaches a name through an import of another module it may not see
    /// (`use crate::b::One;` with a private `use crate::a::One;` in b): "struct import `One` is private", at that
    /// segment. An item the path reaches it may not see is the E0603 check's of items (private_path)
    fn import_privacy(&mut self, m: usize, segs: &[usize], root: bool, value: bool) -> Option<(usize, String)> {
        let first = self.t(segs[0]);
        let (mut cur, mut s) = match first {
            b"crate" => (0, 1),
            b"self" => (m, 1),
            b"super" => (m, 0),
            _ if root => (0, 0),
            _ => (m, 0),
        };
        while s < segs.len() && self.t(segs[s]) == b"super" {
            cur = self.parent(cur);
            if cur == usize::MAX {
                return None;
            }
            s += 1;
        }
        for i in s..segs.len() {
            let name = self.t(segs[i]);
            if matches!(name, b"self" | b"super" | b"crate") {
                return None;
            }
            let own = self.named[cur].contains_key(name);
            let js: Vec<usize> = (0..self.imps[cur].len()).filter(|&j| self.imps[cur][j].glob != 2 && self.imps[cur][j].name == name).collect();
            if !own && !js.is_empty() && !js.iter().any(|&j| self.visible(cur, &self.imps[cur][j].vis, m)) {
                let j = js[0];
                if self.busy_imp.contains(&(cur, j)) {
                    return None;
                }
                let defs = self.resolve_import(cur, j)?;
                let d = defs.iter().find(|x| x.0 & 1 != 0).or(defs.first())?.1;
                let word = import_word(self.items[d.m as usize][d.i as usize].kind, d.v != u32::MAX, value && i + 1 == segs.len());
                return Some((segs[i], format!("{word} import `{}` is private (E0603)", String::from_utf8_lossy(name))));
            }
            if i + 1 == segs.len() {
                return None;
            }
            // (on through the one module the name is here)
            let defs = self.lookup(cur, name, m, 1)?;
            let mods: Vec<usize> = defs.iter().filter(|x| x.1.v == u32::MAX).map(|x| self.items[x.1.m as usize][x.1.i as usize].child).filter(|&c| c != usize::MAX).collect();
            let &[c] = &mods[..] else { return None };
            cur = c;
        }
        None
    }
    /// Whether the tuple struct a path of module m names (its value) has a field m may not see: its constructor is as
    /// visible as that field. Its name, if so
    fn ctor_hidden(&mut self, m: usize, segs: &[usize]) -> Option<&'a [u8]> {
        let APath::Defs(d) = self.path(m, segs, false, 2) else { return None };
        let vals: Vec<ADef> = d.iter().filter(|x| x.0 & 2 != 0 && x.1.v == u32::MAX).map(|x| x.1).collect();
        let &[x] = &vals[..] else { return None };
        let it = &self.items[x.m as usize][x.i as usize];
        if it.kind != "tuple struct" {
            return None;
        }
        let fields = tuple_fields(self.b, self.toks, self.close, it.at)?;
        fields.iter().any(|v| !self.visible(x.m as usize, v, m)).then_some(it.name)
    }
    /// E0432 of an import of module m (import j) that names itself through other imports and nothing else
    /// (`pub use crate::y::x;` in y, `mod a { pub use crate::b::foo; } mod b { pub use crate::a::foo; }`): its path, as
    /// written, is unresolved. Only a path through the program's own modules, where nothing it meets is unknown
    fn import_cycle(&mut self, m: usize, j: usize, segs: &[usize], root: bool) -> Option<(usize, String)> {
        if segs.len() < 2 {
            return None;
        }
        self.cycles.clear();
        self.unknown = false;
        let r = self.path(m, segs, root, 3);
        let APath::Defs(d) = r else { return None };
        if !d.is_empty() || self.unknown || !self.cycles.contains(&(m, j)) {
            return None;
        }
        let p: Vec<String> = segs.iter().map(|&x| String::from_utf8_lossy(self.t(x)).into_owned()).collect();
        Some((segs[0], format!("unresolved import `{}` (E0432)", p.join("::"))))
    }
    /// E0364 / E0365: an import of module m that re-exports (vis wider than private) what it names, the module's own
    /// item (or an enum's variant), where every item of that name is seen in less than the import makes it:
    /// "`f` is only public within the crate, and cannot be re-exported outside" (the item is the crate's), or "`f` is
    /// private, and cannot be re-exported"
    fn reexport(&mut self, m: usize, segs: &[usize], root: bool, vis: &PVis) -> Option<(usize, String)> {
        let last = *segs.last()?;
        let name = self.t(last);
        if matches!(name, b"self" | b"super" | b"crate") {
            return None;
        }
        // (the module of the item, or the enum of the variant)
        let prefix = &segs[..segs.len() - 1];
        let (owner, enum_item) = if segs.len() == 1 {
            (if root { 0 } else { m }, None)
        } else if prefix.iter().all(|&x| matches!(self.t(x), b"crate" | b"self" | b"super")) {
            // (`crate::f`, `super::f`, `self::f`: the module those name)
            let mut cur = if self.t(prefix[0]) == b"crate" { 0 } else { m };
            for &x in prefix {
                match self.t(x) {
                    b"super" => {
                        cur = self.parent(cur);
                        if cur == usize::MAX {
                            return None;
                        }
                    }
                    b"crate" if x != prefix[0] => return None,
                    _ => {}
                }
            }
            (cur, None)
        } else {
            let APath::Defs(d) = self.path(m, &segs[..segs.len() - 1], root, 1) else { return None };
            let types: Vec<ADef> = d.iter().filter(|x| x.0 & 1 != 0 && x.1.v == u32::MAX).map(|x| x.1).collect();
            let &[x] = &types[..] else { return None };
            let it = &self.items[x.m as usize][x.i as usize];
            if it.child != usize::MAX {
                (it.child, None)
            } else if it.kind == "enum" {
                (x.m as usize, Some(x.i as usize))
            } else {
                return None;
            }
        };
        // (the modules each is seen within: None for everywhere, the crate root for the crate)
        let scope = |v: &PVis, at: usize| -> Option<Option<usize>> {
            Some(match v {
                PVis::Pub => None,
                PVis::Crate => Some(0),
                PVis::Priv => Some(at),
                PVis::Super => Some(if at == 0 { 0 } else { self.parent(at) }),
                PVis::In(p) => Some(self.module_at(p, if self.injected { 0 } else { at })?),
            })
        };
        let imp = scope(vis, m)?;
        let mut defs: Vec<(Option<usize>, bool)> = vec![];
        match enum_item {
            Some(ei) => {
                if !self.variants_at(self.items[owner][ei].at, self.mods[owner].1).into_iter().any(|k| self.t(k) == name) {
                    return None;
                }
                defs.push((scope(&self.items[owner][ei].vis, owner)?, false));
            }
            None => {
                for &i in self.named[owner].get(name)? {
                    let it = &self.items[owner][i as usize];
                    defs.push((scope(&it.vis, owner)?, it.child != usize::MAX));
                }
            }
        }
        // (an item seen where the import is: within an import's module, as wide as it)
        let wide_enough = |d: Option<usize>| match (d, imp) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(b), Some(a)) => self.within(a, b),
        };
        if defs.is_empty() || defs.iter().any(|d| wide_enough(d.0)) {
            return None;
        }
        let code = if defs.iter().all(|d| d.1) { "E0365" } else { "E0364" };
        let n = String::from_utf8_lossy(name);
        // (at the leaf's own use tree, as rustc: `crate::f` of `pub use crate::f;`, `V` of `pub use E::{V};`)
        let mut at = last;
        while self.t(at.wrapping_sub(1)) == b":" && self.t(at.wrapping_sub(2)) == b":" && self.is_id(at.wrapping_sub(3)) {
            at -= 3;
        }
        Some((at, if defs.iter().all(|d| d.0 == Some(0)) {
            format!("`{n}` is only public within the crate, and cannot be re-exported outside ({code})")
        } else {
            format!("`{n}` is private, and cannot be re-exported ({code})")
        }))
    }
    /// E0451: a struct literal or pattern written in module m, of the path `segs` and the brace at token `open`, that
    /// names a field it may not see (`a::Foo { x: 1 }`), or takes one by `..base`: "field `x` of struct `Foo` is private"
    /// (`late`: rustc's privacy pass, after the type check, reports it only where that finds no error). A literal
    /// that leaves out such a field and has no `..base` cannot be written at all, as the type check says
    fn literal_privacy(&mut self, m: usize, segs: &[usize], open: usize) -> Option<(usize, String, bool)> {
        let e = *self.close.get(open).filter(|&&e| e != usize::MAX)?;
        let APath::Defs(d) = self.path(m, segs, false, 1) else { return None };
        let types: Vec<ADef> = d.iter().filter(|x| x.0 & 1 != 0).map(|x| x.1).collect();
        let &[x] = &types[..] else { return None };
        if x.v != u32::MAX {
            return None;
        }
        let sm = x.m as usize;
        let it = &self.items[sm][x.i as usize];
        let what = match it.kind {
            "struct" => "struct",
            "union" => "union",
            _ => return None,
        };
        let fields = struct_fields(self.b, self.toks, self.close, it.at, self.mods[sm].1);
        let hidden: Vec<&[u8]> = fields.iter().filter(|f| !self.visible(sm, &f.1, m)).map(|f| f.0).collect();
        if hidden.is_empty() {
            return None;
        }
        let msg = |f: &[u8]| format!("field `{}` of {what} `{}` is private (E0451)", String::from_utf8_lossy(f), String::from_utf8_lossy(it.name));
        let name = it.name;
        // (a pattern: the brace is followed by `=>` or `=` (one token each of `=` and `>`; not `==`), `|`, a guard's
        // `if`, `in` or a parameter's `:`)
        let after = self.t(e + 1);
        let pattern = matches!(after, b"|" | b"if" | b"in" | b":") || after == b"=" && self.t(e + 2) != b"=";
        let mut named: Vec<&[u8]> = vec![];
        let mut first: Option<(usize, String, bool)> = None;
        let mut rest = false;
        let mut k = open + 1;
        while k < e {
            while self.t(k) == b"#" && self.t(k + 1) == b"[" && self.close[k + 1] != usize::MAX {
                k = self.close[k + 1] + 1;
            }
            let s = if self.t(k) == b"r" && self.t(k + 1) == b"#" && self.is_id(k + 2) { k + 2 } else { k };
            if self.is_id(s) && (self.t(s + 1) == b":" && self.t(s + 2) != b":" || matches!(self.t(s + 1), b"," | b"}")) {
                let f = self.t(s);
                named.push(f);
                if hidden.contains(&f) && first.is_none() {
                    first = Some((s, msg(f), true));
                }
            } else if self.t(k) == b"." && self.t(k + 1) == b"." {
                rest = true;
                // (`..base` of a literal takes the fields not named; a pattern's `..` reads none)
                if self.t(k + 2) != b"}" && k + 2 < e && first.is_none() {
                    if let Some(f) = hidden.iter().find(|f| !named.contains(f)) {
                        first = Some((k + 2, msg(f), true));
                    }
                }
                break;
            }
            while k < e && self.t(k) != b"," {
                if matches!(self.t(k), b"(" | b"[" | b"{") && self.close[k] != usize::MAX {
                    k = self.close[k];
                }
                k += 1;
            }
            k += 1;
        }
        // (a literal without `..base` that leaves out a field it may not see; not a union's, which names one)
        if !pattern && !rest && what == "struct" && hidden.iter().any(|f| !named.contains(f)) {
            return Some((segs[0], format!("cannot construct `{}` with struct literal syntax due to private fields", String::from_utf8_lossy(name)), false));
        }
        first
    }
    fn is_id(&self, k: usize) -> bool {
        self.toks.get(k).is_some_and(|x| x.0 == 0)
    }
    /// the tokens of the variant names of the enum whose name is token `at`
    fn variants_at(&self, at: usize, hi: usize) -> Vec<usize> {
        let mut out = vec![];
        let mut k = at + 1;
        while k < hi && self.t(k) != b"{" {
            if matches!(self.t(k), b"(" | b"[" | b";") {
                return out;
            }
            k += 1;
        }
        let Some(&e) = self.close.get(k).filter(|&&e| e != usize::MAX) else { return out };
        k += 1;
        while k < e {
            while self.t(k) == b"#" && self.t(k + 1) == b"[" && self.close[k + 1] != usize::MAX {
                k = self.close[k + 1] + 1;
            }
            if self.is_id(k) {
                out.push(k);
            }
            while k < e && self.t(k) != b"," {
                if matches!(self.t(k), b"(" | b"[" | b"{") && self.close[k] != usize::MAX {
                    k = self.close[k];
                }
                k += 1;
            }
            k += 1;
        }
        out
    }
    /// the end of the item whose generics open at token `lt`: its body's `}` or its `;`
    fn item_end(&self, lt: usize, hi: usize) -> usize {
        let mut d = 0i32;
        let mut k = lt;
        while k < hi {
            match self.t(k) {
                b"<" => d += 1,
                b">" if self.t(k.wrapping_sub(1)) != b"-" => {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                b"(" | b"[" if self.close[k] != usize::MAX => k = self.close[k],
                _ => {}
            }
            k += 1;
        }
        while k < hi {
            match self.t(k) {
                b"{" => return if self.close[k] == usize::MAX { hi } else { self.close[k] },
                b";" => return k,
                b"(" | b"[" if self.close[k] != usize::MAX => k = self.close[k],
                _ => {}
            }
            k += 1;
        }
        hi
    }
    /// the scope facts of module m's own tokens lo..hi, past `jumps` (its child modules and item-level macros)
    fn scope(&self, lo: usize, hi: usize, jumps: &[(usize, usize)], names: &FxSet<&[u8]>) -> AScope<'a> {
        let mut sc = AScope { covers: vec![], defs: FxSet::default(), skip: vec![] };
        let mut braces: Vec<usize> = vec![];
        let mut ji = 0;
        let mut k = lo;
        while k < hi {
            while ji < jumps.len() && jumps[ji].1 < k {
                ji += 1;
            }
            if ji < jumps.len() && jumps[ji].0 <= k {
                k = jumps[ji].1 + 1;
                continue;
            }
            let block_end = |braces: &Vec<usize>| braces.last().map_or(hi, |&o| self.close[o].min(hi));
            let tk = self.t(k);
            match tk {
                b"#" if self.t(k + 1) == b"[" || self.t(k + 1) == b"!" && self.t(k + 2) == b"[" => {
                    let o = if self.t(k + 1) == b"[" { k + 1 } else { k + 2 };
                    k = self.close[o].saturating_add(1);
                    continue;
                }
                b"{" => braces.push(k),
                b"}" => {
                    braces.pop();
                }
                b"fn" | b"struct" | b"enum" | b"trait" | b"type" | b"union" | b"const" | b"static" | b"mod" if !braces.is_empty() => {
                    let at = if self.t(k + 1) == b"mut" { k + 2 } else { k + 1 };
                    if self.is_id(at) && names.contains(self.t(at)) {
                        let o = *braces.last().unwrap();
                        sc.covers.push((self.t(at), o, block_end(&braces)));
                    }
                    if tk == b"enum" && self.is_id(at) {
                        sc.defs.extend(self.variants_at(at, hi));
                    }
                }
                b"use" if !braces.is_empty() && !matches!(self.t(k.wrapping_sub(1)), b":" | b".") => {
                    let o = *braces.last().unwrap();
                    let (mut prefix, mut leaves, mut kinds) = (vec![], vec![], vec![]);
                    let j = if self.t(k + 1) == b":" && self.t(k + 2) == b":" { k + 3 } else { k + 1 };
                    use_leaves(self.b, self.toks, self.close, j, &mut prefix, &mut leaves, &mut kinds);
                    for (l, &g) in leaves.iter().zip(&kinds) {
                        let name = match (g, l.last()) {
                            (2, _) => &b""[..],
                            (_, Some(&x)) if self.t(x + 1) == b"as" => self.t(x + 2),
                            (_, Some(&x)) => self.t(x),
                            _ => continue,
                        };
                        sc.covers.push((name, o, block_end(&braces)));
                    }
                }
                b"mut" | b"ref" if self.is_id(k + 1) && !matches!(self.t(k.wrapping_sub(1)), b"&" | b"*") => {
                    let at = if self.t(k + 1) == b"mut" { k + 2 } else { k + 1 };
                    if names.contains(self.t(at)) {
                        sc.covers.push((self.t(at), k, block_end(&braces)));
                    }
                }
                b"@" if self.is_id(k.wrapping_sub(1)) && names.contains(self.t(k - 1)) => {
                    sc.covers.push((self.t(k - 1), k - 1, block_end(&braces)));
                }
                b"<" if self.t(k.wrapping_sub(1)) == b"impl"
                    || self.is_id(k.wrapping_sub(1)) && matches!(self.t(k.wrapping_sub(2)), b"fn" | b"struct" | b"enum" | b"trait" | b"type" | b"union") =>
                {
                    let end = self.item_end(k, hi);
                    let mut j = k + 1;
                    let mut d = 0i32;
                    let mut start = true;
                    while j < hi {
                        match self.t(j) {
                            b"<" | b"(" | b"[" => d += 1,
                            b">" if self.t(j - 1) != b"-" => {
                                if d == 0 {
                                    break;
                                }
                                d -= 1;
                            }
                            b")" | b"]" => d -= 1,
                            b"," if d == 0 => {
                                start = true;
                                j += 1;
                                continue;
                            }
                            b"const" if start => {
                                j += 1;
                                continue;
                            }
                            x if start && self.is_id(j) && names.contains(x) => sc.covers.push((x, k, end)),
                            _ => {}
                        }
                        start = false;
                        j += 1;
                    }
                }
                b"enum" if self.is_id(k + 1) => sc.defs.extend(self.variants_at(k + 1, hi)),
                _ if self.is_id(k) && self.t(k + 1) == b"!" && !EXPR_MACROS.contains(&tk) => {
                    let o = if self.is_id(k + 2) { k + 3 } else { k + 2 };
                    if self.close.get(o).is_some_and(|&e| e != usize::MAX) {
                        sc.skip.push((o, self.close[o]));
                        k = self.close[o] + 1;
                        continue;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        sc
    }
    /// the namespaces the path whose first token is `first` and last `last` is read in where it is written (1 types,
    /// 2 values, 3 where this does not tell)
    fn site_ns(&self, first: usize, last: usize) -> u8 {
        let after = self.t(last + 1);
        let upper = self.t(last).first().is_some_and(u8::is_ascii_uppercase);
        if after == b"(" {
            return 2;
        }
        if after == b"{" && upper || after == b"<" && upper {
            return 1;
        }
        let mut j = first.wrapping_sub(1);
        if self.t(j) == b":" && self.t(j.wrapping_sub(1)) == b":" {
            j = j.wrapping_sub(2);
        }
        let p = self.t(j);
        let pp = self.t(j.wrapping_sub(1));
        match p {
            b"dyn" | b"impl" | b"as" => 1,
            b">" if pp == b"-" => 1,
            b"<" if upper => 1,
            b"for" if self.t(last + 1) != b"in" => 1,
            b":" if pp != b":" => {
                // (a struct literal's field takes a value: the brace that holds it follows a path, not a definition's
                // name)
                let mut d = 0i32;
                let mut o = j;
                while o != usize::MAX {
                    match self.t(o) {
                        b")" | b"]" | b"}" => d += 1,
                        b"(" | b"[" | b"{" if d == 0 => break,
                        b"(" | b"[" | b"{" => d -= 1,
                        _ => {}
                    }
                    o = o.wrapping_sub(1);
                }
                if o != usize::MAX && self.t(o) == b"{" && self.is_id(o.wrapping_sub(1)) && !matches!(self.t(o.wrapping_sub(2)), b"struct" | b"union" | b"enum" | b"{" | b",") {
                    2
                } else {
                    1
                }
            }
            _ => 3,
        }
    }
}

/// E0401: an item written in a fn body (a fn, struct, enum, union, type alias, const, static, trait, impl, module)
/// that names a generic parameter of an item around that fn, or its `Self`: "can't use generic parameters from outer
/// item", "can't use `Self` from outer item", at the first such name. The names the nested item's own generics give
/// are its own. Not in attributes or macro definitions
fn outer_generics(src: &str, toks: &[(u8, usize, usize)]) -> Option<String> {
    let b = src.as_bytes();
    let n = toks.len();
    let t = |k: usize| -> &[u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    // (an identifier; not a lifetime, whose `'` the tokens leave out)
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0 && (x.1 == 0 || b[x.1 - 1] != b'\''));
    let mut close = vec![usize::MAX; n];
    let mut stack: Vec<usize> = vec![];
    for k in 0..n {
        if toks[k].0 == 1 {
            match b[toks[k].1] {
                b'(' | b'[' | b'{' => stack.push(k),
                b')' | b']' | b'}' => {
                    if let Some(o) = stack.pop() {
                        close[o] = k;
                    }
                }
                _ => {}
            }
        }
    }
    // (the names a `<..>` of generics from token lt declares, and the token past its `>`)
    let generics = |lt: usize| -> (Vec<&[u8]>, usize) {
        let mut out = vec![];
        let (mut k, mut d, mut start) = (lt + 1, 0i32, true);
        while k < n {
            match t(k) {
                b"<" | b"(" | b"[" => d += 1,
                b">" if t(k - 1) != b"-" => {
                    if d == 0 {
                        return (out, k + 1);
                    }
                    d -= 1;
                }
                b")" | b"]" => d -= 1,
                b"," if d == 0 => {
                    start = true;
                    k += 1;
                    continue;
                }
                b"const" if start => {
                    k += 1;
                    continue;
                }
                x if start && is_id(k) && t(k.wrapping_sub(1)) != b"'" => out.push(x),
                _ => {}
            }
            start = false;
            k += 1;
        }
        (out, n)
    };
    struct Ctx<'s> {
        visible: Vec<&'s [u8]>,
        beyond: Vec<&'s [u8]>,
        self_ok: bool,
        self_beyond: bool,
        /// (the `Self` beyond is a generic impl's or a trait's: its constructor is an error too, not a warning)
        self_generic: bool,
    }
    // (what context a range of tokens is: an item's header or a type's body (0), a fn body (1), an impl's or trait's
    // body (2))
    fn walk<'s>(
        lo: usize,
        hi: usize,
        cx: &Ctx<'s>,
        body: u8,
        t: &dyn Fn(usize) -> &'s [u8],
        is_id: &dyn Fn(usize) -> bool,
        close: &[usize],
        generics: &dyn Fn(usize) -> (Vec<&'s [u8]>, usize),
    ) -> Option<(usize, &'static str)> {
        let mut k = lo;
        while k < hi {
            let tk = t(k);
            if tk == b"#" && (t(k + 1) == b"[" || t(k + 1) == b"!" && t(k + 2) == b"[") {
                let o = if t(k + 1) == b"[" { k + 1 } else { k + 2 };
                k = close[o].saturating_add(1);
                continue;
            }
            if tk == b"macro_rules" && t(k + 1) == b"!" {
                let o = if is_id(k + 2) { k + 3 } else { k + 2 };
                k = close.get(o).map_or(hi, |&e| e.saturating_add(1));
                continue;
            }
            // (an item: its kind, the token of its generics' `<`)
            let prev = t(k.wrapping_sub(1));
            let item = match tk {
                b"fn" if is_id(k + 1) => Some("fn"),
                b"struct" | b"enum" | b"union" | b"trait" if is_id(k + 1) => Some("adt"),
                b"impl" if matches!(prev, b"}" | b";" | b"{" | b"]" | b"unsafe" | b"") => Some("impl"),
                b"type" if is_id(k + 1) && body != 2 => Some("adt"),
                b"const" | b"static" if body == 1 && is_id(k + 1) && t(k + 2) == b":" || body == 1 && tk == b"static" && t(k + 1) == b"mut" => Some("value"),
                b"mod" if is_id(k + 1) && t(k + 2) == b"{" => Some("mod"),
                _ => None,
            };
            if let Some(kind) = item {
                let lt = if kind == "impl" { k + 1 } else { k + 2 };
                let (own, mut j) = if t(lt) == b"<" { generics(lt) } else { (vec![], lt) };
                // (the item's extent: its body's brace, or its `;`)
                let mut open = usize::MAX;
                while j < hi {
                    match t(j) {
                        b"{" => {
                            open = j;
                            break;
                        }
                        b";" => break,
                        b"(" | b"[" if close[j] != usize::MAX => j = close[j],
                        _ => {}
                    }
                    j += 1;
                }
                let end = if open != usize::MAX { close[open].min(hi) } else { j.min(hi) };
                // (a barrier where the item is in a fn body: what was visible is beyond it)
                let generic_self = tk == b"trait" || kind == "impl" && !own.is_empty();
                let ncx = if body == 1 {
                    let mut beyond = cx.beyond.clone();
                    beyond.extend(cx.visible.iter().copied());
                    let self_ok = kind == "impl" || tk == b"trait";
                    Ctx { visible: own.clone(), beyond, self_ok, self_beyond: cx.self_beyond || cx.self_ok, self_generic: if self_ok { generic_self } else { cx.self_generic } }
                } else {
                    let mut visible = cx.visible.clone();
                    visible.extend(own.iter().copied());
                    let opens = kind == "impl" || tk == b"trait";
                    Ctx { visible, beyond: cx.beyond.clone(), self_ok: cx.self_ok || opens, self_beyond: cx.self_beyond, self_generic: if opens { generic_self } else { cx.self_generic } }
                };
                // (the header, then the body)
                let head_end = if open != usize::MAX { open } else { end };
                if let Some(e) = walk(k + 1, head_end, &ncx, 0, t, is_id, close, generics) {
                    return Some(e);
                }
                if open != usize::MAX {
                    let inner = match kind {
                        "fn" => 1,
                        "impl" => 2,
                        _ if tk == b"trait" => 2,
                        "mod" => 3,
                        _ => 0,
                    };
                    let mcx;
                    let bcx = if inner == 3 {
                        mcx = Ctx { visible: vec![], beyond: vec![], self_ok: false, self_beyond: false, self_generic: false };
                        &mcx
                    } else {
                        &ncx
                    };
                    if let Some(e) = walk(open + 1, end, bcx, if inner == 3 { 0 } else { inner }, t, is_id, close, generics) {
                        return Some(e);
                    }
                }
                k = end + 1;
                continue;
            }
            if is_id(k) && !matches!(prev, b"." | b"'") && !(prev == b":" && t(k.wrapping_sub(2)) == b":") {
                if tk == b"Self" && cx.self_beyond && !cx.self_ok {
                    // (`Self(0)` / `Self` as a value: the constructor, which a non-generic impl's only warns of)
                    let value = t(k + 1) == b"(" || matches!(prev, b"=" | b"(" | b"," | b"return" | b"{" | b";" | b">") && t(k + 1) != b"::" && t(k + 1) != b"{" && prev != b">";
                    if !value {
                        return Some((k, "can't use `Self` from outer item (E0401)"));
                    }
                    if cx.self_generic {
                        return Some((k, "can't reference `Self` constructor from outer item (E0401)"));
                    }
                }
                if cx.beyond.contains(&tk) && !cx.visible.contains(&tk) {
                    return Some((k, "can't use generic parameters from outer item (E0401)"));
                }
            }
            k += 1;
        }
        None
    }
    let cx = Ctx { visible: vec![], beyond: vec![], self_ok: false, self_beyond: false, self_generic: false };
    let (at, msg) = walk(0, n, &cx, 0, &t, &is_id, &close, &generics)?;
    let off = toks[at].1;
    let line = b[..off].iter().filter(|&&c| c == b'\n').count() + 1;
    let col = off - src[..off].rfind('\n').map_or(0, |p| p + 1) + 1;
    Some(format!("{line}:{col}: {msg}"))
}

/// A name a module defines, for duplicate_name
struct DupDef<'a> {
    /// the namespaces: 1 types, 2 values (0: an import that names nothing found, not compared)
    ns: u8,
    name: &'a [u8],
    def: Def,
    /// the token the error points at (usize::MAX: the injected crate, written nowhere)
    at: usize,
    /// an import's path (`use a::b as c;`: [a, b])
    path: Vec<&'a [u8]>,
    /// an import `self` of a list (`use a::b::{self}`): the module, not a value of its name
    module_only: bool,
}

/// The names a `use` tree from token k binds: (name, path, the first token of its part of the tree, whether it is a
/// `self` of a list, which names a module only). A glob and `as _` bind none. Returns the token past the tree
fn use_tree_names<'a>(
    b: &'a [u8],
    toks: &[(u8, usize, usize)],
    close: &[usize],
    k: usize,
    prefix: &mut Vec<&'a [u8]>,
    out: &mut Vec<(&'a [u8], Vec<&'a [u8]>, usize, bool)>,
) -> usize {
    let t = |k: usize| -> &'a [u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    let at = k;
    let depth = prefix.len();
    let mut k = k;
    // (a leading `::`)
    if t(k) == b":" && t(k + 1) == b":" {
        k += 2;
    }
    let end = loop {
        if t(k) == b"{" {
            let Some(&e) = close.get(k).filter(|&&e| e != usize::MAX) else { break k };
            let mut j = k + 1;
            while j < e {
                j = use_tree_names(b, toks, close, j, prefix, out);
                if t(j) != b"," {
                    break;
                }
                j += 1;
            }
            break e + 1;
        }
        if !is_id(k) {
            // (a glob, or what this does not read)
            break k + usize::from(t(k) == b"*");
        }
        // (`r#foo` is the name `foo`: the tokens split it into `r`, `#`, `foo`)
        let s = if t(k) == b"r" && t(k + 1) == b"#" && is_id(k + 2) { k + 2 } else { k };
        if t(s + 1) == b":" && t(s + 2) == b":" {
            prefix.push(t(s));
            k = s + 3;
            continue;
        }
        // (`as r#x` binds `x`)
        let (alias, next) = match t(s + 1) == b"as" {
            true if t(s + 2) == b"r" && t(s + 3) == b"#" && is_id(s + 4) => (Some(t(s + 4)), s + 5),
            true => (Some(t(s + 2)), s + 3),
            false => (None, s + 1),
        };
        // (`use a::{self}` binds `a`, the module alone)
        let own = t(s) == b"self";
        let (name, path) = if own {
            (alias.or(prefix.last().copied()), prefix.clone())
        } else {
            let mut p = prefix.clone();
            p.push(t(s));
            (Some(alias.unwrap_or(t(s))), p)
        };
        if let Some(nm) = name.filter(|&nm| nm != b"_" && !path.is_empty()) {
            out.push((nm, path, at, own));
        }
        break next;
    };
    prefix.truncate(depth);
    end
}

/// The namespaces of what an import of std / core / alloc names, where its path tells (0: unknown): the crate, one of
/// its modules, or a type or trait (`std::collections::HashMap`)
fn std_path_ns(path: &[&[u8]]) -> u8 {
    const MODS: &[&str] = &[
        "alloc", "any", "array", "ascii", "borrow", "boxed", "cell", "char", "clone", "cmp", "collections", "convert", "default",
        "env", "error", "f32", "f64", "ffi", "fmt", "fs", "future", "hash", "hint", "i8", "i16", "i32", "i64", "i128", "io",
        "isize", "iter", "marker", "mem", "net", "num", "ops", "option", "os", "panic", "path", "pin", "prelude", "primitive",
        "process", "ptr", "rc", "result", "slice", "str", "string", "sync", "task", "thread", "time", "u8", "u16", "u32", "u64",
        "u128", "usize", "vec",
    ];
    let last = path[path.len() - 1];
    if path.len() == 1 || (path.len() == 2 && MODS.iter().any(|m| m.as_bytes() == last)) {
        1
    } else if last.first().is_some_and(u8::is_ascii_uppercase) && last.iter().any(u8::is_ascii_lowercase) {
        1
    } else {
        0
    }
}

/// E0428 and the like: two definitions of one name in one namespace of a module (the crate root or a `mod m { .. }`),
/// at the second (`enum Foo {}` and `mod Foo {}`). The definitions: items (types: `mod` / `struct` / `enum` / `union`
/// / `trait` / `type` / `extern crate`; values: `fn` / `const` / `static` / a unit or tuple struct), imports of a module
/// or item of the program or of a module or type of std, and the `extern crate std` the compiler puts in the crate
/// root in edition 2015 (`injected`; a later edition's is hygienic). Only read when the program has a `mod` or an
/// `extern crate`, or in edition 2015 something that may define `std` / `core`. `src` is the configured text, `externs` the `extern crate` items
/// the configuring blanked (cfg::strip_noting)
/// improper_ctypes: the first `CStr` / `CString` in the parameters or result of a fn of an `extern "C" { .. }` block,
/// or of an `extern "C" fn` (improper_ctypes_definitions), as `line:col: msg`. Behind a pointer too: neither has a C
/// layout
/// text names `lint` in an `allow(..)` (`#[allow(a, improper_ctypes)]`, `#![allow(..)]`): that lint, not one it is
/// a prefix of
fn allows_lint(text: &str, lint: &str) -> bool {
    let sq: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    sq.match_indices("allow(").any(|(i, _)| {
        let rest = &sq[i + 6..];
        rest[..rest.find(')').unwrap_or(rest.len())].split(',').any(|n| n == lint)
    })
}

fn first_cstr_in_extern(src: &str, items: &[SrcItem], (blocks, defs): (bool, bool)) -> Option<String> {
    for it in items {
        let text = src.get(it.start..it.end).unwrap_or("");
        if !text.contains("extern") || !(text.contains("CStr")) {
            continue;
        }
        let b = text.as_bytes();
        let toks = crate::lexer::tokenize(b);
        let tk = |k: usize| &text[toks[k].1..toks[k].2];
        let sym = |k: usize, c: &str| k < toks.len() && toks[k].0 == 1 && tk(k) == c;
        // (a fn's signature from `fn` to its `;` or body: the first C string type in it)
        let sig_cstr = |k: usize| -> Option<&str> {
            let mut j = k;
            while j < toks.len() && !sym(j, ";") && !sym(j, "{") {
                if toks[j].0 == 0 && matches!(tk(j), "CStr" | "CString") {
                    return Some(tk(j));
                }
                j += 1;
            }
            None
        };
        let line_of = |k: usize| it.line as usize + b[..toks[k].1].iter().filter(|&&c| c == b'\n').count();
        // (the attributes of the item or declaration token k starts: back to the `;` / `{` / `}` before them)
        let attrs_allow = |k: usize, lint: &str| {
            let mut a = k;
            while a > 0 && !(sym(a - 1, ";") || sym(a - 1, "{") || sym(a - 1, "}")) {
                a -= 1;
            }
            let from = if a < toks.len() { toks[a].1 } else { 0 };
            allows_lint(&text[from..toks[k].1], lint)
        };
        let mut k = 0;
        while k + 2 < toks.len() {
            if toks[k].0 == 0 && tk(k) == "extern" && toks[k + 1].0 == 2 && tk(k + 1) == "\"C\"" {
                if sym(k + 2, "{") {
                    // (`#[allow(improper_ctypes)]` on the block: none of its fns; on one fn: that one)
                    if !blocks || attrs_allow(k, "improper_ctypes") {
                        k += 3;
                        continue;
                    }
                    let mut j = k + 3;
                    let mut d = 1;
                    while j < toks.len() && d > 0 {
                        if sym(j, "{") {
                            d += 1;
                        } else if sym(j, "}") {
                            d -= 1;
                        } else if d == 1 && toks[j].0 == 0 && tk(j) == "fn" && !attrs_allow(j, "improper_ctypes") {
                            if let Some(t) = sig_cstr(j) {
                                return Some(format!("{}:1: `extern` block uses type `{t}`, which is not FFI-safe", line_of(j)));
                            }
                        }
                        j += 1;
                    }
                    k = j;
                    continue;
                }
                if defs && toks[k + 2].0 == 0 && tk(k + 2) == "fn" && !attrs_allow(k, "improper_ctypes_definitions") {
                    if let Some(t) = sig_cstr(k + 2) {
                        return Some(format!("{}:1: `extern` fn uses type `{t}`, which is not FFI-safe", line_of(k)));
                    }
                }
            }
            k += 1;
        }
    }
    None
}

/// A reserved word, which no item may be named (`mod break`, `struct for`)
fn reserved_word(w: &[u8]) -> bool {
    matches!(
        w,
        b"as" | b"break" | b"const" | b"continue" | b"crate" | b"else" | b"enum" | b"extern" | b"false" | b"fn" | b"for" | b"if" | b"impl" | b"in"
            | b"let" | b"loop" | b"match" | b"mod" | b"move" | b"mut" | b"pub" | b"ref" | b"return" | b"self" | b"Self" | b"static" | b"struct"
            | b"super" | b"trait" | b"true" | b"type" | b"unsafe" | b"use" | b"where" | b"while" | b"abstract" | b"become" | b"box" | b"do"
            | b"final" | b"macro" | b"override" | b"priv" | b"typeof" | b"unsized" | b"virtual" | b"yield"
    )
}

fn duplicate_name(src: &str, src_toks: Option<&[(u8, usize, usize)]>, externs: &[(usize, String)], injected: bool) -> Option<String> {
    // (a few scans: a program without these has its other duplicates found when its items are collected)
    // (2015's injected `std` meets an item or import of that name: `struct std;`, `use x as std;`, `enum std {`)
    let names_std = || {
        ["std", "core"].iter().any(|w| {
            src.match_indices(w).any(|(i, _)| src[i + w.len()..].trim_start().starts_with([';', '{', '=', '<', '(']))
        })
    };
    if externs.is_empty() && !src.contains("mod ") && !(injected && names_std()) {
        return None;
    }
    // (the blanked `extern crate x;` items back in their places, where those are still blank)
    let restored;
    let src = if externs.is_empty() {
        src
    } else {
        let mut out = src.as_bytes().to_vec();
        for (p, text) in externs {
            if let Some(at) = out.get_mut(*p..*p + text.len()).filter(|at| at.iter().all(u8::is_ascii_whitespace)) {
                at.copy_from_slice(text.as_bytes());
            }
        }
        restored = String::from_utf8(out).ok()?;
        restored.as_str()
    };
    let b = src.as_bytes();
    // (the configured text's tokens where the caller has them and no `extern crate` was put back)
    let own;
    let toks: &[(u8, usize, usize)] = match src_toks {
        Some(t) if externs.is_empty() => t,
        _ => {
            own = crate::lexer::tokenize(b);
            &own
        }
    };
    let n = toks.len();
    let t = |k: usize| -> &[u8] { toks.get(k).map_or(&b""[..], |&(_, a, e)| &b[a..e]) };
    // (each open bracket's match)
    let mut close = vec![usize::MAX; n];
    let mut stack: Vec<usize> = vec![];
    for k in 0..n {
        if toks[k].0 == 1 {
            match b[toks[k].1] {
                b'(' | b'[' | b'{' => stack.push(k),
                b')' | b']' | b'}' => {
                    if let Some(o) = stack.pop() {
                        close[o] = k;
                    }
                }
                _ => {}
            }
        }
    }
    let line_of = |k: usize| b[..toks[k].1].iter().filter(|&&c| c == b'\n').count() + 1;
    let is_id = |k: usize| toks.get(k).is_some_and(|x| x.0 == 0);
    // (`r#foo` is the name `foo`: the tokens split it into `r`, `#`, `foo`)
    let raw = |k: usize| if t(k) == b"r" && t(k + 1) == b"#" && is_id(k + 2) { k + 2 } else { k };
    // (modules: (the tokens of the body, the parent, the name); the crate root first)
    let mut mods: Vec<(usize, usize, usize, &[u8])> = vec![(0, n, usize::MAX, b"")];
    let mut defs: Vec<Vec<DupDef>> = vec![];
    let mut uses: Vec<(&[u8], Vec<&[u8]>, usize, bool)> = vec![];
    let mut prefix: Vec<&[u8]> = vec![];
    while defs.len() < mods.len() {
        let m = defs.len();
        let (lo, hi, _, _) = mods[m];
        let mut names: Vec<DupDef> = vec![];
        // (the crate's own attributes `#![no_std]` / `#![no_core]`: the inner attributes the text starts with, from the
        // tokens (not a comment, nor a macro's input))
        let inner = |a: &[u8]| {
            let mut k = 0;
            while t(k) == b"#" && t(k + 1) == b"!" && t(k + 2) == b"[" && close[k + 2] != usize::MAX {
                if t(k + 3) == a && t(k + 4) == b"]" {
                    return true;
                }
                k = close[k + 2] + 1;
            }
            false
        };
        if m == 0 && injected && !inner(b"no_core") {
            let name: &[u8] = if inner(b"no_std") { b"core" } else { b"std" };
            names.push(DupDef { ns: 1, name, def: Def::Injected, at: usize::MAX, path: vec![], module_only: false });
        }
        let mut k = lo;
        // (the item's first token past its attributes: `pub` included)
        let mut start = usize::MAX;
        while k < hi {
            // attributes
            if t(k) == b"#" && (t(k + 1) == b"[" || (t(k + 1) == b"!" && t(k + 2) == b"[")) {
                let o = if t(k + 1) == b"[" { k + 1 } else { k + 2 };
                k = close[o].saturating_add(1);
                continue;
            }
            if start == usize::MAX {
                start = k;
            }
            if t(k) == b"pub" {
                k += 1;
                if t(k) == b"(" {
                    k = close[k].saturating_add(1);
                }
                continue;
            }
            if matches!(t(k), b"unsafe" | b"async" | b"default") {
                k += 1;
                continue;
            }
            // (`const fn` / `const unsafe fn` / `extern "C" fn`: qualifiers of a fn)
            if t(k) == b"const" && matches!(t(k + 1), b"fn" | b"unsafe" | b"extern" | b"async") {
                k += 1;
                continue;
            }
            if t(k) == b"extern" && toks.get(k + 1).is_some_and(|x| x.0 == 2) && matches!(t(k + 2), b"fn" | b"unsafe") {
                k += 2;
                continue;
            }
            // (the item's namespaces and name: 1 types, 2 values)
            let mut item: Option<(u8, usize, Def)> = None;
            match t(k) {
                // (an inline module; a `mod m;` file may be configured away by its own `#![cfg(..)]`)
                // (nor one whose body starts with an inner `#![cfg(..)]`, nor an empty one over several lines: a file module
                // the loader brought in, configured away by its own `#![cfg(..)]`)
                // (`mod break {`: a keyword where the module's name goes, rustc's parse error)
                b"mod" if is_id(k + 1) && reserved_word(t(k + 1)) => {
                    return Some(format!("{}:{}: expected identifier, found keyword `{}`", line_of(k + 1), toks[k + 1].1 - b[..toks[k + 1].1].iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1) + 1, String::from_utf8_lossy(t(k + 1))));
                }
                b"mod" if is_id(k + 1)
                    && t(k + 2) == b"{"
                    && !(t(k + 3) == b"#" && t(k + 4) == b"!" && t(k + 5) == b"[" && t(k + 6).starts_with(b"cfg"))
                    && !(close[k + 2] == k + 3 && b[toks[k + 2].1..toks[k + 3].1].contains(&b'\n')) =>
                {
                    item = Some((1, k + 1, Def::Item));
                    if close[k + 2] < hi {
                        mods.push((k + 3, close[k + 2], m, t(raw(k + 1))));
                    }
                }
                b"struct" if is_id(k + 1) => {
                    // (a unit / tuple struct is a value too)
                    let mut j = k + 2;
                    if t(j) == b"<" {
                        let mut d = 0i32;
                        while j < hi {
                            match t(j) {
                                b"<" => d += 1,
                                b">" => {
                                    d -= 1;
                                    if d == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        j += 1;
                    }
                    item = Some((if matches!(t(j), b";" | b"(") { 3 } else { 1 }, k + 1, Def::Item));
                }
                b"enum" | b"trait" | b"type" if is_id(k + 1) => item = Some((1, k + 1, Def::Item)),
                b"union" if is_id(k + 1) && t(k + 1) != b"union" => item = Some((1, k + 1, Def::Item)),
                b"fn" | b"const" | b"static" if is_id(k + 1) && t(k + 1) != b"_" && t(k + 1) != b"fn" && t(k + 1) != b"mut" => {
                    item = Some((2, k + 1, Def::Item))
                }
                b"static" if t(k + 1) == b"mut" && is_id(k + 2) => item = Some((2, k + 2, Def::Item)),
                b"extern" if t(k + 1) == b"crate" && is_id(k + 2) => {
                    let nm = if t(k + 3) == b"as" && (is_id(k + 4) || t(k + 4) == b"_") { k + 4 } else { k + 2 };
                    // (`as _` binds no name)
                    if t(nm) != b"self" && t(nm) != b"_" {
                        item = Some((1, nm, Def::Crate));
                    }
                }
                b"use" => {
                    uses.clear();
                    use_tree_names(b, &toks, &close, k + 1, &mut prefix, &mut uses);
                    for (name, path, at, own) in uses.drain(..) {
                        names.push(DupDef { ns: 0, name, def: Def::Use, at, path, module_only: own });
                    }
                }
                _ => {}
            }
            if let Some((ns, at, def)) = item {
                names.push(DupDef { ns, name: t(raw(at)), def, at: start, path: vec![], module_only: false });
            }
            start = usize::MAX;
            // to the item's end: a `;` or a body `{ .. }` (a body in brackets is skipped whole)
            let mut j = if item.is_some() { k + 1 } else { k };
            loop {
                if j >= hi {
                    break;
                }
                match t(j) {
                    b";" => break,
                    b"{" => {
                        j = close[j];
                        // (`struct S { .. }` / `fn f() { .. }` end at their body; `const C: T = S { .. };` goes on)
                        if t(j + 1) != b";" && !matches!(t(j + 1), b"." | b"?" | b"+" | b"-" | b"*" | b"/") {
                            break;
                        }
                    }
                    b"(" | b"[" => j = close[j],
                    _ => {}
                }
                if j == usize::MAX {
                    return None;
                }
                j += 1;
            }
            k = j.saturating_add(1);
        }
        defs.push(names);
    }
    // (an import's namespaces: what it names in a module of the program, std's, or an `extern crate`)
    // (each module's child modules by name; the first of a name, as a lookup in order finds)
    let mut kids: FxMap<(usize, &[u8]), usize> = FxMap::default();
    for (i, x) in mods.iter().enumerate() {
        kids.entry((x.2, x.3)).or_insert(i);
    }
    let child = |m: usize, nm: &[u8]| kids.get(&(m, nm)).copied();
    let mut resolved: Vec<(usize, usize, u8)> = vec![];
    for (m, ds) in defs.iter().enumerate() {
        for (i, d) in ds.iter().enumerate() {
            if d.def != Def::Use {
                continue;
            }
            let path = &d.path;
            let mut cur = m;
            let mut s = 0;
            match path[0] {
                b"crate" => {
                    cur = 0;
                    s = 1;
                }
                b"self" => s = 1,
                b"super" => {
                    while path.get(s).is_some_and(|&x| x == b"super") {
                        cur = mods[cur].2;
                        if cur == usize::MAX {
                            break;
                        }
                        s += 1;
                    }
                }
                // (a path of neither: from the crate root in 2015, from this module later)
                b"std" | b"core" | b"alloc" if child(if injected { 0 } else { m }, path[0]).is_none() => {
                    resolved.push((m, i, std_path_ns(path) & if d.module_only { 1 } else { 3 }));
                    continue;
                }
                _ if injected => cur = 0,
                _ => {}
            }
            if cur == usize::MAX || s >= path.len() {
                continue;
            }
            for seg in &path[s..path.len() - 1] {
                match child(cur, seg) {
                    Some(c) => cur = c,
                    None => {
                        cur = usize::MAX;
                        break;
                    }
                }
            }
            if cur == usize::MAX {
                continue;
            }
            let last = path[path.len() - 1];
            let mut ns = defs[cur].iter().filter(|x| x.name == last && x.def != Def::Use && x.at != usize::MAX).fold(0, |a, x| a | x.ns);
            if child(cur, last).is_some() {
                ns |= 1;
            }
            resolved.push((m, i, if d.module_only { ns & 1 } else { ns }));
        }
    }
    for (m, i, ns) in resolved {
        defs[m][i].ns = ns;
    }
    // (in each module, the definitions of one name in their order: by name, then by place)
    let mut first: Option<(usize, usize, &[u8], &str)> = None;
    let mut order: Vec<usize> = vec![];
    for ds in &defs {
        order.clear();
        order.extend((0..ds.len()).filter(|&i| ds[i].ns != 0));
        order.sort_by(|&x, &y| ds[x].name.cmp(ds[y].name).then(x.cmp(&y)));
        for (oi, &i) in order.iter().enumerate() {
            let e = &ds[i];
            let prev = order[..oi].iter().rev().take_while(|&&p| ds[p].name == e.name).map(|&p| &ds[p]).find(|p| {
                p.ns & e.ns != 0
                    // (`use std;` names the crate the root already has)
                    && !(p.def == Def::Injected && e.def == Def::Use && e.path.len() == 1 && e.path[0] == e.name)
            });
            let Some(p) = prev else { continue };
            let code = match (p.def, e.def) {
                (Def::Item, Def::Item) => "E0428",
                (Def::Use, Def::Use) => "E0252",
                (Def::Use, Def::Item) | (Def::Item, Def::Use) => "E0255",
                (Def::Use, _) | (_, Def::Use) => "E0254",
                (Def::Item, _) | (_, Def::Item) => "E0260",
                _ => "E0259",
            };
            let line = line_of(e.at);
            if first.is_none_or(|(l, _, _, _)| line < l) {
                // (the column: the item's first token, an import's path)
                let p = toks[e.at].1;
                let col = p - b[..p].iter().rposition(|&c| c == b'\n').map_or(0, |l| l + 1) + 1;
                first = Some((line, col, e.name, code));
            }
        }
    }
    first.map(|(line, col, name, code)| format!("{line}:{col}: the name `{}` is defined multiple times ({code})", String::from_utf8_lossy(name)))
}

/// E0252 / E0255: a name a top-level `use` brings in that another top-level `use` or item of the crate root also
/// defines ("the name `X` is defined multiple times", at the second). Top level = the item starts a line (the
/// flattened modules' items keep their indentation)
fn duplicate_import(src0: &str, split: &str, items: &[SrcItem]) -> Option<String> {
    // (no crate-root `use`, no clash to look for: the common case costs one look at each item's first bytes)
    if !items.iter().any(|it| {
        matches!(it.kind, ItemKind::Normal)
            && (it.start == 0 || split.as_bytes()[it.start - 1] == b'\n')
            && {
                // (past the item's attributes, which only a few items have)
                let mut t = &split[it.start..it.end];
                while t.starts_with("#[") {
                    match t.find(']') {
                        Some(e) => t = t[e + 1..].trim_start(),
                        None => break,
                    }
                }
                t.starts_with("use ") || t.starts_with("pub use ")
            }
    }) {
        return None;
    }
    // (name → (some definition is a `use`, some definition is an item, a line): a hash, as a crate has thousands of items)
    let mut seen: FxMap<&str, (bool, bool, usize)> = FxMap::default();
    // (the original file's last line: items past it were hoisted out of fn bodies, and are their blocks')
    let mut total: Option<usize> = None;
    // (reused for each item: no allocation per item)
    let mut names: Vec<(&str, bool)> = vec![];
    for it in items {
        if !matches!(it.kind, ItemKind::Normal) || !(it.start == 0 || split.as_bytes()[it.start - 1] == b'\n') {
            continue;
        }
        let text = &split[it.start..it.end];
        if text.starts_with([' ', '\t']) || text.starts_with("#!") || (text.starts_with('#') && text.contains("#[cfg")) {
            continue;
        }
        // (past the item's attributes: `#[allow(unused_imports)]`)
        let mut t = text;
        while t.starts_with("#[") {
            let Some(e) = t.find(']') else { break };
            t = t[e + 1..].trim_start();
        }
        let line_shift = text[..text.len() - t.len()].matches('\n').count();
        let t = t.strip_prefix("pub ").unwrap_or(t);
        names.clear();
        if let Some(rest) = t.strip_prefix("use ") {
            let body = rest.trim_end().trim_end_matches(';').trim();
            let (prefix, list): (&str, Vec<&str>) = match body.find('{') {
                Some(i) if body.ends_with('}') && !body[i + 1..].contains('{') => (&body[..i], body[i + 1..body.len() - 1].split(',').map(str::trim).filter(|x| !x.is_empty()).collect()),
                Some(_) => continue,
                None => ("", vec![body]),
            };
            for e in list {
                let (path, alias) = match e.split_once(" as ") {
                    Some((p, a)) => (p.trim(), Some(a.trim())),
                    None => (e.trim(), None),
                };
                // (`use a::{self}` names `a`)
                let last = if path == "self" { prefix.trim_end_matches("::").rsplit("::").next().unwrap_or("") } else { path.rsplit("::").next().unwrap_or(path) };
                let n = alias.unwrap_or(last);
                if !n.is_empty() && n != "_" && n != "*" && !path.ends_with('*') {
                    names.push((n, true));
                }
            }
        } else {
            let mut ws = t.split_whitespace();
            if let (Some(kw), Some(name)) = (ws.next(), ws.next()) {
                if matches!(kw, "struct" | "enum" | "trait" | "type" | "union" | "fn" | "const" | "static") {
                    let end = name.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(name.len());
                    // (a user type renamed off a prelude type's name is still that name)
                    let n = &name[..end];
                    let n = n.strip_prefix("__u_").unwrap_or(n);
                    if !n.is_empty() && !n.contains("__") {
                        names.push((n, false));
                    }
                }
            }
        }
        let line = it.line + line_shift;
        for &(n, is_use) in &names {
            // (two items of one name are the build's own duplicate check; here at least one is a `use`)
            if let Some(&(any_use, any_item, l0)) = seen.get(&n) {
                if (any_use || is_use) && (any_use || any_item) {
                    let last = *total.get_or_insert_with(|| src0.bytes().filter(|&c| c == b'\n').count() + 1);
                    if line <= last && l0 <= last {
                        return Some(format!("{line}:1: the name `{n}` is defined multiple times"));
                    }
                }
            }
            let e = seen.entry(n).or_insert((false, false, line));
            if is_use {
                e.0 = true;
            } else {
                e.1 = true;
            }
        }
    }
    None
}

/// A top-level `use a::..` (at the start of a line) whose first segment is no crate the build knows (std, core,
/// a dependency, an `extern crate`), no module or item of the program, nor `crate` / `self` / `super`:
/// "unresolved import `a`" (E0432). Only read when such a `use` is there at all
fn unresolved_import(src: &str, split: &str, items: &[SrcItem], deps: &[String]) -> Option<String> {
    // (and the names std's prelude brings: `use Default::default` / `use Option::*`)
    let known = |n: &str| {
        matches!(
            n,
            "std" | "core" | "crate" | "self" | "super" | "" | "proc_macro" | "Default" | "Option" | "Result" | "Some" | "None" | "Ok" | "Err"
                | "Vec" | "String" | "Box" | "Iterator" | "IntoIterator" | "ToString" | "From" | "Into" | "Clone" | "Copy" | "Drop" | "Fn" | "FnMut"
                | "FnOnce" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "AsRef" | "AsMut" | "Send" | "Sync" | "Sized" | "ToOwned" | "Extend"
        ) || deps.iter().any(|d| d == n)
    };
    let mut firsts: Vec<(usize, String)> = vec![];
    let mut seconds: Vec<(usize, String, String)> = vec![];
    for it in items {
        if !matches!(it.kind, ItemKind::Normal) {
            continue;
        }
        let text = &split[it.start..it.end];
        // (E0658: an unstable item of std's, `use std::intrinsics;`, after the item's attributes too)
        {
            let t = text.trim_start();
            let t = if t.starts_with("#[") { t[leading_attrs(t).len()..].trim_start() } else { t };
            let t = t.strip_prefix("pub ").unwrap_or(t);
            if let Some(rest) = t.strip_prefix("use ").filter(|r| r.starts_with("std::") || r.starts_with("core::")) {
                if let Some(e) = unstable_std_use(split, text, rest, it.line) {
                    return Some(e);
                }
            }
        }
        let t = text.trim_start();
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("use ") else { continue };
        let rest = rest.trim_start().trim_start_matches("::");
        let first: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        // (a fn's own `use`, hoisted out with a `__h<N>` suffix, is resolved in its block)
        if !known(&first) && !first.starts_with("r#") && !first.contains("__") {
            let line = it.line + text[..text.len() - text.trim_start().len()].matches('\n').count();
            // (`use m::f..`: the second segment, for a module of the program's)
            if let Some(r2) = rest[first.len()..].strip_prefix("::") {
                let second: String = r2.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !second.is_empty() && second != "self" && second != "super" {
                    seconds.push((line, first.clone(), second));
                } else if let Some(list) = r2.trim_start().strip_prefix('{').and_then(|l| l.split_once('}')).map(|(l, _)| l).filter(|l| !l.contains('{')) {
                    // (`use m::{f, g as h};`: each name of a flat list)
                    for part in list.split(',') {
                        // (`r#foo` names `foo`)
                        let p = part.trim_start();
                        let n: String = p.strip_prefix("r#").unwrap_or(p).chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                        if !n.is_empty() && n != "self" && n != "super" && !part.contains("::") {
                            seconds.push((line, first.clone(), n));
                        }
                    }
                }
            }
            firsts.push((line, first));
        }
    }
    if firsts.is_empty() {
        return None;
    }
    // (whether the program defines or brings in a name: a few substring searches per distinct candidate, not a
    // tokenization of the whole file)
    let b = src.as_bytes();
    let is_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let defines = |n: &str| {
        ["mod", "struct", "enum", "trait", "fn", "type", "const", "static", "union", "crate", "as", "macro_rules!"].iter().any(|kw| {
            let pat = format!("{kw} {n}");
            src.match_indices(pat.as_str()).any(|(i, _)| {
                let before_ok = i == 0 || !is_id(b[i - 1]);
                let after_ok = b.get(i + pat.len()).is_none_or(|&c| !is_id(c));
                // (not in a `//` comment: `//~| SUGGESTION extern crate foo;`)
                let ls = src[..i].rfind('\n').map(|k| k + 1).unwrap_or(0);
                before_ok && after_ok && !src[ls..i].contains("//")
            })
        })
    };
    let mut defined: crate::fxhash::FxSet<String> = Default::default();
    for (_, f) in &firsts {
        if defines(f) {
            defined.insert(f.clone());
        }
    }
    // (a macro could write items: leave such programs alone)
    if src.contains("macro_rules!") && firsts.iter().any(|(_, f)| !defined.contains(f)) && src.contains("$") {
        return None;
    }
    // (a top-level glob import may bring the name, an enum's variant for one (`use self::A::*; use V;`): a name the
    // text has elsewhere too is left to the module resolution)
    // (a glob of a path that starts at a name the program has: not one of the unresolved names)
    let globbed = items.iter().any(|it| matches!(it.kind, ItemKind::Normal) && {
        let t = split[it.start..it.end].trim();
        let t = t.strip_prefix("pub ").unwrap_or(t);
        t.trim_end_matches(';').trim_end().ends_with('*') && t.strip_prefix("use ").is_some_and(|r| {
            let r = r.trim_start().trim_start_matches("::");
            let first: String = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            matches!(first.as_str(), "crate" | "self" | "super") || defined.contains(&first) || defines(&first)
        })
    });
    // (the name as a word outside `//` comments, more than once: in the `use` and elsewhere)
    let elsewhere = |f: &str| {
        src.match_indices(f)
            .filter(|&(i, _)| {
                let ls = src[..i].rfind('\n').map_or(0, |k| k + 1);
                (i == 0 || !is_id(b[i - 1])) && b.get(i + f.len()).is_none_or(|&c| !is_id(c)) && !src[ls..i].contains("//")
            })
            .nth(1)
            .is_some()
    };
    if let Some((line, f)) = firsts.into_iter().find(|(_, f)| !defined.contains(f) && !(globbed && elsewhere(f))) {
        return Some(format!("{line}:1: unresolved import `{f}` (E0432)"));
    }
    // (`use m::f` with an inline `mod m { .. }` that defines no `f`)
    for (line, m, f) in seconds {
        let pat = format!("mod {m} {{");
        let Some(at) = src.match_indices(pat.as_str()).map(|(i, _)| i).find(|&i| i == 0 || !is_id(b[i - 1])) else { continue };
        let open = at + pat.len();
        let mut depth = 1;
        let mut end = None;
        for (k, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { continue };
        let body = &src[open..end];
        // (defined there, or brought in by a `use` of it, or written by a macro / glob)
        let bb = body.as_bytes();
        let named = ["mod", "struct", "enum", "trait", "fn", "type", "const", "static", "union", "as", "macro_rules!"].iter().any(|kw| {
            let p = format!("{kw} {f}");
            body.match_indices(p.as_str()).any(|(i, _)| (i == 0 || !is_id(bb[i - 1])) && bb.get(i + p.len()).is_none_or(|&c| !is_id(c)))
        }) || [format!("::{f};"), format!("::{f},"), format!("::{f} "), format!("{{{f}"), format!(" {f},"), format!(" {f}}}")].iter().any(|p| body.contains(p.as_str()))
            || body.contains('*')
            || body.contains('$')
            || body.contains("include!");
        if !named {
            return Some(format!("{line}:1: unresolved import `{m}::{f}` (E0432)"));
        }
    }
    None
}

/// std's items behind a library feature (rustc 1.100): the path as `std::..` (`core::..` reads the same), the feature, and
/// the reason rustc gives
const UNSTABLE_STD: &[(&str, &str, &str)] = &[
    ("std::intrinsics", "core_intrinsics", ": intrinsics are unlikely to ever be stabilized, instead they should be used through stabilized interfaces in the rest of the standard library"),
    ("std::ops::FnPtr", "fn_static", ""),
    ("std::field::FieldRepresentingType", "field_representing_type_raw", ""),
    ("std::marker::ConstParamTy_", "const_param_ty_trait", ""),
    ("std::marker::CoercePointee", "derive_coerce_pointee", ""),
    ("std::marker::CoerceShared", "reborrow", ""),
    ("std::marker::Reborrow", "reborrow", ""),
    ("std::ptr::Unique", "ptr_internals", ": use `NonNull` instead and consider `PhantomData<T>` (if you also use `#[may_dangle]`), `Send`, and/or `Sync`"),
    ("std::unicode::UNICODE_VERSION", "unicode_internals", ""),
    ("std::mem::TransmuteFrom", "transmutability", ""),
    ("std::mem::Assume", "transmutability", ""),
];

/// E0658 "use of unstable library feature `x`" of a `use` of one of UNSTABLE_STD (or of `core::io`'s items, at `io`),
/// the crate not enabling the feature: at the path, or at the name in a braced list (`use std::marker::{Copy, Reborrow};`)
#[cold]
#[inline(never)]
fn unstable_std_use(src: &str, text: &str, rest: &str, line: usize) -> Option<String> {
    let rest = &rest[..rest.find(';').unwrap_or(rest.len())];
    // (the column of rest's byte k on the item's line)
    let col = |k: usize| {
        let off = rest.as_ptr() as usize - text.as_ptr() as usize + k;
        let before = &text[..off];
        (line + before.matches('\n').count(), before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1)
    };
    let mut found: Option<((usize, usize), &str, &str)> = None;
    let mut look = |path: &str, at: usize| {
        let p = path.split(" as ").next().unwrap_or("").trim();
        let std = p.strip_prefix("core::").map(|r| format!("std::{r}")).unwrap_or_else(|| p.to_string());
        if found.is_some() {
            return;
        }
        if let Some(&(_, feat, why)) = UNSTABLE_STD.iter().find(|u| u.0 == std) {
            found = Some((col(at), feat, why));
        } else if p.starts_with("core::io::") || p == "core::io" {
            found = Some((col(at + 6), "core_io", ""));
        }
    };
    match rest.find('{') {
        Some(b) => {
            let prefix = &rest[..b];
            let inner = &rest[b + 1..rest.rfind('}').unwrap_or(rest.len())];
            if inner.contains('{') {
                return None;
            }
            let mut k = b + 1;
            for part in inner.split(',') {
                let lead = part.len() - part.trim_start().len();
                look(&format!("{prefix}{}", part.trim()), k + lead);
                k += part.len() + 1;
            }
        }
        None => look(rest, 0),
    }
    let ((l, c), feat, why) = found?;
    if enabled_features(src).iter().any(|f| f == feat) {
        return None;
    }
    Some(format!("{l}:{c}: use of unstable library feature `{feat}`{why} (E0658)"))
}

/// The naming lints (`deny` = [non_snake_case, non_camel_case_types, non_upper_case_globals]): the first fn / method,
/// type / trait or static / const of the program whose name breaks its case convention, with rustc's message
fn first_bad_name(src: &str, items: &[SrcItem], deny: [bool; 3]) -> Option<(usize, String)> {
    let snake = |n: &str| !n.chars().any(|c| c.is_uppercase());
    let camel = |n: &str| {
        let t = n.trim_matches('_');
        t.is_empty()
            || (!t.chars().next().is_some_and(|c| c.is_lowercase())
                && !t.contains("__")
                && !t.as_bytes().windows(2).any(|w| w[0] == b'_' && !w[1].is_ascii_digit()))
    };
    let upper = |n: &str| !n.chars().any(|c| c.is_lowercase());
    for it in items {
        let text = &src[it.start..it.end];
        if text.contains("allow(non_") || text.contains("expect(non_") || text.contains("allow(nonstandard_style") || text.contains("#[cfg") || text.contains("no_mangle")
            || text.contains("extern \"C\"") || text.contains("export_name")
        {
            continue;
        }
        let t = crate::lexer::tokenize(text.as_bytes());
        let tw = |k: usize| -> &str { if t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" } };
        let mut k = 0;
        while k < t.len() && (t[k].0 != 0 || matches!(tw(k), "pub" | "unsafe" | "async" | "crate" | "default")) {
            // (skip attributes and visibility: `#[..]`, `pub(crate)`)
            if tw(k) == "impl" {
                break;
            }
            k += 1;
        }
        if k + 1 >= t.len() {
            continue;
        }
        let kw = tw(k);
        let kw = if kw == "const" && tw(k + 1) == "fn" { "fn" } else { kw };
        let nk = if tw(k) == "const" && tw(k + 1) == "fn" { k + 2 } else { k + 1 };
        if nk >= t.len() {
            continue;
        }
        let raw = tw(nk);
        // (a fn-local item hoisted out has a `__h<N>` suffix; a module's item a `mod__` prefix)
        let raw = match raw.rfind("__h") {
            Some(i) if i > 0 && raw[i + 3..].chars().all(|c| c.is_ascii_digit()) && raw.len() > i + 3 => &raw[..i],
            _ => raw,
        };
        let name = raw.rsplit("__").next().unwrap_or(raw);
        let name = if raw.contains("__") { name.trim_start_matches("u_") } else { raw };
        if name.is_empty() || name == "_" || name.starts_with("r#") {
            continue;
        }
        let member = matches!(it.kind, ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
        // (a struct's fields: `X: usize` is "structure field `X`")
        if (kw == "struct" || kw == "enum") && deny[0] {
            // (an enum's struct-like variants: their fields one level deeper)
            let field_depth = if kw == "enum" { 2 } else { 1 };
            let mut depth = 0;
            for j in nk + 1..t.len() {
                if t[j].0 == 1 {
                    match text.as_bytes()[t[j].1] {
                        b'{' | b'(' | b'<' | b'[' => depth += 1,
                        b'}' | b')' | b'>' | b']' => depth -= 1,
                        _ => {}
                    }
                    continue;
                }
                let f = tw(j);
                if depth == field_depth && j + 1 < t.len() && t[j + 1].0 == 1 && text.as_bytes()[t[j + 1].1] == b':' && f != "pub" && !snake(f) && !f.starts_with('_') {
                    let line = it.line + text[..t[j].1].matches('\n').count();
                    return Some((line, format!("structure field `{f}` should have a snake case name")));
                }
            }
        }
        // (lifetime parameters in the header: `fn f<'FooBar>` / `struct S<'Ab>`)
        if deny[0] {
            let head = &text[..text.find('{').unwrap_or(text.len())];
            let hb = head.as_bytes();
            let mut i = 0;
            while let Some(k) = head[i..].find('\'') {
                let a = i + k + 1;
                let e = a + head[a..].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(head.len() - a);
                // (a lifetime, not a char literal `'a'`)
                if e > a && hb.get(e) != Some(&b'\'') {
                    let l = &head[a..e];
                    if l != "static" && !snake(l) {
                        let line = it.line + head[..a].matches('\n').count();
                        return Some((line, format!("lifetime `'{l}` should have a snake case name")));
                    }
                }
                i = e.max(a);
            }
        }
        // (a fn's parameters and `let`s: "variable `X`")
        if kw == "fn" && deny[0] {
            if let Ok(Some(f)) = parse_fn(src, it) {
                for (p, _) in &f.sig.params {
                    if let Pat::Ident(_, _, n, None) = p {
                        if !snake(n) && !n.starts_with("__exr") {
                            return Some((it.line, format!("variable `{n}` should have a snake case name")));
                        }
                    }
                }
                for (pos, n) in crate::ast_closure::let_bindings(&f.block) {
                    // (an upper-case name in a pattern may be a constant or a unit struct: only clear variables)
                    if !snake(&n) && n.chars().any(|c| c.is_lowercase()) && !n.starts_with("__") && pos.line > 0 {
                        return Some((it.line + pos.line as usize - 1, format!("variable `{n}` should have a snake case name")));
                    }
                }
            }
        }
        let in_trait = matches!(it.kind, ItemKind::TraitMember(_));
        let msg = match kw {
            "fn" if deny[0] && !snake(name) => {
                let what = if in_trait { "trait method" } else if member { "method" } else { "function" };
                format!("{what} `{name}` should have a snake case name")
            }
            "struct" | "enum" | "union" | "type" if deny[1] && !camel(name) => {
                let what = if kw == "type" && member { "associated type" } else { "type" };
                format!("{what} `{name}` should have an upper camel case name")
            }
            "trait" if deny[1] && !camel(name) => format!("trait `{name}` should have an upper camel case name"),
            "static" if deny[2] && !upper(name) => format!("static variable `{name}` should have an upper case name"),
            "const" if deny[2] && !upper(name) => {
                let what = if member { "associated constant" } else { "constant" };
                format!("{what} `{name}` should have an upper case name")
            }
            _ => continue,
        };
        let line = it.line + text[..t[nk].1].matches('\n').count();
        return Some((line, msg));
    }
    None
}

/// `#![deny(dead_code)]`: the first top-level item of the program that nothing uses (see the caller)
fn first_dead_item(src: &str, items: &[SrcItem], check_pub: bool, warn_denied: bool) -> Option<(usize, String)> {
    let (Some(first), Some(last)) = (items.first(), items.last()) else { return None };
    let region = &src[first.start..last.end];
    let b = region.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let word = |k: usize| -> &str { if toks[k].0 == 0 { &region[toks[k].1..toks[k].2] } else { "" } };
    let sym = |k: usize, c: u8| toks[k].0 == 1 && b[toks[k].1] == c;
    // (uses of each name: every occurrence but the definition's, the one after `impl` / `for` in an impl header)
    let mut uses: crate::fxhash::FxMap<&str, usize> = Default::default();
    let mut impl_uses: crate::fxhash::FxMap<&str, usize> = Default::default();
    let mut in_impl_header = false;
    for k in 0..toks.len() {
        let w = word(k);
        if w == "impl" {
            in_impl_header = true;
        }
        if in_impl_header && sym(k, b'{') {
            in_impl_header = false;
        }
        if toks[k].0 != 0 {
            continue;
        }
        // (not a raw pointer's `*const T`)
        let defn = k > 0 && matches!(word(k - 1), "fn" | "struct" | "enum" | "const" | "static" | "trait" | "union" | "type") && !(k > 1 && sym(k - 2, b'*'));
        let impl_self = in_impl_header && k > 0 && (word(k - 1) == "impl" || word(k - 1) == "for" || sym(k - 1, b'>'));
        if !defn && !impl_self {
            *uses.entry(w).or_default() += 1;
        }
        // (a type alias an impl header names is used: `impl Tr for Alias`)
        if impl_self {
            *impl_uses.entry(w).or_default() += 1;
        }
    }
    for it in items {
        // (top-level items only: impl and trait members are a type's, not free items)
        if !matches!(it.kind, crate::lexer::ItemKind::Normal) {
            continue;
        }
        let text = &src[it.start..it.end];
        // (the item's own level, the last of its attributes naming dead_code: `#[allow(dead_code)] #[warn(dead_code)]`
        // is a warn, an error under `deny(warnings)`; with none of its own, any allow in it leaves it alone)
        let allowed = match item_lint_level(text, "dead_code").or_else(|| item_lint_level(text, "unused")) {
            Some("allow" | "expect") => true,
            Some("warn") => !warn_denied,
            Some(_) => false,
            None => text.contains("allow(dead_code") || text.contains("expect(dead_code") || text.contains("allow(unused"),
        };
        if allowed || text.contains("no_mangle") || text.contains("#[test]") || text.contains("#[derive") || text.contains("export_name") || text.contains("#[cfg") {
            continue;
        }
        let t = crate::lexer::tokenize(text.as_bytes());
        let tw = |k: usize| -> &str { if t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" } };
        // (the item's keyword and name, after its lint-level attributes and visibility; another attribute (`#[used]`,
        // `#[panic_handler]` ..) may make it used: the scan stops at its name, and the item is left alone)
        let sym_at = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && text.as_bytes()[t[k].1] == c;
        let mut k = 0;
        while k < t.len() && (tw(k) == "pub" || tw(k) == "unsafe" || tw(k) == "async" || tw(k) == "extern" || t[k].0 != 0) {
            if tw(k) == "extern" || tw(k) == "impl" || tw(k) == "mod" || tw(k) == "use" || tw(k) == "macro_rules" {
                break;
            }
            if sym_at(k, b'#') && sym_at(k + 1, b'[') && matches!(tw(k + 2), "allow" | "warn" | "deny" | "forbid" | "expect") {
                let mut d = 0;
                let mut j = k + 1;
                while j < t.len() {
                    if sym_at(j, b'[') {
                        d += 1;
                    } else if sym_at(j, b']') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                k = j + 1;
                continue;
            }
            k += 1;
        }
        if k + 1 >= t.len() {
            continue;
        }
        let (kw, name) = (tw(k), tw(k + 1));
        if !check_pub && (0..k).any(|j| tw(j) == "pub") {
            continue;
        }
        // (`_foo`, also in a flattened module: `m___foo`)
        if name.is_empty() || name.starts_with('_') || name.split("__").last().is_some_and(|s| s.starts_with('_')) || name == "main" {
            continue;
        }
        // (the name as written: without the flattened modules' `a__` prefix and the renamer's `__u_`)
        let shown = name.rsplit("__").next().unwrap_or(name).trim_start_matches("u_");
        let shown = if name.contains("__") { shown } else { name };
        let what = match kw {
            "fn" => format!("function `{shown}` is never used"),
            "struct" => format!("struct `{shown}` is never constructed"),
            "enum" => format!("enum `{shown}` is never used"),
            "const" => format!("constant `{shown}` is never used"),
            "static" => format!("static `{shown}` is never used"),
            "trait" => format!("trait `{shown}` is never used"),
            "type" => format!("type alias `{shown}` is never used"),
            _ => continue,
        };
        let alias_in_impl = kw == "type" && impl_uses.get(name).copied().unwrap_or(0) > 0;
        if uses.get(name).copied().unwrap_or(0) == 0 && !alias_in_impl {
            let line = it.line + text[..t[k].1].matches('\n').count();
            return Some((line, what));
        }
    }
    None
}

/// `#![deny(unused_lifetimes)]` / `#![deny(single_use_lifetimes)]`: the first lifetime parameter of a fn or an impl
/// named nowhere else in it / named once, as `line:col: message`
/// How a signature writes a lifetime (mismatched_lifetime_syntaxes): left out of a path (`S` for `S<'a>`), elided
/// (`&u8`, `'_`), or named (`'a`, `'static`)
#[derive(Clone, Copy, PartialEq)]
enum LtSyntax {
    Hidden,
    Elided,
    Named,
}

/// The lifetimes t writes or leaves out: (the name if it is named, how). Not a `dyn` type's own lifetime, nor one
/// inside a fn pointer's or an `Fn(..)` bound's types, which have their own
fn lt_syntaxes(t: &crate::parse::Ty, adts: &crate::fxhash::FxMap<&str, usize>, out: &mut Vec<(Option<String>, LtSyntax)>, depth: u32) {
    use crate::parse::{Bound, Ty as PTy};
    fn named(l: &str, out: &mut Vec<(Option<String>, LtSyntax)>) {
        out.push(if l == "'_" { (None, LtSyntax::Elided) } else { (Some(l.to_string()), LtSyntax::Named) });
    }
    fn path(p: &crate::parse::Path, adts: &crate::fxhash::FxMap<&str, usize>, out: &mut Vec<(Option<String>, LtSyntax)>, depth: u32) {
        let Some(s) = p.segs.last() else { return };
        if s.fn_args.is_some() {
            return;
        }
        let mut written = 0;
        for a in &s.args {
            match a {
                crate::parse::GArg::Lifetime(l) => {
                    written += 1;
                    named(l, out);
                }
                crate::parse::GArg::Ty(t) | crate::parse::GArg::Bind(_, t) => lt_syntaxes(t, adts, out, depth + 1),
                _ => {}
            }
        }
        if written == 0 && p.segs.len() == 1 {
            if let Some(&n) = adts.get(s.name.as_str()) {
                out.extend((0..n).map(|_| (None, LtSyntax::Hidden)));
            }
        }
    }
    if depth > 16 {
        return;
    }
    match t {
        PTy::Ref(l, _, i) => {
            match l {
                Some(l) => named(l, out),
                None => out.push((None, LtSyntax::Elided)),
            }
            lt_syntaxes(i, adts, out, depth + 1);
        }
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => lt_syntaxes(i, adts, out, depth + 1),
        PTy::Tuple(ts) => ts.iter().for_each(|t| lt_syntaxes(t, adts, out, depth + 1)),
        PTy::Path(None, p) => path(p, adts, out, depth),
        PTy::Impl(bs) => {
            for b in bs {
                match b {
                    Bound::Lifetime(l) => named(l, out),
                    Bound::Trait(_, p) => path(p, adts, out, depth),
                }
            }
        }
        PTy::Dyn(bs) => {
            for b in bs {
                if let Bound::Trait(_, p) = b {
                    path(p, adts, out, depth);
                }
            }
        }
        _ => {}
    }
}

/// mismatched_lifetime_syntaxes: the first fn whose output writes a lifetime of its inputs another way than they do:
/// named in the inputs and elided or hidden in the output, or elided in one and hidden in the other (`&u8` to `S`).
/// The output's elided lifetimes are the receiver's, else the inputs' only one. `line:1: msg` at the output's line
/// (marked: only the items with a `#[deny(mismatched_lifetime_syntaxes)]` of their own)
fn first_mismatched_lifetime_syntax(src: &str, items: &[SrcItem], adts: &crate::fxhash::FxMap<&str, usize>, marked: bool) -> Option<String> {
    use crate::parse::{ItemKind, Ty as PTy};
    for it in items {
        let text = src.get(it.start..it.end).unwrap_or("");
        if !text.contains("fn ") || !text.contains("->") || text.contains("#[cfg") {
            continue;
        }
        // (the item's own level of the lint: an allow / expect / warn takes it out, and where only items deny it, an
        // item is checked when it denies or forbids it)
        let level = item_lint_level(text, "mismatched_lifetime_syntaxes");
        if matches!(level, Some("allow" | "expect" | "warn")) || (marked && !matches!(level, Some("deny" | "forbid"))) {
            continue;
        }
        let Ok(f) = crate::parse::parse_file(text) else { continue };
        let Some(item) = f.items.first() else { continue };
        let ItemKind::Fn(sig, _) = &item.kind else { continue };
        let Some(ret) = &sig.ret else { continue };
        // (a type parameter named as a struct (`fn f<S>(..) -> Option<S>` with `struct S<'s>`) hides nothing)
        let shadowed: Vec<&str> = sig.generics.params.iter().filter_map(|p| if let crate::parse::GParam::Ty(n, ..) = p { Some(n.as_str()) } else { None }).filter(|n| adts.contains_key(n)).collect();
        let own;
        let adts = if shadowed.is_empty() {
            adts
        } else {
            own = adts.iter().filter(|(k, _)| !shadowed.contains(k)).map(|(k, v)| (*k, *v)).collect::<crate::fxhash::FxMap<&str, usize>>();
            &own
        };
        let (mut k, mut ids, mut ins, mut recv) = (0usize, Vec::<String>::new(), Vec::<(String, LtSyntax)>::new(), None::<String>);
        for (j, t) in sig.recv_ty.iter().chain(sig.params.iter().map(|p| &p.1)).enumerate() {
            let mut v = vec![];
            lt_syntaxes(t, adts, &mut v, 0);
            for (n, c) in v {
                let id = n.unwrap_or_else(|| {
                    k += 1;
                    format!("#{k}")
                });
                if j == 0 && sig.recv_ty.is_some() && recv.is_none() && matches!(t, PTy::Ref(..)) {
                    recv = Some(id.clone());
                }
                if !ids.contains(&id) {
                    ids.push(id.clone());
                }
                ins.push((id, c));
            }
        }
        let target = recv.or_else(|| (ids.len() == 1).then(|| ids[0].clone()));
        let mut v = vec![];
        lt_syntaxes(ret, adts, &mut v, 0);
        let outs: Vec<(String, LtSyntax)> = v.into_iter().filter_map(|(n, c)| Some((n.or_else(|| target.clone())?, c))).collect();
        for id in &ids {
            let ic: Vec<LtSyntax> = ins.iter().filter(|o| &o.0 == id).map(|o| o.1).collect();
            let oc: Vec<LtSyntax> = outs.iter().filter(|o| &o.0 == id).map(|o| o.1).collect();
            if oc.is_empty() {
                continue;
            }
            let msg = if ic.contains(&LtSyntax::Named) {
                match (oc.contains(&LtSyntax::Elided), oc.contains(&LtSyntax::Hidden)) {
                    (true, true) => "hiding or eliding a lifetime that's named elsewhere is confusing",
                    (true, false) => "eliding a lifetime that's named elsewhere is confusing",
                    (false, true) => "hiding a lifetime that's named elsewhere is confusing",
                    _ => continue,
                }
            } else if ic.iter().chain(&oc).any(|&c| c == LtSyntax::Hidden) && ic.iter().chain(&oc).any(|&c| c == LtSyntax::Elided) {
                "hiding a lifetime that's elided elsewhere is confusing"
            } else {
                continue;
            };
            let at = text.find("->").map_or(0, |i| text[..i].matches('\n').count());
            return Some(format!("{}:1: {msg}", it.line + at));
        }
    }
    None
}

/// The level an item's own attributes (`#[allow(dead_code, n)]`, `#[expect(n)]`, `#[warn(n)]`, `#[deny(n)]`,
/// `#[forbid(n)]`) give the lint n: the last of them, if one names it. Outer attributes and comments before the item
fn item_lint_level(text: &str, n: &str) -> Option<&'static str> {
    let mut rest = text;
    let mut level = None;
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("//") {
            rest = r.split_once('\n').map_or("", |x| x.1);
            continue;
        }
        // (the crate's inner attributes, at the head of the first item's text: the crate's levels, not the item's)
        if let Some(r) = rest.strip_prefix("#![") {
            let Some(end) = r.find(']') else { break };
            rest = &r[end + 1..];
            continue;
        }
        let Some(r) = rest.strip_prefix("#[") else { break };
        let Some(end) = r.find(']') else { break };
        if let Some((name, list)) = r[..end].split_once('(') {
            let lv = match name.trim() {
                "allow" => Some("allow"),
                "expect" => Some("expect"),
                "warn" => Some("warn"),
                "deny" => Some("deny"),
                "forbid" => Some("forbid"),
                _ => None,
            };
            let list = list.trim_end().strip_suffix(')').unwrap_or(list);
            if lv.is_some() && list.split(',').any(|x| x.trim() == n) {
                level = lv;
            }
        }
        rest = &r[end + 1..];
    }
    level
}

fn first_unused_lifetime(src: &str, items: &[SrcItem], (unused, single): (bool, bool)) -> Option<String> {
    use crate::parse::{GParam, ItemKind};
    // (the uses of l in t, as tokens: not `'a'` (a char) nor `'ab`)
    let count = |t: &str, l: &str| {
        t.match_indices(l)
            .filter(|(i, _)| {
                let next = t[i + l.len()..].chars().next();
                !next.is_some_and(|c| c == '\'' || c.is_alphanumeric() || c == '_')
            })
            .count()
    };
    // (the text without its comments and string literals, where a lifetime's name is no use of it)
    let code = |t: &str| strip_strings(t);
    // (`#[allow(dead_code, unused_lifetimes)]`: the lint anywhere in an allow list)
    let allows = |t: &str, lint: &str| {
        t.match_indices("allow(").any(|(i, _)| {
            let rest = &t[i + 6..];
            let list = &rest[..rest.find(')').unwrap_or(rest.len())];
            list.split(',').any(|x| x.trim() == lint)
        })
    };
    for (k, it) in items.iter().enumerate() {
        let text = src.get(it.start..it.end).unwrap_or("");
        if !text.contains('\'') || allows(text, "unused_lifetimes") || allows(text, "single_use_lifetimes") || text.contains("#[cfg") || text.contains("macro_rules") {
            continue;
        }
        // (an impl's header, `impl<'a> S<'a> {`: its members name its lifetimes too)
        let header = matches!(it.kind, crate::lexer::ItemKind::ImplHeader);
        let Ok(f) = (if header { crate::parse::parse_file(&format!("{text}}}")) } else { crate::parse::parse_file(text) }) else { continue };
        let Some(item) = f.items.into_iter().next() else { continue };
        let (g, ret, params): (_, _, Vec<&crate::parse::Ty>) = match &item.kind {
            ItemKind::Fn(sig, Some(_)) => (&sig.generics, sig.ret.as_ref().map(|r| format!("{r:?}")).unwrap_or_default(), sig.recv_ty.iter().chain(sig.params.iter().map(|p| &p.1)).collect()),
            ItemKind::Impl(g, _, _, _) => (g, String::new(), vec![]),
            _ => continue,
        };
        let is_fn = matches!(item.kind, ItemKind::Fn(..));
        let members: String = if header {
            items.iter().filter(|m| matches!(m.kind, crate::lexer::ItemKind::ImplMember(h) if h as usize == k)).map(|m| code(src.get(m.start..m.end).unwrap_or(""))).collect::<Vec<_>>().join("\n")
        } else {
            String::new()
        };
        let own = code(text);
        for p in &g.params {
            let GParam::Lifetime(l) = p else { continue };
            let n = count(&own, l) + count(&members, l);
            let off = text.find(l.as_str()).unwrap_or(0);
            let line = it.line + text[..off].matches('\n').count();
            if unused && n <= 1 {
                return Some(format!("{line}:1: lifetime parameter `{l}` never used (#![deny(unused_lifetimes)])"));
            }
            // (declared with bounds, or bounding another: a use of its own)
            let bounded = g.preds.iter().any(|(t, bs)| format!("{t:?}").contains(&format!("\"{l}\"")) || bs.iter().any(|b| matches!(b, crate::parse::Bound::Lifetime(x) if x == l)));
            // (a fn's: not in its result's type, which has nothing to elide it to, nor in an `impl` / `dyn` / fn
            // pointer type, where it cannot be elided)
            let elidable = || params.iter().any(|t| elidable_use(t, l)) || !params.iter().any(|t| format!("{t:?}").contains(&format!("\"{l}\"")));
            if single && n == 2 && !bounded && (!is_fn || (count(&ret, l) == 0 && elidable())) {
                return Some(format!("{line}:1: lifetime parameter `{l}` only used once (#![deny(single_use_lifetimes)])"));
            }
        }
    }
    None
}

/// t with its string literals (`"'a"`, raw ones too) and comments blanked out, in one pass: a `//` in a string is
/// no comment, a `"` in a comment no string. Lines are kept
fn strip_strings(t: &str) -> String {
    let b = t.as_bytes();
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    // (blanks b[i..end], keeping its newlines)
    let blank = |out: &mut String, from: usize, end: usize| {
        for &c in &b[from..end] {
            out.push(if c == b'\n' { '\n' } else { ' ' });
        }
    };
    while i < b.len() {
        // (a `"` or `/` char literal is none of these)
        if b[i] == b'\'' && i + 2 < b.len() && b[i + 2] == b'\'' && (b[i + 1] == b'"' || b[i + 1] == b'/') {
            out.push_str(&t[i..i + 3]);
            i += 3;
            continue;
        }
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = t[i..].find('\n').map_or(b.len(), |k| i + k);
            blank(&mut out, i, end);
            i = end;
            continue;
        }
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            // (block comments nest, as in Rust and in `tokenize`)
            let (mut depth, mut j) = (0usize, i);
            while j < b.len() {
                if b[j..].starts_with(b"/*") {
                    depth += 1;
                    j += 2;
                } else if b[j..].starts_with(b"*/") {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            let end = j.min(b.len());
            blank(&mut out, i, end);
            i = end;
            continue;
        }
        // (`r"..."` / `r#"..."#`: to the quote and as many `#`)
        if b[i] == b'r' && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            let mut j = i + 1;
            while j < b.len() && b[j] == b'#' {
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                let hashes = j - i - 1;
                let close: String = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
                let end = t[j + 1..].find(close.as_str()).map_or(b.len(), |k| j + 1 + k + close.len());
                blank(&mut out, i, end);
                i = end;
                continue;
            }
        }
        if b[i] == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(b.len());
            blank(&mut out, i, end);
            i = end;
            continue;
        }
        let c = t[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// t names l where eliding it is allowed: a reference's lifetime or a path's lifetime argument, not in an `impl` /
/// `dyn` / fn pointer type or an associated type's binding
fn elidable_use(t: &crate::parse::Ty, l: &str) -> bool {
    use crate::parse::{GArg, Ty as PTy};
    match t {
        PTy::Ref(x, _, i) => x.as_deref() == Some(l) || elidable_use(i, l),
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => elidable_use(i, l),
        PTy::Tuple(ts) => ts.iter().any(|t| elidable_use(t, l)),
        PTy::Path(None, p) => p.segs.iter().flat_map(|s| s.args.iter()).any(|a| match a {
            GArg::Lifetime(x) => x == l,
            GArg::Ty(t) => elidable_use(t, l),
            _ => false,
        }),
        _ => false,
    }
}

pub fn parse_fn(src: &str, it: &SrcItem) -> R<Option<ItemFn>> {
    let text = &src[it.start..it.end];
    let f = crate::parse::parse_file(text).map_err(|e| relocate(e, it, src))?;
    let Some(item) = f.items.into_iter().next() else { return Ok(None) };
    let crate::parse::ItemKind::Fn(mut sig, Some(mut block)) = item.kind else { return Ok(None) };
    let mut lets = vec![];
    for (k, (p, _)) in sig.params.iter_mut().enumerate() {
        // (a binding by reference or with a subpattern (`ref a @ b: U`) binds through a `let` too)
        if matches!(p, Pat::Ident(false, _, _, None) | Pat::Wild) {
            continue;
        }
        let name = format!("__exr_arg{k}");
        let pat = std::mem::replace(p, Pat::Ident(false, false, name.clone(), None));
        let at = crate::parse::pat_pos(&pat);
        lets.push(Stmt::Let(pat, None, Some(acg::path_expr(&name)), None, at));
    }
    if !lets.is_empty() {
        // (the statements' starts with them: each let at its pattern)
        // (the body's statements move by the lets; a let has its own position)
        block.spos = block.spos.shifted(0, lets.len() as isize);
        lets.append(&mut block.stmts);
        block.stmts = lets;
    }
    Ok(Some(ItemFn { attrs: item.attrs, sig, block }))
}

/// クロージャ式を関数にする (`fn call(__env: (), p0: (), ..) { 本体 }`: 型はシグネチャから決まる)。
/// in_unsafe: the closure is written where `unsafe` operations are allowed, so its body is put in `unsafe { }`
pub fn synth_closure(c: &Expr, in_unsafe: bool) -> ItemFn {
    let Expr::Closure(_, ps, _, body, _) = c else { unreachable!() };
    let unit = crate::parse::Ty::Tuple(vec![]);
    let mut params = vec![(Pat::Ident(false, false, "__env".into(), None), unit.clone())];
    let mut stmts = vec![];
    for (i, (p, _)) in ps.iter().enumerate() {
        let simple = matches!(p, Pat::Ident(false, _, _, None));
        let pin = if simple {
            p.clone()
        } else {
            let id = format!("__p{i}");
            stmts.push(Stmt::Let(p.clone(), None, Some(acg::path_expr(&id)), None, crate::parse::pat_pos(p)));
            Pat::Ident(false, false, id, None)
        };
        params.push((pin, unit.clone()));
    }
    // (the body's statements' starts, after the parameters' lets, each at its pattern)
    let mut spos = crate::parse::Spos::default();
    match &**body {
        Expr::Block(None, false, b) => {
            spos = b.spos.shifted(0, stmts.len() as isize);
            stmts.extend(b.stmts.iter().cloned());
        }
        e => stmts.push(Stmt::Expr(e.clone(), false)),
    }
    if in_unsafe {
        stmts = vec![Stmt::Expr(Expr::Block(None, true, Block { stmts, spos }), false)];
        spos = crate::parse::Spos::default();
    }
    let sig = crate::parse::FnSig {
        name: "call".into(),
        generics: crate::parse::Generics { params: vec![], preds: vec![] },
        recv: None,
        recv_ty: None,
        params,
        ret: None,
        quals: vec![],
    };
    ItemFn { attrs: vec![], sig, block: Block { stmts, spos } }
}

/// The text of an item after its attributes and comments, and the number of lines skipped to get there
fn after_attrs(text: &str) -> (&str, usize) {
    let mut t = text;
    loop {
        let u = t.trim_start();
        if let Some(r) = u.strip_prefix("//") {
            t = r.find('\n').map(|k| &r[k..]).unwrap_or("");
        } else if let Some(r) = u.strip_prefix("/*") {
            t = r.find("*/").map(|k| &r[k + 2..]).unwrap_or("");
        } else if let Some(r) = u.strip_prefix("#[").or_else(|| u.strip_prefix("#![")) {
            t = &r[attr_end(r)..];
        } else {
            t = u;
            break;
        }
    }
    let skipped = &text[..text.len() - t.len()];
    (t, skipped.matches('\n').count())
}

/// The length of an attribute's body up to and with its closing `]` (r is after `#[`). Brackets inside string, raw
/// string and char literals do not count: `#[doc = "]"]` ends at the last `]`
fn attr_end(r: &str) -> usize {
    let b = r.as_bytes();
    let mut depth = 1;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            // a raw string `r"..."` / `r#"..."#`: to the quote followed by as many `#`
            b'r' if i + 1 < b.len() && (b[i + 1] == b'"' || b[i + 1] == b'#') && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) => {
                let mut j = i + 1;
                let mut hashes = 0;
                while j < b.len() && b[j] == b'#' {
                    hashes += 1;
                    j += 1;
                }
                if j < b.len() && b[j] == b'"' {
                    j += 1;
                    while j < b.len() && !(b[j] == b'"' && b[j + 1..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes) {
                        j += 1;
                    }
                    i = j + hashes;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            // a char literal ('x', '\'', '\\'): not a lifetime, which has no closing quote two or three bytes on
            b'\'' if b.get(i + 2) == Some(&b'\'') || (b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'')) => {
                i += if b[i + 1] == b'\\' { 3 } else { 2 };
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// t without its `// ..` and `/* .. */` comments (each one a space)
fn strip_comments(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    loop {
        let (a, b) = (rest.find("//"), rest.find("/*"));
        let k = match (a, b) {
            (Some(x), Some(y)) => x.min(y),
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => break,
        };
        out.push_str(&rest[..k]);
        out.push(' ');
        rest = if rest[k..].starts_with("//") {
            rest[k..].find('\n').map(|e| &rest[k + e..]).unwrap_or("")
        } else {
            rest[k + 2..].find("*/").map(|e| &rest[k + 2 + e + 2..]).unwrap_or("")
        };
    }
    out.push_str(rest);
    out
}

/// `t` without the keyword `w` at its start (and the spaces after it)
fn after_word<'a>(t: &'a str, w: &str) -> Option<&'a str> {
    let r = t.strip_prefix(w)?;
    if r.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(r.trim_start())
}

/// E0200: an impl of an unsafe trait (`Send`, `Sync`, or an `unsafe trait` of the program) must be an `unsafe impl`.
/// Implementing `Send` for a type holding an `Rc` is how safe code would otherwise share it between threads
fn unsafe_impls(src: &str, items: &[SrcItem]) -> R<()> {
    let mut unsafe_traits: Vec<&str> = vec!["Send", "Sync"];
    for it in items {
        let (t, _) = after_attrs(src.get(it.start..it.end).unwrap_or(""));
        let t = after_word(t, "pub").map(|r| if r.starts_with('(') { r.find(')').map(|k| r[k + 1..].trim_start()).unwrap_or(r) } else { r }).unwrap_or(t);
        if let Some(r) = after_word(t, "unsafe").and_then(|r| after_word(r, "trait")) {
            let n = r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("");
            if !n.is_empty() {
                unsafe_traits.push(n);
            }
        }
    }
    for it in items {
        if !matches!(it.kind, ItemKind::Normal | ItemKind::ImplHeader) {
            continue;
        }
        let (t, skipped) = after_attrs(src.get(it.start..it.end).unwrap_or(""));
        let Some(mut r) = after_word(t, "impl") else { continue };
        // (the impl's own generics: `impl<T: Send> Send for W<T>`)
        if r.starts_with('<') {
            let mut depth = 0;
            let mut end = r.len();
            for (k, c) in r.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' if k > 0 && r.as_bytes()[k - 1] == b'-' => {}
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = k + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            r = r[end..].trim_start();
        }
        // (the header without comments, in single spaces: `impl Send // ..\nfor S` is the same header)
        let head = strip_comments(r.split('{').next().unwrap_or("")).split_whitespace().collect::<Vec<_>>().join(" ");
        let Some(k) = head.find(" for ") else { continue };
        let path = head[..k].trim();
        let last = path.split('<').next().unwrap_or("").rsplit("::").next().unwrap_or("").trim();
        if unsafe_traits.contains(&last) {
            return Err(format!("{}:1: the trait `{last}` requires an `unsafe impl` declaration (E0200)", it.line + skipped));
        }
    }
    Ok(())
}

/// 関数 (インスタンスも) id の本体
fn fn_ast(a: &Analysis, body: u32) -> R<ItemFn> {
    let ii = a.prog.item_of[body as usize].ok_or("internal: no item")?;
    let it = &a.items[ii as usize];
    if let Some(cd) = a.prog.closures.get(&body) {
        let mut encl = a.encl.lock().unwrap();
        if !encl.contains_key(&ii) {
            let f = parse_fn(&a.src, it)?.ok_or_else(|| format!("`{}` has no body", a.prog.names[body as usize]))?;
            let u = crate::ast_closure::closures_in_unsafe(&f.block);
            encl.insert(ii, crate::ast_closure::closures_of(&f.block).into_iter().zip(u).collect());
        }
        let cs = encl.get(&ii).unwrap();
        let (c, in_unsafe) = cs.get(cd.k as usize).ok_or("internal: closure not found")?;
        // (a closure written in `unsafe { }` or in an `unsafe fn` may do what its surroundings may)
        let in_unsafe = *in_unsafe || a.prog.sigs.get(cd.parent as usize).is_some_and(|s| s.is_unsafe);
        return Ok(synth_closure(c, in_unsafe));
    }
    let f = parse_fn(&a.src, it)?.ok_or_else(|| format!("`{}` has no body", a.prog.names[body as usize]))?;
    Ok(f)
}

/// 展開してよい関数: prelude の (クロージャでも特別な本体でもない) 小さい関数
fn inline_bodies(a: &Analysis) -> acg::Inline {
    let p = &a.prog;
    let mut inl = acg::Inline::default();
    let special = [
        crate::prelude::DROP_GLUE, "__exr_hash", "__exr_cmp", "__exr_eq", "__exr_clone", "__exr_default", "__exr_display", "__exr_debug",
        "__exr_rt_init", "__exr_on_trap", "Option::unwrap", "Option::expect", "Result::unwrap", "Result::expect", "Result::unwrap_err", "Result::expect_err",
    ];
    // (read once: getenv scans the whole environment)
    let (no_inline_closure, inline_log) = (std::env::var_os("ZUSTC_NO_INLINE_CLOSURE").is_some(), std::env::var_os("ZUSTC_INLINE_LOG").is_some());
    for id in 0..p.names.len() {
        let Some(ii) = p.item_of[id] else { continue };
        // (依存するクレートの関数は展開しない: 本体はインターフェースのハッシュに入らないので、変わっても気付けない)
        if a.ext_items.get(ii as usize).copied().unwrap_or(false) || !p.sigs[id].has_body || p.sigs[id].is_extern {
            continue;
        }
        // (クロージャの本体: 大きさは本体の構文木で見る (アイテムは囲む関数のもの))
        if p.closures.contains_key(&(id as u32)) {
            if no_inline_closure {
                continue;
            }
            if let Ok(f) = fn_ast(a, id as u32) {
                let ok = acg::inline_candidate(&f) && format!("{:?}", f.block).len() <= 2400;
                if inline_log {
                    eprintln!("inline {} closure {}", if ok { "yes" } else { "no " }, p.names[id]);
                }
                if ok {
                    inl.bodies.insert(id as u32, f);
                    let it = &a.items[ii as usize];
                    if ii < p.prelude_from {
                        inl.user_lines.insert(id as u32, it.line as u32 - 1);
                    }
                }
            }
            continue;
        }
        if special.contains(&p.names[id].as_str()) {
            continue;
        }
        let it = &a.items[ii as usize];
        if it.end - it.start > 4000 {
            continue;
        }
        // (大きさはソースのトークンの数。式の Debug の長さは位置などで膨らむ)
        let ntok = lexer::tokenize(a.src[it.start..it.end].as_bytes()).len();
        if ntok > acg::MAX_TOKENS {
            continue;
        }
        if let Ok(f) = fn_ast(a, id as u32) {
            let ok = acg::inline_candidate(&f);
            if inline_log {
                eprintln!("inline {} {ntok} {}", if ok { "yes" } else { "no " }, p.names[id]);
            }
            if ok {
                inl.bodies.insert(id as u32, f);
                if ii < p.prelude_from {
                    inl.user_lines.insert(id as u32, it.line as u32 - 1);
                }
            }
        }
    }
    inl
}

fn concrete(p: &Program, id: usize) -> bool {
    p.item_of[id].is_some() && !p.sigs[id].is_generic() && p.sigs[id].has_body
}

/// 子プロセスから受け取るコンパイル結果の並び: (失敗か, 番号, 中身)
fn read_all(fd: i32) -> Vec<u8> {
    let mut out = vec![];
    let mut buf = vec![0u8; 1 << 16];
    loop {
        // SAFETY: buf は buf.len() byte ある
        let n = unsafe { c_read(fd, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    out
}

fn write_all(fd: i32, mut d: &[u8]) {
    while !d.is_empty() {
        // SAFETY: d は d.len() byte ある
        let n = unsafe { c_write(fd, d.as_ptr(), d.len()) };
        if n <= 0 {
            return;
        }
        d = &d[n as usize..];
    }
}

impl Session {
    /// 関数 id のキャッシュのキー: 本体のハッシュ + 自分と参照先のシグネチャ + 名前で現れる struct (レイアウトごと) + const

    /// closed: prelude の関数で、ユーザの関数を呼びえないもの (総称でないか、型引数が prelude の型だけ)。参照の
    /// シグネチャは prelude のものだけを入れる (名前の最後で多めに取ると、ユーザの `Acc::push` などが入って、同じ prelude の
    /// 関数がクレートごとに違うキーになり、どのクレートでもコンパイルし直していた)
    fn fn_key_r(&self, a: &Analysis, id: usize, closed: Option<&[bool]>) -> Key {
        // (the program's non-generic type aliases: a constant's cast through one (`257u32 as Lane`) changes its
        // value, and a function that uses the constant, when the alias does. Order-independent)
        let aliases = || {
            let mut k: Key = 0;
            for (n, (g, t)) in &a.prog.aliases {
                if g.is_empty() {
                    let mut h = KeyHasher::new();
                    h.add(n);
                    h.add(t);
                    k ^= h.finish();
                }
            }
            k
        };
        let ii = a.prog.item_of[id].unwrap() as usize;
        let pf = a.prog.prelude_from as usize;
        // (a closed fn's key holds the program's fmt imports only when its body calls a `fmt` method (#256): the mask
        // is the whole program's, which a crate's keys would otherwise take from its consumer)
        let fmt_mask = || a.src.get(a.items[ii].start..a.items[ii].end).is_some_and(|t| t.contains(".fmt")).then_some(a.prog.fmt_imports);
        // 閉じた依存するクレートの関数 (と、型引数に依存するクレートの型を持つ prelude の関数): 本体・自分のシグネチャと、
        // 見えるクレートの中の参照先だけ (prelude はコンパイラで決まる、ほかのクレートのものは呼べない)
        if let Some(u) = closed.filter(|u| ii < pf || u.iter().any(|&x| x)) {
            let in_u = |item: Option<u32>| item.is_some_and(|ri| a.crate_of_item(ri as usize).is_some_and(|c| u[c]));
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(&3u8);
            h.add(&(self.opts.opt as u8));
            h.add(&lint_bits(&a.prog));
            h.add(&(self.opts.backend_id()));
            h.add(&a.items[ii].hash);
            h.add(&a.sig_key(id as u32));
            let uk = {
                let mut uh = KeyHasher::new();
                uh.add(u);
                uh.finish() as u64 | 1 << 63
            };
            // (a group none of whose fns the crate sees adds nothing, as no fn of it did before the groups: a name in
            // the crate that only a consumer's fn has does not change the crate's keys. The parts are added in the
            // order of their values, not of the groups' numbers, which follow where the program first meets each
            // name: a consumer that names one before the crate does would reorder them)
            let mut parts: Vec<Key> = a.refs[id]
                .iter()
                .filter_map(|&g| {
                    self.group_key(a, g, uk, &|r, h| {
                        let seen = in_u(a.prog.item_of[r as usize]);
                        if seen {
                            h.add(&a.sig_key(r));
                        }
                        seen
                    })
                })
                .collect();
            parts.sort_unstable();
            for k in &parts {
                h.add(k);
            }
            for &st in &a.srefs[id] {
                let b = a.prog.base_of(st);
                if in_u(a.prog.struct_item.get(b as usize).copied()) {
                    h.add(&a.prog.structs[st as usize].deep);
                }
            }
            for &c in &a.crefs[id] {
                let cd = &a.prog.consts[c as usize];
                if in_u(Some(cd.item)) {
                    h.add(&cd.name);
                    h.add(&a.items[cd.item as usize].hash);
                    a.prog.hash_ty(cd.ty, &mut h);
                }
            }
            if !a.crefs[id].is_empty() {
                h.add(&aliases());
            }
            // (the associated types of the crates it sees)
            let mut ak: Key = 0;
            for (c, &seen) in u.iter().enumerate() {
                if seen {
                    ak ^= a.assoc_keys.1.get(c).copied().unwrap_or(0);
                }
            }
            h.add(&ak);
            if let Some(m) = fmt_mask() {
                h.add(&m);
            }
            match a.prog.panic_handler {
                Some(p) => h.add(&a.prog.names[p as usize]),
                None => h.add(&0u8),
            }
            return h.finish();
        }
        let closed = closed.is_some();
        // 閉じた prelude の関数: prelude の本体とシグネチャはコンパイラ (cid) に入っているので、本体と自分のシグネチャと
        // panic の行き先 (`#[panic_handler]`) だけで決まる (参照先を名前の最後で多めに取ると、クレートごとに選ぶ prelude が
        // 違うのでキーが変わり、同じ関数をクレートごとにコンパイルし直していた)
        if closed && ii >= pf {
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(&2u8);
            h.add(&(self.opts.opt as u8));
            h.add(&lint_bits(&a.prog));
            h.add(&(self.opts.backend_id()));
            h.add(&a.items[ii].hash);
            h.add(&a.sig_key(id as u32));
            if let Some(m) = fmt_mask() {
                h.add(&m);
            }
            match a.prog.panic_handler {
                Some(p) => h.add(&a.prog.names[p as usize]),
                None => h.add(&0u8),
            }
            return h.finish();
        }
        let mut h = KeyHasher::new();
        h.add(&self.cid);
        h.add(&1u8);
        h.add(&(self.opts.opt as u8));
        h.add(&lint_bits(&a.prog));
        h.add(&(self.opts.backend_id()));
        h.add(&a.items[ii].hash);
        // (with traits of one name in two modules, which impl a method call takes depends on the traits in scope at the
        // item: a `use` changed elsewhere changes it, with the body the same)
        if let Some(vis) = a.prog.traits_at_item(ii as u32) {
            h.add(&vis.join(","));
        }
        // (and the fmt traits the program imports, which `x.fmt(f)` takes its trait from (#256): a `use std::fmt::Debug;`
        // changed elsewhere changes the call, the body the same; also an inlined body's)
        h.add(&a.prog.fmt_imports);
        if a.prog.closures.contains_key(&(id as u32)) {
            h.add(&a.prog.names[id]);
        }
        h.add(&a.sig_key(id as u32));
        let each = |r: u32, h: &mut KeyHasher| -> bool {
            if closed && a.prog.item_of[r as usize].is_some_and(|ri| (ri as usize) < pf) {
                return false;
            }
            h.add(&a.sig_key(r));
            // (展開しうるユーザの関数は、本体と行も効く)
            if let Some(&lb) = a.inl.user_lines.get(&r) {
                if let Some(ri) = a.prog.item_of[r as usize] {
                    h.add(&a.items[ri as usize].hash);
                    h.add(&lb);
                    // (and the traits in scope there, which the inlined body's method calls resolve with; and those of
                    // the user fns inlined into it in turn, inlining going two deep)
                    if !a.prog.trait_scopes.is_empty() {
                        if let Some(vis) = a.prog.traits_at_item(ri) {
                            h.add(&vis.join(","));
                        }
                        for r2 in a.ref_fns(r as usize) {
                            if a.inl.user_lines.contains_key(&r2) {
                                if let Some(vis) = a.prog.item_of[r2 as usize].and_then(|ri2| a.prog.traits_at_item(ri2)) {
                                    h.add(&vis.join(","));
                                }
                            }
                        }
                    }
                }
            }
            true
        };
        for &g in &a.refs[id] {
            if let Some(k) = self.group_key(a, g, closed as u64, &each) {
                h.add(&k);
            }
        }
        for &st in &a.srefs[id] {
            if closed && !a.prog.is_prelude_ty(Ty::Struct(st)) {
                continue;
            }
            h.add(&a.prog.structs[st as usize].deep);
        }
        for &c in &a.crefs[id] {
            let cd = &a.prog.consts[c as usize];
            h.add(&cd.name);
            h.add(&a.items[cd.item as usize].hash);
            a.prog.hash_ty(cd.ty, &mut h);
        }
        // (and a cast in the body's own constants: `::<{ 257u32 as Lane }>`)
        h.add(&aliases());
        // (and the program's associated types: `type Out = u32;` changed to `u64`, or `&'a u32` to `&'static u32`,
        // changes what `<() as Foo>::Out` is with the body the same)
        h.add(&a.assoc_keys.0);
        h.finish()
    }

    /// A group's part of a fn's cache key (fn_key_r): what `each` adds of each of its fns, in their order, or None when
    /// it adds nothing of any (each says whether it added). The same for every fn that refers to the group, so it is
    /// made once per analysis and `kind` (which fns `each` keeps: 0 for a key that keeps every fn, 1 for one that leaves
    /// out the fns before the prelude's (the program's own), a universe's hash with the top bit for a closed crate's)
    fn group_key(&self, a: &Analysis, g: u32, kind: u64, each: &dyn Fn(u32, &mut KeyHasher) -> bool) -> Option<Key> {
        // (0: not made yet; a made key is never 0, see below)
        let got = {
            let ks = a.group_keys.lock().unwrap();
            if kind < 2 { ks.0.get(g as usize).map_or(0, |k| k[kind as usize]) } else { ks.1.get(&(g, kind)).copied().unwrap_or(0) }
        };
        if got != 0 {
            return (got != 1).then_some(got);
        }
        // (made outside the lock: each takes the sig keys' own)
        let mut h = KeyHasher::new();
        let mut any = false;
        for &r in &a.prog.ref_groups[g as usize] {
            any |= each(r, &mut h);
        }
        // (kept as 1 when nothing was added, and as a key that is neither 0 nor 1 otherwise)
        let k = if any { h.finish().max(2) } else { 1 };
        let mut ks = a.group_keys.lock().unwrap();
        if kind < 2 {
            if ks.0.is_empty() {
                ks.0.resize(a.prog.ref_groups.len(), [0; 2]);
            }
            ks.0[g as usize][kind as usize] = k;
        } else {
            ks.1.insert((g, kind), k);
        }
        any.then_some(k)
    }

    /// 関数かインスタンスのキー (インスタンスは元の関数のキー + 型引数)
    fn key_of(&self, a: &Analysis, insts: &Mutex<Insts>, id: u32) -> Key {
        let g = insts.lock().unwrap();
        let body = g.body_of(id) as usize;
        let closed = a.closed_univ(&*g, id);
        let k = self.fn_key_r(a, body, closed.as_ref().map(|v| v.as_slice()));
        match g.get(id) {
            None => k,
            Some(inst) => {
                let mut h = KeyHasher::new();
                h.add(&k);
                h.add(&7u8);
                for &t in &inst.args {
                    a.prog.hash_ty(t, &mut h);
                }
                h.finish()
            }
        }
    }

}

/// An error at an item's first line about a name it does not find (`L:1: cannot find type \`X\``): at the line of the
/// item where X is first written instead (a field, a parameter), when that is another line
fn name_line(e: &str, src: &str, items: &[SrcItem]) -> String {
    let Some((l, rest)) = e.split_once(":1: ") else { return e.to_string() };
    let Ok(line) = l.parse::<usize>() else { return e.to_string() };
    // (`\u{5}word\u{5}msg`: at the first whole word `word` of the item, after its attributes; an empty word: at the item's
    // first token after them)
    if let Some((w, m)) = rest.strip_prefix('\u{5}').and_then(|r| r.split_once('\u{5}')) {
        let at = items.iter().find(|it| it.line == line).and_then(|it| {
            let text = src.get(it.start..it.end.min(src.len()))?;
            let from = sig_part(text, "f")?;
            if w.is_empty() {
                return Some(it.start + from);
            }
            let b = text.as_bytes();
            let id = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
            text[from..].match_indices(w).map(|(i, _)| it.start + from + i).find(|&i| !id(src.as_bytes()[i - 1]) && src.as_bytes().get(i + w.len()).is_none_or(|&c| !id(c))).filter(|_| !b.is_empty())
        });
        let Some(at) = at else { return format!("{line}:1: {m}") };
        let col = at - src[..at].rfind('\n').map_or(0, |p| p + 1) + 1;
        return format!("{}:{col}: {m}", src[..at].matches('\n').count() + 1);
    }
    // (`\u{6}part\u{6}msg`: at that part of the method's signature, as sig_part reads it)
    if let Some((part, m)) = rest.strip_prefix('\u{6}').and_then(|r| r.split_once('\u{6}')) {
        let at = items.iter().find(|it| it.line == line).and_then(|it| Some(it.start + sig_part(src.get(it.start..it.end.min(src.len()))?, part)?));
        let Some(at) = at else { return format!("{line}:1: {m}") };
        let col = at - src[..at].rfind('\n').map_or(0, |p| p + 1) + 1;
        return format!("{}:{col}: {m}", src[..at].matches('\n').count() + 1);
    }
    let name = ["cannot find type `", "cannot find trait `", "cannot find value `", "cannot find struct `", "the name `", "enum discriminant overflowed (E0370): `", "trait objects without an explicit `dyn` are deprecated: `dyn "]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
        .and_then(|r| r.split('`').next());
    let (Some(name), Some(it)) = (name, items.iter().find(|it| it.line == line)) else { return e.to_string() };
    // (the line of the name as a word in an item's text)
    let found = |it: &SrcItem| {
        let text = src.get(it.start..it.end.min(src.len()))?;
        let word = |i: usize| {
            let before = text[..i].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let after = text[i + name.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            before && after
        };
        text.match_indices(name).map(|(i, _)| i).find(|&i| word(i)).map(|i| it.line + text[..i].matches('\n').count())
    };
    // (a type the item does not write itself, for "cannot find type": the self type of the impl a member is in
    // (`impl Core<XXX> { fn spawn(self) }`), or a type alias's right-hand side that the item names through the alias
    // (`type Foo = Bar;`): rustc reports it there)
    let elsewhere = || {
        if !rest.starts_with("cannot find type `") {
            return None;
        }
        if let crate::lexer::ItemKind::ImplMember(hi) = it.kind {
            if let Some(l) = items.get(hi as usize).and_then(|h| found(h)) {
                return Some(l);
            }
        }
        items.iter().filter(|j| src.get(j.start..j.end).is_some_and(|t| t.trim_start().starts_with("type ") || t.trim_start().starts_with("pub type "))).find_map(|j| found(j))
    };
    match found(it).or_else(elsewhere) {
        Some(l) => format!("{l}:1: {rest}"),
        None => e.to_string(),
    }
}

/// E0433 of an attribute written as a path, in src[start..start + len] (an item's outer attributes): "cannot find
/// module or crate `foo` in this scope" at its first segment when that is no tool (rustfmt, clippy, diagnostic, ..,
/// or one `register_tool` names) and no module of the crate's (in `src0`, the source as written); "cannot find `bar`
/// in `m`" at its second when the first is a module (which shadows a tool of its name). Not a path from `std` /
/// `core` / `crate` and the like
fn attr_path_error(src: &str, src0: &str, start: usize, len: usize, line: usize) -> Option<String> {
    let text = src.get(start..start + len)?;
    if !text.contains("::") {
        return None;
    }
    let toks = crate::lexer::tokenize(text.as_bytes());
    let tk = |k: usize| toks.get(k).map_or("", |t| &text[t.1..t.2]);
    let id = |k: usize| toks.get(k).is_some_and(|t| t.0 == 0);
    let word_in = |hay: &str, w: &str| {
        let b = hay.as_bytes();
        let ok = |c: Option<&u8>| c.is_none_or(|&c| !(c == b'_' || c.is_ascii_alphanumeric()));
        hay.match_indices(w).any(|(i, _)| ok(i.checked_sub(1).and_then(|j| b.get(j))) && ok(b.get(i + w.len())))
    };
    let module = |m: &str| src0.match_indices("mod").any(|(i, _)| {
        let b = src0.as_bytes();
        (i == 0 || !(b[i - 1] == b'_' || b[i - 1].is_ascii_alphanumeric())) && src0[i + 3..].starts_with(char::is_whitespace) && {
            let r = src0[i + 3..].trim_start();
            r.starts_with(m) && !r[m.len()..].starts_with(|c: char| c == '_' || c.is_alphanumeric())
        }
    });
    for k in 0..toks.len() {
        if !(tk(k) == "#" && tk(k + 1) == "[" && id(k + 2) && tk(k + 3) == ":" && tk(k + 4) == ":" && id(k + 5)) {
            continue;
        }
        let (first, second) = (tk(k + 2), tk(k + 5));
        if matches!(first, "std" | "core" | "alloc" | "crate" | "self" | "super" | "unsafe") {
            continue;
        }
        let tool = matches!(first, "rustfmt" | "clippy" | "rustdoc" | "rust_analyzer" | "diagnostic" | "miri") || (src0.contains("register_tool") && word_in(src0, first) && src0.contains(&format!("register_tool({first}")));
        let at = |j: usize| {
            let i = start + toks[j].1;
            (line + text[..toks[j].1].matches('\n').count(), i - src[..i].rfind('\n').map_or(0, |p| p + 1) + 1)
        };
        if module(first) {
            let (l, c) = at(k + 5);
            return Some(format!("{l}:{c}: cannot find `{second}` in `{first}` (E0433)"));
        }
        if !tool {
            let (l, c) = at(k + 2);
            return Some(format!("{l}:{c}: cannot find module or crate `{first}` in this scope (E0433)"));
        }
    }
    None
}

/// The outer attributes an item's text starts with (`#[..]` groups, with the space between them)
fn leading_attrs(t: &str) -> &str {
    let b = t.as_bytes();
    let mut i = 0;
    while b.get(i) == Some(&b'#') && b.get(i + 1) == Some(&b'[') {
        let mut d = 0;
        while i < b.len() {
            match b[i] {
                b'[' => d += 1,
                b']' => d -= 1,
                _ => {}
            }
            i += 1;
            if d == 0 && b[i - 1] == b']' {
                break;
            }
        }
        // (the space and the comments after it: `#[deny(x)] // note`)
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if b[i..].starts_with(b"//") {
                i += b[i..].iter().position(|&c| c == b'\n').unwrap_or(b.len() - i);
            } else if b[i..].starts_with(b"/*") {
                i += t[i + 2..].find("*/").map_or(b.len() - i, |k| k + 4);
            } else {
                break;
            }
        }
    }
    &t[..i]
}

/// The lint attributes of an item's text that name the lint n of the `unused` group (`unused_variables`; or `unused`,
/// `warnings`), in order, past literals and comments: where each starts, the end of what it covers (all of the item for
/// one of the item's own, before its first other token; the statement, arm or field for one inside), and its level
/// and names (lint_attr)
fn lint_attrs(text: &str, n: &str) -> Vec<(usize, usize, u8, u8)> {
    let b = text.as_bytes();
    let mut out = vec![];
    let (mut i, mut outer) = (0, true);
    while i < b.len() {
        // (a literal ends the item's own attributes; a comment does not)
        if let Some(e) = literal_or_comment_end(b, i) {
            outer = outer && b[i] == b'/';
            i = e.max(i + 1);
            continue;
        }
        if !(b[i] == b'#' && b.get(i + 1) == Some(&b'[')) {
            outer = outer && b[i].is_ascii_whitespace();
            i += 1;
            continue;
        }
        let at = i;
        let Some(close) = attr_close(b, at + 1) else { break };
        i = close + 1;
        let attr: String = text[at + 2..close].chars().filter(|c| !c.is_whitespace()).collect();
        if let Some((level, names)) = lint_attr(&attr, n) {
            out.push((at, if outer { b.len() } else { scope_end(b, i) }, level, names));
        }
    }
    out
}

/// The `]` of the attribute whose `[` is at b[i], past the literals inside (`#[doc = "]"]`)
fn attr_close(b: &[u8], mut i: usize) -> Option<usize> {
    let mut d = 0i32;
    while i < b.len() {
        if let Some(e) = literal_or_comment_end(b, i) {
            i = e.max(i + 1);
            continue;
        }
        d += match b[i] {
            b'[' => 1,
            b']' => -1,
            _ => 0,
        };
        if d == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// A lint attribute's level for the lint n of the `unused` group (its text inside `#[..]`, without spaces), when it
/// names n, `unused` or `warnings`: 0 allow / expect / warn, 1 deny, 2 forbid (only a `forbid` of the lint itself is
/// not undone by an `allow`: a group's lets a member be allowed); and what it names, as bits: 1 n or `unused`, 2
/// `warnings`, 4 (a level 0 that is `warn`)
fn lint_attr(attr: &str, n: &str) -> Option<(u8, u8)> {
    let (name, rest) = attr.split_once('(')?;
    let lints: Vec<&str> = rest.trim_end_matches(')').split(',').filter(|l| *l == n || matches!(*l, "unused" | "warnings")).collect();
    let level = match name {
        _ if lints.is_empty() => return None,
        "allow" | "expect" | "warn" => 0,
        "deny" => 1,
        "forbid" if lints.contains(&n) => 2,
        "forbid" => 1,
        _ => return None,
    };
    let names = lints.iter().any(|l| *l != "warnings") as u8 | (lints.contains(&"warnings") as u8) << 1 | ((name == "warn") as u8) << 2;
    Some((level, names))
}

/// The end of the statement, arm or field that starts at `i`: its `;` or `,`, the end of the block it ends with
/// (unless `else`, `.`, `?` or an operator goes on after it), or the end of the block around it
fn scope_end(b: &[u8], mut i: usize) -> usize {
    let mut d = 0i32;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'\'' if b.get(i + 2) == Some(&b'\'') => i += 2,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => d += 1,
            b')' | b']' | b'}' => {
                d -= 1;
                if d < 0 {
                    return i;
                }
                if d == 0 && b[i] == b'}' {
                    let j = (i + 1..b.len()).find(|&j| !b[j].is_ascii_whitespace()).unwrap_or(b.len());
                    let goes_on = matches!(b.get(j), Some(b';' | b',' | b'.' | b'?' | b'+' | b'-' | b'*' | b'/' | b'%' | b'=' | b'<' | b'>' | b'!' | b'&' | b'^'))
                        || b[j..].starts_with(b"else")
                        || b[j..].starts_with(b"as ");
                    if !goes_on {
                        return i + 1;
                    }
                }
            }
            b';' | b',' if d == 0 => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Where a part of a fn's signature starts in its item's text, as rustc points at it: `f` the signature (after the
/// attributes), `p{i}` the `i`th parameter (a receiver itself, another parameter's type), `r` the result's type
fn sig_part(text: &str, part: &str) -> Option<usize> {
    let toks = crate::lexer::tokenize(text.as_bytes());
    let tk = |k: usize| &text[toks[k].1..toks[k].2];
    // (the attributes: `#` and its bracketed group)
    let mut k = 0;
    while k + 1 < toks.len() && tk(k) == "#" && tk(k + 1) == "[" {
        let mut d = 0;
        k += 1;
        while k < toks.len() {
            match tk(k) {
                "[" => d += 1,
                "]" => d -= 1,
                _ => {}
            }
            k += 1;
            if d == 0 {
                break;
            }
        }
    }
    if part == "f" {
        return toks.get(k).map(|t| t.1);
    }
    // (the parameters' `(`: after `fn` (not `pub(crate)`'s) and outside the generics' `<..>` (not a bound's `Fn()`))
    let from = (k..toks.len()).find(|&j| tk(j) == "fn").map_or(k, |j| j + 1);
    let open = {
        let mut angle = 0i32;
        (from..toks.len()).find(|&j| {
            match tk(j) {
                "<" => angle += 1,
                ">" if !(j > 0 && tk(j - 1) == "-" && toks[j - 1].2 == toks[j].1) => angle -= 1,
                "(" if angle <= 0 => return true,
                _ => {}
            }
            false
        })?
    };
    // (the parameters: split at the commas outside brackets, `->` not closing one)
    let (mut d, mut start, mut params, mut close) = (0i32, open + 1, vec![], None);
    let mut j = open;
    while j < toks.len() {
        match tk(j) {
            "(" | "[" | "<" | "{" => d += 1,
            ">" if j > 0 && tk(j - 1) == "-" && toks[j - 1].2 == toks[j].1 => {}
            ")" | "]" | ">" | "}" => {
                d -= 1;
                if d == 0 {
                    if start < j {
                        params.push((start, j));
                    }
                    close = Some(j);
                    break;
                }
            }
            "," if d == 1 => {
                params.push((start, j));
                start = j + 1;
            }
            _ => {}
        }
        j += 1;
    }
    let close = close?;
    if part == "r" {
        let a = (close + 2 < toks.len() && tk(close + 1) == "-" && tk(close + 2) == ">").then_some(close + 3)?;
        return toks.get(a).map(|t| t.1);
    }
    let (a, b) = *params.get(part.strip_prefix('p')?.parse::<usize>().ok()?)?;
    // (the type after the pattern's `:`, not a path's `::`; a receiver without one (`&self`), itself)
    let colon = (a..b).find(|&c| tk(c) == ":" && (c + 1 >= b || tk(c + 1) != ":") && (c == a || tk(c - 1) != ":"));
    Some(toks[colon.filter(|&c| c + 1 < b).map_or(a, |c| c + 1)].1)
}

/// compile_id と、コード生成が要求した (総称関数の) インスタンス
fn compile_id_req(a: &Analysis, insts: &Mutex<Insts>, id: u32) -> R<(Blob, Vec<u32>)> {
    let body = insts.lock().unwrap().body_of(id);
    // (本体の構文木は本体ごとに 1 回だけ読む: 総称関数のインスタンスごとに読み直していた (コンパイルの 10%))
    let mut asts = a.asts.lock().unwrap();
    if !asts.contains_key(&body) {
        let f = fn_ast(a, body)?;
        asts.insert(body, f);
    }
    let f = asts.get(&body).unwrap();
    let ii = a.prog.item_of[body as usize].unwrap() as usize;
    let it = &a.items[ii];
    // (acg のエラーはファイルの中の `行:桁:` を持つ。持たないものはアイテムの先頭の行にする)
    let lb = {
        let g = insts.lock().unwrap();
        a.line_base_for(&*g, id).unwrap_or(it.line as u32 - 1)
    };
    // (a dependency crate's function has its lines from that crate's start, tagged (line_base_for): its errors are put
    // back at the lines of the combined source)
    let shift = {
        let g = insts.lock().unwrap();
        a.line_shift(&*g, id)
    };
    let l0 = insts.lock().unwrap().fx_log.len();
    let (c, req) = acg::compile(&a.prog, insts, id, f, lb, &a.inl)
        .map_err(|e| {
            if acg::has_loc(&e) {
                return untag_lines(&e, shift);
            }
            // (an error with no position of its own in a closure's body (its parameters' patterns): at the closure,
            // as rustc reports it, not at the line of the item it is in)
            let at = a.prog.closures.get(&body).and_then(|cd| {
                let encl = a.encl.lock().unwrap();
                match encl.get(&(ii as u32)).and_then(|cs| cs.get(cd.k as usize)).map(|c| &c.0) {
                    Some(Expr::Closure(.., p)) if p.line != 0 => Some((lb + p.line, p.col + 1)),
                    _ => None,
                }
            });
            match at {
                Some((l, c)) => format!("{l}:{c}: in `{}`: {e}", a.prog.names[body as usize]),
                None => format!("{}:1: in `{}`: {e}", it.line, a.prog.names[body as usize]),
            }
        })?;
    // (what it told the closures it makes goes with its Blob: one read back (a cache, another process) tells them too)
    let clo_facts = insts.lock().unwrap().facts_since(l0);
    // (check は機械語にしない。ビルドは記録をバックエンドで機械語にする)
    if a.inl.check_only {
        return Ok((Blob { clo_facts, ..acg::Compiled::empty_blob() }, req));
    }
    lower_blob(a, c, req).map(|(b, req)| (Blob { clo_facts, ..b }, req))
}

/// What `check` keeps of a checked fn: no machine code, only the relocations to the instances it requested
fn check_blob(b: Blob, req: &[u32]) -> Blob {
    let relocs = req.iter().map(|&i| acg::BlobReloc { offset: 0, kind: 4, addend: 0, target: acg::Target::Func(i) }).collect();
    Blob { relocs, ..b }
}

/// Whether the instances ids (of a forked child's) can be sent to the parent: their type arguments have a written form
/// the cache's encoding takes (types_enc: not a type parameter, not one only inference has)
fn sendable(a: &Analysis, g: &Insts, ids: &[u32]) -> bool {
    // (not a closure's fn-pointer instance (request_fnptr): the encoding writes it as the closure's own instance, which
    // the parent would request instead)
    ids.iter().all(|&i| g.get(i).is_none_or(|inst| !inst.fnptr && inst.args.iter().all(|&t| syn_sendable(&a.prog.ty_syn(t)))))
}

/// Whether this process has one thread: a fork copies only the calling thread, and a lock another thread holds (an
/// interner's) would stay held in the child. A host that runs Session beside other threads checks in process
pub(crate) fn single_threaded() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Threads:")).and_then(|l| l[8..].trim().parse::<u32>().ok()))
        == Some(1)
}

/// How many times this process has forked for parallel work (summaries, checks, builds): tests read it to see that a
/// threaded host did not fork
static FORKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn forks() -> usize {
    FORKS.load(std::sync::atomic::Ordering::Relaxed)
}

/// fork, counted (FORKS)
pub(crate) fn fork_counted() -> i32 {
    FORKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: the callers' children only work and write, then _exit
    unsafe { c_fork() }
}

/// Waits for a child, again when a signal interrupts the wait
fn wait_child(pid: i32) {
    let mut st = 0i32;
    loop {
        // SAFETY: st is an int
        if unsafe { c_waitpid(pid, &mut st, 0) } == pid || std::io::Error::last_os_error().raw_os_error() != Some(4) {
            return;
        }
    }
}

/// In a forked child: a panic that unwinds past this exits the child at once, so it never runs its parent's frames
struct ChildExit;

impl Drop for ChildExit {
    fn drop(&mut self) {
        // SAFETY: the child only exits
        unsafe { c_exit(1) };
    }
}

fn syn_sendable(t: &crate::types::TySyn) -> bool {
    match t {
        crate::types::TySyn::Prim(Ty::Vec(_)) => true,
        crate::types::TySyn::Prim(p) => crate::types::TYS.contains(p),
        crate::types::TySyn::Ref(_, i) | crate::types::TySyn::Array(i, _) | crate::types::TySyn::Slice(i) | crate::types::TySyn::Ptr(_, i) | crate::types::TySyn::Assoc(i, _) => syn_sendable(i),
        crate::types::TySyn::App(_, v) | crate::types::TySyn::Tuple(v) | crate::types::TySyn::FnPtr(v) => v.iter().all(syn_sendable),
        crate::types::TySyn::Named(_) | crate::types::TySyn::Str | crate::types::TySyn::Dyn(_) | crate::types::TySyn::Infer => true,
    }
}

/// A checked fn: the instances it requested and its Blob as the cache keeps it (None when nothing needs it), or its error
type Checked = R<(Vec<u32>, Option<Vec<u8>>)>;

impl Session {
    /// Checks ids, in forked children when there are many (#101): child k checks ids[i] with i % jobs == k and sends
    /// back each one's Blob (encoded by name, as a parallel build's children do) or its error. The parent reads them
    /// back in the order of ids, which requests again the instances each one requested, so what follows (the
    /// diagnostics, the next wave) is as with one process. Only when checking (no machine code), with -j above 1
    fn check_many(&self, a: &Analysis, insts: &Mutex<Insts>, ids: &[u32], check_only: bool) -> Vec<Checked> {
        let one = |id: u32| -> Checked {
            let (b, req) = compile_id_req(a, insts, id)?;
            if self.opts.no_cache {
                return Ok((req, None));
            }
            let b = if check_only { check_blob(b, &req) } else { b };
            let mut e = vec![];
            {
                let g = insts.lock().unwrap();
                crate::build_cache::encode_blob(&b, &a.prog, &*g, &mut e);
            }
            Ok((req, Some(e)))
        };
        let per_job = std::env::var("ZUSTC_CHECK_FORK_MIN").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(32).max(1);
        let jobs = if check_only { self.opts.jobs.min(ids.len() / per_job).max(1) } else { 1 };
        if jobs <= 1 || !single_threaded() {
            return ids.iter().map(|&id| one(id)).collect();
        }
        let mut kids: Vec<(i32, i32)> = vec![];
        for k in 0..jobs {
            let mut fds = [0i32; 2];
            // SAFETY: fds は 2 つの int
            if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                continue;
            }
            // SAFETY: 子は検査して書くだけで _exit する
            let pid = fork_counted();
            if pid < 0 {
                unsafe { c_close(fds[0]) };
                unsafe { c_close(fds[1]) };
                continue;
            }
            if pid == 0 {
                let _exit_on_panic = ChildExit;
                unsafe { c_close(fds[0]) };
                let mut buf = vec![];
                let mut i = k;
                while i < ids.len() {
                    crate::build_cache::put_uv(&mut buf, i as u64);
                    let (p0, l0) = {
                        let g = insts.lock().unwrap();
                        (g.pending.len(), g.fx_log.len())
                    };
                    match compile_id_req(a, insts, ids[i]) {
                        Ok((b, req)) => {
                            let mut e = vec![];
                            {
                                let g = insts.lock().unwrap();
                                // (the instances it made, in the order it made them (what the parent's pending gets
                                // when it checks the fn itself), then the others it requested: lowering makes some that
                                // it does not return in req)
                                let mut all: Vec<u32> = g.pending[p0..].to_vec();
                                all.extend(req.iter().filter(|i| !all.contains(i)).copied().collect::<Vec<u32>>());
                                if sendable(a, &*g, &all) {
                                    crate::build_cache::encode_blob(&check_blob(b, &all), &a.prog, &*g, &mut e);
                                }
                            }
                            // (what cannot be sent is checked again by the parent)
                            if e.is_empty() {
                                buf.push(2);
                                crate::build_cache::put_uv(&mut buf, 0);
                                i += jobs;
                                continue;
                            }
                            buf.push(0);
                            crate::build_cache::put_uv(&mut buf, e.len() as u64);
                            buf.extend_from_slice(&e);
                        }
                        Err(m) => {
                            // (and what it did before it failed, which the fn checked here leaves behind: the instances
                            // it made, pending, and what it told the closures it makes)
                            let mut e = vec![];
                            {
                                let g = insts.lock().unwrap();
                                let made = g.pending[p0..].to_vec();
                                if sendable(a, &*g, &made) {
                                    let b = Blob { clo_facts: g.facts_since(l0), ..acg::Compiled::empty_blob() };
                                    crate::build_cache::encode_blob(&check_blob(b, &made), &a.prog, &*g, &mut e);
                                }
                            }
                            if e.is_empty() {
                                buf.push(2);
                                crate::build_cache::put_uv(&mut buf, 0);
                                i += jobs;
                                continue;
                            }
                            buf.push(1);
                            crate::build_cache::put_uv(&mut buf, m.len() as u64);
                            buf.extend_from_slice(m.as_bytes());
                            crate::build_cache::put_uv(&mut buf, e.len() as u64);
                            buf.extend_from_slice(&e);
                        }
                    }
                    i += jobs;
                }
                write_all(fds[1], &buf);
                unsafe { c_close(fds[1]) };
                unsafe { c_exit(0) };
            }
            unsafe { c_close(fds[1]) };
            kids.push((pid, fds[0]));
        }
        let mut got: Vec<Option<Result<Vec<u8>, (String, Vec<u8>)>>> = (0..ids.len()).map(|_| None).collect();
        for &(pid, fd) in &kids {
            let d = read_all(fd);
            unsafe { c_close(fd) };
            wait_child(pid);
            let mut r = crate::build_cache::Rd { d: &d, pos: 0 };
            while r.pos < d.len() {
                let (Some(i), Some(fail), Some(n)) = (r.uv(), r.u8(), r.uv()) else { break };
                let Some(body) = r.take(n as usize) else { break };
                let v = if fail == 2 {
                    continue;
                } else if fail == 0 {
                    Ok(body.to_vec())
                } else {
                    let m = String::from_utf8_lossy(body).into_owned();
                    let Some(n2) = r.uv() else { break };
                    let Some(made) = r.take(n2 as usize) else { break };
                    Err((m, made.to_vec()))
                };
                if let Some(slot) = got.get_mut(i as usize) {
                    *slot = Some(v);
                }
            }
        }
        // (in order: reading a Blob back requests the instances it requested. What a child did not send (it failed, or
        // was not started) is checked here)
        got.into_iter()
            .zip(ids)
            .map(|(g, &id)| match g {
                Some(Ok(d)) => match crate::build_cache::decode_blob(&d, &a.prog, insts) {
                    Some(b) => {
                        let req = b.relocs.iter().filter_map(|r| match r.target { acg::Target::Func(i) => Some(i), _ => None }).collect();
                        Ok((req, if self.opts.no_cache { None } else { Some(d) }))
                    }
                    None => one(id),
                },
                // (what the failed fn left behind, read back; when it cannot be (a name only the child knows), the fn is
                // checked here, which leaves it as the serial check does: requests are made once, so what the reading
                // requested before it failed is not requested twice)
                Some(Err((m, made))) => match crate::build_cache::decode_blob(&made, &a.prog, insts) {
                    Some(_) => Err(m),
                    None => one(id),
                },
                None => one(id),
            })
            .collect()
    }
}

/// 記録を機械語にして Blob を作る (バックエンドなしでは作れない)
#[cfg(not(feature = "build"))]
fn lower_blob(_: &Analysis, _: acg::Compiled, _: Vec<u32>) -> R<(Blob, Vec<u32>)> { Err("internal: zust-core was built without the backend (feature `build`)".into()) }

impl Session {
    /// An error of the analysis at a line past the program's own (an item hoisted out of a fn body, prep::HOIST_MARK):
    /// at the line it was written on. The text is preprocessed again for it, only when there is such an error
    #[cold]
    #[inline(never)]
    pub(crate) fn unhoist_analysis_err(&self, src0: &str, e: String) -> String {
        let d = Diagnostic::parse(&e);
        let user_lines = src0.bytes().filter(|&c| c == b'\n').count() + 1;
        if d.line <= user_lines || !e.trim_start().starts_with(|c: char| c.is_ascii_digit()) || !hoisted_err_holds(&d.msg) {
            return e;
        }
        // (the analysis's configuration: the crate's features, cfgs and env when it has them)
        let cfg = self.opts.cfg_of(self.ctx.features.as_ref().unwrap_or(&self.opts.features), &self.ctx.cfgs, &self.ctx.env);
        let stripped = crate::cfg::strip(src0, &cfg);
        let s = stripped.as_deref().unwrap_or(src0);
        let renamed = crate::prep::std_renames(s);
        let s = renamed.as_deref().unwrap_or(s);
        let mut crates = self.ctx.deps.clone();
        crates.extend(self.ctx.lib.iter().cloned());
        crates.extend(self.ctx.externs.iter().cloned());
        let Ok(p) = crate::prep::prep_full(s, &crates, &mut vec![], Some(&cfg)) else { return e };
        match crate::prep::hoisted_line(&crate::prep::marked(p), d.line) {
            Some(l) => with_line(&e, l),
            None => e,
        }
    }
}

/// An analysis error of a hoisted item that says what rustc says there: a name it cannot see (a type parameter or
/// `Self` of the fn or impl around it, a local), an import, `_` in a signature, an item never used. Not what zustc
/// does not read (`unsupported ..`, a parse error of syntax it does not know), nor a name the hoisting made (`X__h0`)
fn hoisted_err_holds(msg: &str) -> bool {
    let named = ["cannot find type `", "cannot find value `", "`Self` is only available in impls", "`self` is only allowed in impls", "unresolved import `", "the placeholder `_` is not allowed", "expected identifier, found keyword",
        // (an item's own definition: a discriminant past its repr, a union's fields and derives)
        "enum discriminant overflowed (E0370)", "field must implement `Copy`", "this trait cannot be derived for unions", "cannot find derive macro `",
        // (an item's own syntax: `struct Test { Very\n Bad }` in a fn body; any `expected ..` of its text, which is
        // moved as written: `expected `,` or `>` (found `=`)`)
        "expected `", "free static item without body",
        // (a type's syntax in a fn's signature: `Box<dyn X< : 32 >>` in a `const _: () = { .. }`'s block)
        "expected a type (found `",
        // (`#[test]` on an item in a fn body)
        "the `test` attribute may only be used on a free function",
        // (`const FOO = 1;` in a fn body)
        "missing type for `",
        // (an item's written types: an unsized type where a Sized one is needed, sized_error)
        "the size for values of type `",
        // (an impl's method against its trait's: an impl in a fn body is hoisted)
        "impl has stricter requirements than trait"];
    let unused = msg.starts_with("function `") && msg.contains("is never used") || msg.starts_with("type parameter `") && msg.ends_with("is never used (E0091)")
        // (two variants or foreign items of one name, in the item itself)
        || msg.starts_with("the name `") && msg.ends_with("is defined multiple times (E0428)");
    // (an impl's associated type against its trait's bound (E0477): an impl in a fn body is hoisted)
    let unmet = msg.starts_with("the type `") && msg.contains("does not fulfill the required lifetime");
    // (a derive's field or a field's type without the bound it needs (derive_field_error, wf_error): a local
    // `#[derive(Eq)] struct S(f64);` too)
    let bounds = ["the trait bound `", "binary operation `==` cannot be applied to type `", "can't compare `"].iter().any(|n| msg.starts_with(n))
        || (msg.starts_with('`') && (msg.contains("` doesn't implement `Debug`") || msg.contains("` is not an iterator")));
    !msg.contains("__h") && !msg.contains("unsupported") && (named.iter().any(|n| msg.starts_with(n)) || msg.ends_with("is not dyn compatible (E0038)") || (msg.starts_with("cannot find constant `") && !msg.starts_with("cannot find constant `_`")) || unused || bounds || unmet)
}

/// A type check's error in an item hoisted out of a fn body that holds wherever the item is: a type nothing declares,
/// a local where a constant goes (E0435), a tuple pattern's fields (E0023) or one of a unit or struct variant (E0532,
/// E0164), an unsafe call outside `unsafe` (E0133). Not one naming `Self` (of an impl around the item, which the
/// hoisting leaves behind), a name the hoisting made, or what zustc does not read
fn hoisted_check_err_holds(msg: &str) -> bool {
    const HOLDS: [&str; 5] = [
        "cannot find type `",
        "attempt to use a non-constant value in a constant",
        "this pattern has ",
        "expected tuple struct or tuple variant, found ",
        "call to unsafe function",
    ];
    HOLDS.iter().any(|h| msg.starts_with(h)) && !msg.contains("Self") && !msg.contains("__h") && !msg.contains("unsupported")
}

/// E0170 (bindings_with_variant_name) taken out of errs where an attribute of an item's (its leading ones) or of the
/// crate's names the lint (an `allow`, read for all of the program)
#[cold]
#[inline(never)]
fn allow_variant_bindings(src: &str, errs: &mut Vec<String>) {
    let Ok(items) = crate::lexer::split(src) else { return };
    // (the crate's own, `#![allow(..)]`, is an item of its own)
    let allowed = items.iter().any(|it| {
        src.get(it.start..it.end).is_some_and(|t| t.starts_with('#') && if t.starts_with("#!") { t } else { leading_attrs(t) }.contains("bindings_with_variant_name"))
    });
    if allowed {
        errs.retain(|e| !e.ends_with("(E0170)"));
    }
}

/// The items of a `<..>` list starting at the `<` at byte k of t, split at its top-level commas, and the byte after
/// its `>` (None: no list there)
fn angle_items(t: &str, k: usize) -> Option<(Vec<&str>, usize)> {
    let b = t.as_bytes();
    if b.get(k) != Some(&b'<') {
        return None;
    }
    let mut d = 0i32;
    let mut start = k + 1;
    let mut out = vec![];
    let mut i = k;
    while i < b.len() {
        match b[i] {
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            // (a char or byte literal, `b'>'`, `'\''`, and a string: their brackets close nothing)
            b'\'' | b'"' if quote_end(b, i).is_some() => i = quote_end(b, i).unwrap_or(i),
            b'<' | b'(' | b'[' => d += 1,
            b'>' | b')' | b']' => {
                d -= 1;
                if d == 0 {
                    let piece = t[start..i].trim();
                    if !piece.is_empty() {
                        out.push(piece);
                    }
                    return Some((out, i + 1));
                }
            }
            b',' if d == 1 => {
                out.push(t[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The byte of the closing quote of a char, byte or string literal whose opening quote is at byte i of b (`'x'`,
/// `'\''`, `b']'`, `"a]"`). None for a lifetime (`'a`), which has none
fn quote_end(b: &[u8], i: usize) -> Option<usize> {
    match b.get(i)? {
        b'\'' if b.get(i + 1) == Some(&b'\\') || b.get(i + 2) == Some(&b'\'') => {
            let mut j = i + if b.get(i + 1) == Some(&b'\\') { 3 } else { 2 };
            while j < b.len() && b[j] != b'\'' {
                j += 1;
            }
            Some(j)
        }
        b'"' => {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            Some(j)
        }
        _ => None,
    }
}

/// The byte of t's first `;` outside brackets and literals (`fn f(_: [u8; 1]) where T: 'a;`: the last one; not one
/// after `b']'`)
fn top_semi(t: &str) -> Option<usize> {
    let b = t.as_bytes();
    let mut d = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' if quote_end(b, i).is_some() => i = quote_end(b, i).unwrap_or(i),
            b'(' | b'[' | b'{' => d += 1,
            b')' | b']' | b'}' => d -= 1,
            b';' if d == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The byte of a generic list item's `=` before its default (`T = fn(x: u8)`, `const N: usize = 3`), not one in
/// brackets (`Iterator<Item = u8>`)
fn top_eq(it: &str) -> Option<usize> {
    let b = it.as_bytes();
    let mut d = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' if quote_end(b, i).is_some() => i = quote_end(b, i).unwrap_or(i),
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            b'<' | b'(' | b'[' | b'{' => d += 1,
            b'>' | b')' | b']' | b'}' => d -= 1,
            b'=' if d == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The byte of the first word w in t (not part of a longer name: `where` after a space or a line break)
fn find_word(t: &str, w: &str) -> Option<usize> {
    let id = |c: char| c.is_alphanumeric() || c == '_';
    t.match_indices(w).map(|(i, _)| i).find(|&i| !t[..i].ends_with(id) && !t[i + w.len()..].starts_with(id))
}

/// The text with each identifier or lifetime that names a key of map replaced by its value, and its whitespace one
/// space where it parts two names (`&'a T`, `Vec<T>`)
fn subst_names(t: &str, map: &[(String, String)]) -> String {
    let mut out = String::new();
    let cs: Vec<char> = t.chars().collect();
    let mut i = 0;
    let id = |c: char| c.is_alphanumeric() || c == '_';
    while i < cs.len() {
        let c = cs[i];
        if id(c) || (c == '\'' && cs.get(i + 1).is_some_and(|&n| id(n))) {
            let mut j = i + 1;
            while j < cs.len() && id(cs[j]) {
                j += 1;
            }
            let w: String = cs[i..j].iter().collect();
            let v = map.iter().find(|(k, _)| *k == w).map_or(w.as_str(), |(_, v)| v.as_str());
            if out.ends_with(id) && v.starts_with(|c: char| id(c) || c == '\'') {
                out.push(' ');
            }
            out.push_str(v);
            i = j;
            continue;
        }
        if !c.is_whitespace() {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// E0276 "impl has stricter requirements than trait" for an impl's method whose where clause bounds by a lifetime
/// what the trait's method does not: a lifetime of the impl outliving another (`where 'a: 'b`), or a type outliving a
/// lifetime (`where V: 'a`), read through the impl header's trait arguments (`impl<'a, U, V> Master<'a, U, V>` for
/// `trait Master<'a, T, U>`: the trait's `where T: 'a` is the impl's `U: 'a`), with the trait's and the impl's own
/// header bounds. Not compared where a type of the signatures implies it (`x: &'a V` implies `V: 'a`), nor for the
/// method's own parameters (stricter_impl_bound, early_mismatch). At the bound
fn stricter_impl_where(src: &str, items: &[SrcItem]) -> Option<String> {
    use crate::lexer::ItemKind as K;
    let text = |it: &SrcItem| src.get(it.start..it.end).unwrap_or("");
    // (a member's text to its body or its `;`, not one in its types: `fn f(_: [u8; 1]);`)
    let member_head = |t: &str| -> usize { fn_body_start(t).or_else(|| top_semi(t)).unwrap_or(t.len()) };
    if !items.iter().any(|it| matches!(it.kind, K::ImplMember(_)) && {
        let t = text(it);
        let h = &t[..member_head(t)];
        h.contains("where") && h.contains('\'') && h.contains("fn ")
    }) {
        return None;
    }
    let word_in = |p: &str, n: &str| {
        p.match_indices(n).any(|(i, _)| {
            let id = |c: char| c.is_alphanumeric() || c == '_';
            !p[..i].ends_with(id) && !p[i + n.len()..].starts_with(id)
        })
    };
    // (a generic list's names, and the lifetime bounds its items write: `'b: 'a`, `T: 'a + Clone`)
    let names_and_bounds = |its: &[&str]| -> (Vec<String>, Vec<(String, String)>) {
        let mut names = vec![];
        let mut bounds = vec![];
        for it in its {
            let it = it.trim().strip_prefix("const ").unwrap_or(it.trim());
            // (the name and bounds before a default: not the `:` of `T = fn(x: u8)`)
            let it = it[..top_eq(it).unwrap_or(it.len())].trim();
            let (n, rest) = match it.find(':') {
                Some(c) => (it[..c].trim(), &it[c + 1..]),
                None => (it.split('=').next().unwrap_or(it).trim(), ""),
            };
            for b in rest.split('+') {
                let b = b.trim();
                if b.starts_with('\'') {
                    bounds.push((n.to_string(), b.to_string()));
                }
            }
            names.push(n.to_string());
        }
        (names, bounds)
    };
    let where_bounds = |head: &str| -> Vec<(String, String)> {
        where_preds(head).unwrap_or_default().into_iter().flat_map(|(s, bs)| bs.into_iter().filter(|b| b.starts_with('\'')).map(move |b| (s.clone(), b))).collect()
    };
    // (a generic list item's default: `T = ()`, `const N: usize = 3`; not the `=` of `Iterator<Item = u8>`)
    let default_of = |it: &str| -> Option<String> { top_eq(it).map(|i| it[i + 1..].trim().to_string()) };
    // (the generic parameters after a fn's name in its text: `fn f<T>`, not the `f` of `fn`)
    let fn_generics = |t: &str, name: &str| -> Vec<String> {
        let Some(k) = t.find("fn ") else { return vec![] };
        let r = t[k + 3..].trim_start();
        angle_items(t, t.len() - r.len() + name.len()).map(|(its, _)| names_and_bounds(&its).0).unwrap_or_default()
    };
    // (the program's traits: name, header item, parameters, header bounds, the parameters' defaults)
    let mut traits: Vec<(String, u32, Vec<String>, Vec<(String, String)>, Vec<Option<String>>)> = vec![];
    for (i, it) in items.iter().enumerate() {
        if it.kind != K::TraitHeader {
            continue;
        }
        let t = text(it);
        let head = &t[..t.find('{').unwrap_or(t.len())];
        let Some(k) = head.find("trait ") else { continue };
        let r = &head[k + 6..];
        let n_end = r.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(r.len());
        let name = r[..n_end].to_string();
        let (params, mut bounds, defaults) = match angle_items(head, k + 6 + n_end) {
            Some((its, _)) => {
                let (n, b) = names_and_bounds(&its);
                (n, b, its.iter().map(|it| default_of(it)).collect())
            }
            None => (vec![], vec![], vec![]),
        };
        bounds.extend(where_bounds(head));
        traits.push((name, i as u32, params, bounds, defaults));
    }
    if traits.is_empty() {
        return None;
    }
    for m in items {
        let K::ImplMember(h) = m.kind else { continue };
        let mt = text(m);
        let mh = &mt[..member_head(mt)];
        if !(mh.contains("where") && mh.contains('\'')) {
            continue;
        }
        let Some(fk) = mh.find("fn ") else { continue };
        let r = mh[fk + 3..].trim_start();
        let name = &r[..r.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(r.len())];
        // (the method's own generics: not compared here)
        let own = fn_generics(mh, name);
        // (the impl's header: its generics, the trait and its arguments, the self type)
        let Some(hi) = items.get(h as usize) else { continue };
        let ht = text(hi);
        let head = &ht[..ht.find('{').unwrap_or(ht.len())];
        let Some(ik) = head.find("impl") else { continue };
        let (impl_names, impl_bounds, after) = match angle_items(head, ik + 4 + head[ik + 4..].len() - head[ik + 4..].trim_start().len()) {
            Some((its, e)) => {
                let (n, b) = names_and_bounds(&its);
                (n, b, e)
            }
            None => (vec![], vec![], ik + 4),
        };
        // (the keyword `for` after a space or a line break, not the `for<'b>` of a higher-ranked type in the
        // arguments)
        let hs = &head[after..];
        let Some(f) = hs.match_indices("for").map(|(i, _)| i).find(|&i| hs[..i].ends_with(char::is_whitespace) && hs[i + 3..].starts_with(char::is_whitespace)) else { continue };
        let trait_part = hs[..f].trim();
        // (to its `where`, after a space or a line break)
        let rest = &hs[f + 3..];
        let self_ty = rest[..find_word(rest, "where").unwrap_or(rest.len())].trim();
        let tn_end = trait_part.find('<').unwrap_or(trait_part.len());
        let tname = trait_part[..tn_end].rsplit("::").next().unwrap_or("").trim();
        let Some((_, th, tparams, tbounds, tdefaults)) = traits.iter().find(|t| t.0 == tname) else { continue };
        let mut targs: Vec<String> = if tn_end < trait_part.len() { angle_items(trait_part, tn_end).map(|x| x.0).unwrap_or_default() } else { vec![] }.iter().map(|a| subst_names(a, &[])).collect();
        let mut map: Vec<(String, String)> = vec![("Self".to_string(), subst_names(self_ty, &[]))];
        map.extend(tparams.iter().cloned().zip(targs.iter().cloned()));
        // (an argument left out for a parameter with a default: the default, read through the ones before it,
        // `Rhs = Self`)
        while targs.len() < tparams.len() {
            let Some(Some(dv)) = tdefaults.get(targs.len()) else { break };
            let v = subst_names(dv, &map);
            map.push((tparams[targs.len()].clone(), v.clone()));
            targs.push(v);
        }
        if targs.len() != tparams.len() {
            continue;
        }
        // (the trait's method of that name)
        let Some(d) = items.iter().find(|d| d.kind == K::TraitMember(*th) && {
            let dt = text(d);
            dt.find("fn ").is_some_and(|k| {
                let r = dt[k + 3..].trim_start();
                r.starts_with(name) && !r[name.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
            })
        }) else {
            continue;
        };
        let dt = text(d);
        let dh = &dt[..member_head(dt)];
        // (what holds: the trait's method's and header's bounds through the arguments, the impl header's. Not the
        // trait's method's bounds of its own parameters, which are not the impl's of the same names)
        // (the impl's `Self` is its self type)
        let self_map = [("Self".to_string(), subst_names(self_ty, &[]))];
        let downs = fn_generics(dh, name);
        let of_downs = |x: &str| downs.iter().any(|o| word_in(x, o));
        let mut facts: Vec<(String, String)> =
            where_bounds(dh).into_iter().filter(|(a, b)| !of_downs(a) && !of_downs(b)).chain(tbounds.iter().cloned()).map(|(a, b)| (subst_names(&a, &map), subst_names(&b, &map))).collect();
        facts.extend(impl_bounds.iter().map(|(a, b)| (subst_names(a, &self_map), b.clone())));
        facts.extend(where_bounds(head).into_iter().map(|(a, b)| (subst_names(&a, &self_map), b)));
        // (a type outlives a lifetime where each of its parts does (RFC 1214): `[U]: 'a`, `&'b U: 'a`, `Bar<'b, T>:
        // 'a` give `U: 'a`, `'b: 'a`, `T: 'a`. Not a projection's (`U::Item: 'a` says nothing of U))
        let mut parts: Vec<(String, String)> = vec![];
        for (a, b) in &facts {
            if a.starts_with('\'') || a.contains("::") {
                continue;
            }
            let cs: Vec<char> = a.chars().collect();
            let mut i = 0;
            while i < cs.len() {
                let lt = cs[i] == '\'';
                if lt || cs[i].is_alphabetic() || cs[i] == '_' {
                    let mut j = i + 1;
                    while j < cs.len() && (cs[j].is_alphanumeric() || cs[j] == '_') {
                        j += 1;
                    }
                    let w: String = cs[i..j].iter().collect();
                    if (lt && w.len() > 1) || impl_names.contains(&w) {
                        parts.push((w, b.clone()));
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }
        facts.extend(parts);
        // (types that may imply a bound of what they name: the signatures', not the impl header's (rustc 1.97 takes
        // nothing from `for &'a T` or `Tr<'a, &'a T>` here: `where T: 'a` is stricter). As text, and parsed
        // (wf_implies: `(&'a u8, &'b u8)` implies nothing of `'a: 'b`); the text of what does not parse is read as
        // before)
        // (the trait's method's own generics renamed in its types: its `T` is not the impl header's `T`)
        let mut dmap = map.clone();
        dmap.extend(downs.iter().map(|d| (d.clone(), match d.strip_prefix('\'') {
            Some(l) => format!("'__m_{l}"),
            None => format!("__m_{d}"),
        })));
        let mut pieces: Vec<String> = sig_type_pieces(dh).iter().map(|p| subst_names(p, &dmap)).collect();
        pieces.extend(sig_type_pieces(mh).iter().map(|p| subst_names(p, &[])));
        let parse_sig = |t: &str| -> Option<crate::parse::FnSig> {
            match crate::parse::parse_file(&format!("{};", t.trim_end())).ok()?.items.into_iter().next()?.kind {
                crate::parse::ItemKind::Fn(s, _) => Some(s),
                _ => None,
            }
        };
        let (mut tys, mut loose): (Vec<crate::parse::Ty>, Vec<String>) = (vec![], vec![]);
        let fn_at = |t: &str| t.find("fn ").unwrap_or(0);
        for (sg, text_pieces) in [(subst_names(&dh[fn_at(dh)..], &dmap), sig_type_pieces(dh).iter().map(|p| subst_names(p, &dmap)).collect::<Vec<_>>()), (mh[fn_at(mh)..].to_string(), sig_type_pieces(mh).iter().map(|p| subst_names(p, &[])).collect())] {
            match parse_sig(&sg) {
                Some(fs) => tys.extend(fs.recv_ty.into_iter().chain(fs.params.into_iter().map(|p| p.1)).chain(fs.ret)),
                None => loose.extend(text_pieces),
            }
        }
        // (what the signatures' types imply of the impl's parameters and lifetimes, as facts, so the outlives below
        // go through them: `x: &'b T` gives `T: 'b`, which with the header's `'b: 'a` gives `T: 'a`)
        let lts: Vec<String> = impl_names.iter().filter(|n| n.starts_with('\'')).cloned().chain(["'static".to_string()]).collect();
        for n in &impl_names {
            for l in &lts {
                if n != l && tys.iter().any(|t| wf_implies(t, n, l)) {
                    facts.push((n.clone(), l.clone()));
                }
            }
        }
        let outlives = |x: &str, y: &str| -> bool {
            if x == y || x == "'static" {
                return true;
            }
            let (mut seen, mut work) = (vec![x.to_string()], vec![x.to_string()]);
            while let Some(z) = work.pop() {
                for (a, b) in &facts {
                    if *a == z && !seen.contains(b) {
                        if b == y || b == "'static" {
                            return true;
                        }
                        seen.push(b.clone());
                        work.push(b.clone());
                    }
                }
            }
            false
        };
        for (subj0, bound) in where_bounds(mh) {
            let subj = subst_names(&subj0, &self_map);
            let mentions_own = own.iter().any(|o| word_in(&subj, o) || word_in(&bound, o));
            // (a parameter or a lifetime by the types' structure; another type (`Vec<T>: 'a`) by their text)
            let simple = subj.starts_with('\'') || subj.chars().all(|c| c.is_alphanumeric() || c == '_');
            let implied = if simple {
                tys.iter().any(|t| wf_implies(t, &subj, &bound)) || loose.iter().any(|p| word_in(p, &subj) && word_in(p, &bound))
            } else {
                pieces.iter().any(|p| word_in(p, &subj) && word_in(p, &bound))
            };
            // (a type that names no parameter and no lifetime outlives every lifetime: `fn(u8): 'a`, `(): 'a`)
            let trivial = !subj.contains('\'') && !impl_names.iter().any(|n| word_in(&subj, n)) && !word_in(&subj, "Self");
            if mentions_own || implied || trivial {
                continue;
            }
            let held = if subj.starts_with('\'') {
                outlives(&subj, &bound)
            } else {
                facts.iter().any(|(a, b)| *a == subj && outlives(b, &bound))
            };
            if held {
                continue;
            }
            // (at the bound: the first `bound` after `subj:` in the where clause, `where T: 'a,\n U: 'a` for U's)
            let w = mh.rfind("where").unwrap_or(0);
            let wc = &mh[w..];
            let id = |c: char| c.is_alphanumeric() || c == '_';
            let off = wc
                .match_indices(&*subj0)
                .find_map(|(i, _)| {
                    let after = &wc[i + subj0.len()..];
                    let t = after.trim_start();
                    let at = !wc[..i].ends_with(|c: char| id(c) || c == '\'') && !after.starts_with(id) && t.starts_with(':') && !t.starts_with("::");
                    at.then(|| wc[i + subj0.len()..].find(&*bound).map(|o| w + i + subj0.len() + o)).flatten()
                })
                .or_else(|| wc.find(&*bound).map(|o| w + o))?;
            let line = m.line + mt[..off].matches('\n').count();
            let abs = m.start + off;
            let col = abs - src[..abs].rfind('\n').map_or(0, |k| k + 1) + 1;
            return Some(format!("{line}:{col}: impl has stricter requirements than trait (E0276)"));
        }
    }
    None
}

/// Whether a type names the type parameter n
fn ty_names(t: &crate::parse::Ty, n: &str) -> bool {
    use crate::parse::{Bound, GArg, Ty as PTy};
    let path = |p: &crate::parse::Path| {
        p.segs.iter().any(|s| {
            s.name == n
                || s.args.iter().any(|a| match a {
                    GArg::Ty(t) | GArg::Bind(_, t) => ty_names(t, n),
                    _ => false,
                })
                || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(|t| ty_names(t, n)) || r.as_deref().is_some_and(|t| ty_names(t, n)))
        })
    };
    let bounds = |bs: &[Bound]| bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if path(p)));
    match t {
        PTy::Path(q, p) => q.as_deref().is_some_and(|q| ty_names(q, n)) || path(p),
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => ty_names(i, n),
        PTy::Tuple(ts) => ts.iter().any(|t| ty_names(t, n)),
        PTy::Fn(ps, r) => ps.iter().any(|t| ty_names(t, n)) || ty_names(r, n),
        PTy::Impl(bs) | PTy::Dyn(bs) => bounds(bs),
        _ => false,
    }
}

/// Whether a type of a signature may imply `n: 'static` (wf_implies)
fn static_implies(t: &crate::parse::Ty, n: &str) -> bool {
    wf_implies(t, n, "'static")
}

/// Whether n (a type parameter or a lifetime) is a component of a type, what its outliving a lifetime makes outlive
/// it (RFC 1214): not the inputs of a projection (`<T as Has>::Item`, `T::Item`: an `Item` may hold nothing of T)
fn comp_names(t: &crate::parse::Ty, n: &str) -> bool {
    use crate::parse::{Bound, GArg, Ty as PTy};
    let lt = n.starts_with('\'');
    let path = |p: &crate::parse::Path| {
        // (`T::Item`: a projection)
        if p.segs.len() > 1 && p.segs.first().is_some_and(|s| s.name == n || s.name == "Self") {
            return false;
        }
        (!lt && p.segs.len() == 1 && p.segs[0].name == n)
            || p.segs.iter().any(|s| {
                s.args.iter().any(|a| match a {
                    GArg::Lifetime(x) => lt && x == n,
                    GArg::Ty(t) | GArg::Bind(_, t) => comp_names(t, n),
                    _ => false,
                }) || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(|t| comp_names(t, n)) || r.as_deref().is_some_and(|t| comp_names(t, n)))
            })
    };
    match t {
        PTy::Path(Some(_), _) => false,
        PTy::Path(None, p) => path(p),
        PTy::Ref(r, _, i) => (lt && r.as_deref() == Some(n)) || comp_names(i, n),
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => comp_names(i, n),
        PTy::Tuple(ts) => ts.iter().any(|t| comp_names(t, n)),
        PTy::Fn(ps, r) => ps.iter().any(|t| comp_names(t, n)) || comp_names(r, n),
        PTy::Impl(bs) | PTy::Dyn(bs) => bs.iter().any(|b| match b {
            Bound::Lifetime(x) => lt && x == n,
            Bound::Trait(_, p) => path(p),
        }),
        _ => false,
    }
}

/// Whether a type of a signature may imply `n: l`, n a type parameter or a lifetime: n behind a `&l` (`&'b &'a u8`
/// gives `'a: 'b`), or beside l among one path's arguments (`W<'static, T>`, whose definition may bound T by it), or
/// in a trait object or `impl Trait` bounded by l. Not two parts of a tuple, a fn pointer's parameters or an array's
/// element beside an l elsewhere (`(T, &'static str)`, `(&'a u8, &'b u8)`)
fn wf_implies(t: &crate::parse::Ty, n: &str, l: &str) -> bool {
    use crate::parse::{Bound, GArg, Ty as PTy};
    let names = |t: &crate::parse::Ty| comp_names(t, n);
    match t {
        PTy::Ref(Some(r), _, i) if r == l && names(i) => true,
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => wf_implies(i, n, l),
        PTy::Tuple(ts) => ts.iter().any(|t| wf_implies(t, n, l)),
        PTy::Fn(ps, r) => ps.iter().any(|t| wf_implies(t, n, l)) || wf_implies(r, n, l),
        PTy::Path(q, p) => {
            q.as_deref().is_some_and(|q| wf_implies(q, n, l))
                || p.segs.iter().any(|s| {
                    let has_l = s.args.iter().any(|a| matches!(a, GArg::Lifetime(x) if x == l));
                    let has_n = s.args.iter().any(|a| match a {
                        GArg::Lifetime(x) => x == n,
                        GArg::Ty(t) | GArg::Bind(_, t) => names(t),
                        _ => false,
                    });
                    (has_l && has_n) || s.args.iter().any(|a| matches!(a, GArg::Ty(t) | GArg::Bind(_, t) if wf_implies(t, n, l)))
                })
        }
        PTy::Impl(bs) | PTy::Dyn(bs) => bs.iter().any(|b| matches!(b, Bound::Lifetime(x) if x == l)) && names(t),
        _ => false,
    }
}

/// A fn's text: its parameters' types (with their patterns) and its result's, each a piece, past its generics
/// (`fn f<F: Fn(u8)>(x: F, s: &'static str) -> u8 where ..`: `x: F`, `s: &'static str`, `u8`)
fn sig_type_pieces(t: &str) -> Vec<&str> {
    let Some(k) = t.find("fn ") else { return vec![] };
    let b = t.as_bytes();
    let mut i = k + 3;
    // (the name, then its generics, to the parameters' `(`)
    let mut angle = 0i32;
    while i < b.len() {
        match b[i] {
            b'<' => angle += 1,
            b'>' if i > 0 && b[i - 1] != b'-' => angle -= 1,
            b'(' if angle == 0 => break,
            _ => {}
        }
        i += 1;
    }
    let mut pieces = vec![];
    let mut d = 0i32;
    let mut start = i + 1;
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'(' | b'[' | b'<' => d += 1,
            b'>' if b[j - 1] == b'-' => {}
            b')' if d == 0 => {
                pieces.push(t[start..j].trim());
                break;
            }
            b')' | b']' | b'>' => d -= 1,
            b',' if d == 0 => {
                pieces.push(t[start..j].trim());
                start = j + 1;
            }
            _ => {}
        }
        j += 1;
    }
    // (the result: after `->`, to its where clause, body or `;`)
    if let Some(r) = t.get(j + 1..).and_then(|r| r.trim_start().strip_prefix("->")) {
        let end = [r.find(" where"), r.find("\nwhere"), r.find('{'), r.find(';')].into_iter().flatten().min().unwrap_or(r.len());
        pieces.push(r[..end].trim());
    }
    pieces.retain(|p| !p.is_empty());
    pieces
}

/// E0276 "impl has stricter requirements than trait", at the bound: a method of an impl of a trait of the program's
/// whose own type parameter (by its place) has a trait bound, inline or in the method's where clause, that the trait's
/// method's parameter does not have, nor one of the bounds it has as a supertrait (`T: Ord` where the trait says
/// `T: Eq`; `T: Eq` where it says `T: Ord` is fine). Bounds are told by their trait's name, not its arguments; not
/// lifetimes, nor the impl's own parameters. Only read for a program with a trait with a generic method
fn stricter_impl_bound(src: &str, items: &[SrcItem]) -> Option<String> {
    let text = |it: &SrcItem| src.get(it.start..it.end).unwrap_or("");
    // (a trait member with type parameters of its own: `fn f<T` before its `(`)
    let generic_fn = |t: &str| t.find('(').is_some_and(|p| t[..p].contains('<') && t[..p].contains("fn "));
    if !items.iter().any(|it| matches!(it.kind, crate::lexer::ItemKind::TraitMember(_)) && generic_fn(text(it))) {
        return None;
    }
    let (line, col, m) = stricter_impl_bound_in(src, items)?;
    Some(format!("{line}:{col}: {m}"))
}

#[cold]
#[inline(never)]
fn stricter_impl_bound_in(src: &str, items: &[SrcItem]) -> Option<(usize, usize, String)> {
    use crate::lexer::ItemKind as K;
    let text = |it: &SrcItem| src.get(it.start..it.end).unwrap_or("");
    let generic_fn = |t: &str| t.find('(').is_some_and(|p| t[..p].contains('<') && t[..p].contains("fn "));
    use crate::parse::{Bound, GParam, Ty as PTy};
    // (a header's name after its keyword, and the names after a `:` before its `{` / `where`: `trait B: A + Sized {`)
    let words = |t: &str| -> Vec<String> { crate::lexer::tokenize(t.as_bytes()).iter().filter(|x| x.0 == 0).map(|x| t[x.1..x.2].to_string()).collect() };
    let mut supers: crate::fxhash::FxMap<String, Vec<String>> = Default::default();
    let mut trait_of: crate::fxhash::FxMap<u32, String> = Default::default();
    for (i, it) in items.iter().enumerate() {
        if it.kind != K::TraitHeader {
            continue;
        }
        let t = text(it);
        let head = &t[..t.find('{').unwrap_or(t.len())];
        let w = words(head);
        let Some(k) = w.iter().position(|x| x == "trait") else { continue };
        let Some(name) = w.get(k + 1) else { continue };
        let sup: Vec<String> = match head.find(':') {
            Some(c) => {
                let r = &head[c + 1..];
                let r = &r[..r.find("where").unwrap_or(r.len())];
                words(r).into_iter().filter(|x| x.starts_with(|c: char| c.is_ascii_uppercase())).collect()
            }
            None => vec![],
        };
        supers.insert(name.clone(), sup);
        trait_of.insert(i as u32, name.clone());
    }
    // (std's supertraits)
    let std_supers = |n: &str| -> &'static [&'static str] {
        match n {
            "Eq" => &["PartialEq"],
            "Ord" => &["Eq", "PartialOrd", "PartialEq"],
            "PartialOrd" => &["PartialEq"],
            "Copy" => &["Clone"],
            "Error" => &["Debug", "Display"],
            "DoubleEndedIterator" | "ExactSizeIterator" => &["Iterator"],
            "Fn" => &["FnMut", "FnOnce"],
            "FnMut" => &["FnOnce"],
            _ => &[],
        }
    };
    let close = |ns: Vec<String>| -> Vec<String> {
        let mut out = ns;
        let mut k = 0;
        while k < out.len() {
            let n = out[k].clone();
            for s in std_supers(&n) {
                if !out.iter().any(|o| o == s) {
                    out.push(s.to_string());
                }
            }
            if let Some(ss) = supers.get(&n) {
                for s in ss {
                    if !out.contains(s) {
                        out.push(s.clone());
                    }
                }
            }
            k += 1;
        }
        out
    };
    // (a method's own type parameters with their bounds: inline, and its where clause's `T: B`)
    let params = |g: &crate::parse::Generics| -> Vec<Vec<(String, crate::parse::Pos)>> {
        g.params
            .iter()
            .filter_map(|q| match q {
                GParam::Ty(n, bs, _) => {
                    let mut v: Vec<(String, crate::parse::Pos)> = vec![];
                    let mut add = |bs: &[Bound]| {
                        for b in bs {
                            if let Bound::Trait(false, p) = b {
                                if let Some(s) = p.segs.last() {
                                    v.push((s.name.clone(), p.pos));
                                }
                            }
                        }
                    };
                    add(bs);
                    for (t, pbs) in &g.preds {
                        if matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == *n && p.segs[0].args.is_empty()) {
                            add(pbs);
                        }
                    }
                    Some(v)
                }
                _ => None,
            })
            .collect()
    };
    // (whether each of a method's own type parameters is bounded by `'static`: inline, or in its where clause)
    let static_params = |g: &crate::parse::Generics| -> Vec<bool> {
        let st = |bs: &[Bound]| bs.iter().any(|b| matches!(b, Bound::Lifetime(l) if l == "'static"));
        g.params
            .iter()
            .filter_map(|q| match q {
                GParam::Ty(n, bs, _) => Some(
                    st(bs) || g.preds.iter().any(|(t, pbs)| matches!(t, PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == *n && p.segs[0].args.is_empty()) && st(pbs)),
                ),
                _ => None,
            })
            .collect()
    };
    // (a member's signature alone, as a declaration: its body is not parsed)
    let sig = |t: &str| -> Option<crate::parse::FnSig> {
        let decl = match fn_body_start(t) {
            Some(b) => std::borrow::Cow::Owned(format!("{};", &t[..b])),
            None => std::borrow::Cow::Borrowed(t),
        };
        match crate::parse::parse_file(&decl).ok()?.items.into_iter().next()?.kind {
            crate::parse::ItemKind::Fn(s, _) => Some(s),
            _ => None,
        }
    };
    // (a member's fn name: `fn name<..>(`)
    fn fn_name(t: &str) -> Option<&str> {
        let k = t.find("fn ")? + 3;
        let r = t[k..].trim_start();
        Some(&r[..r.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(r.len())])
    }
    // (the generic methods of the program's traits, by trait and name; their signatures parsed once each)
    let mut decls: crate::fxhash::FxMap<(u32, &str), &SrcItem> = Default::default();
    for d in items {
        if let K::TraitMember(th) = d.kind {
            if generic_fn(text(d)) {
                if let Some(n) = fn_name(text(d)) {
                    decls.insert((th, n), d);
                }
            }
        }
    }
    let mut decl_sigs: crate::fxhash::FxMap<(u32, &str), Option<crate::parse::FnSig>> = Default::default();
    // (each impl header's trait of the program's, read once)
    let mut impl_trait: crate::fxhash::FxMap<u32, Option<u32>> = Default::default();
    let mut by_text: crate::fxhash::FxMap<&str, Option<u32>> = Default::default();
    // (a member's verdict: E0276 at a bound, or E0049 with (its own, the trait's) type parameter counts)
    let mut verdicts: crate::fxhash::FxMap<(u32, &str, bool), Option<Result<crate::parse::Pos, (usize, usize, bool)>>> = Default::default();
    for m in items {
        let K::ImplMember(h) = m.kind else { continue };
        // (a fn of the impl: a generic one for E0276, any for E0049 against a generic method of the trait)
        if !text(m).contains("fn ") {
            continue;
        }
        let th = *impl_trait.entry(h).or_insert_with(|| {
            // (`impl<..> Tr<..> for X {`: the trait's name before `for`, by the text before it (the same for each impl
            // a macro writes: `impl FromHex for [u8; N]`))
            let t = text(items.get(h as usize)?);
            let head = &t[..t.find('{').unwrap_or(t.len())];
            let f = head.find(" for ")?;
            let before = &head[..f];
            // (the trait's path: after `impl` and its generics, before the trait's own arguments (`impl<'a> A<'a>`))
            let path = match before.find("impl") {
                Some(k) => {
                    let r = before[k + 4..].trim_start();
                    match r.strip_prefix('<') {
                        Some(g) => {
                            let mut d = 1;
                            let end = g.char_indices().find(|&(_, c)| {
                                d += match c {
                                    '<' => 1,
                                    '>' => -1,
                                    _ => 0,
                                };
                                d == 0
                            });
                            end.map_or(r, |(e, _)| &g[e + 1..])
                        }
                        None => r,
                    }
                }
                None => before,
            };
            let before = path[..path.find('<').unwrap_or(path.len())].trim();
            *by_text.entry(before).or_insert_with(|| {
                let tw = words(before);
                let tr = tw.last()?;
                trait_of.iter().find(|(_, n)| *n == tr).map(|(&th, _)| th)
            })
        });
        let Some(th) = th else { continue };
        let Some(name) = fn_name(text(m)) else { continue };
        let Some(&d) = decls.get(&(th, name)) else { continue };
        // (the verdict by the member's signature text, the same for each impl a macro writes: where in it a bound is
        // stricter, if one is)
        let mt = text(m);
        let key = &mt[..fn_body_start(mt).unwrap_or(mt.len())];
        // (whether the impl's header gives the trait `'static`: part of the verdict, with the signature)
        let header = items.get(h as usize).map(|it| text(it)).unwrap_or("");
        let header_static = header[..header.find('{').unwrap_or(header.len())].contains("'static");
        let at = verdicts.entry((th, key, header_static)).or_insert_with(|| {
            let ms = sig(mt)?;
            let ds = decl_sigs.entry((th, name)).or_insert_with(|| sig(text(d))).clone()?;
            let (mp, dp) = (params(&ms.generics), params(&ds.generics));
            // (not when the counts of type and const parameters together agree: a kind that differs is rustc's
            // "incompatible generic parameter" (E0053))
            let all = |g: &crate::parse::Generics| g.params.iter().filter(|q| !matches!(q, GParam::Lifetime(_))).count();
            if mp.len() != dp.len() && all(&ms.generics) == all(&ds.generics) {
                return None;
            }
            if mp.len() != dp.len() {
                // (not with an `impl Trait` parameter on either side, which rustc reports otherwise (E0643))
                let dt = text(d);
                let apit = |t: &str| [": impl ", ": &impl ", ": &mut impl "].iter().any(|p| t.contains(p));
                if apit(key) || apit(&dt[..fn_body_start(dt).unwrap_or(dt.len())]) {
                    return None;
                }
                return Some(Err((mp.len(), dp.len(), ms.recv.is_some())));
            }
            let by_trait = mp.iter().zip(&dp).find_map(|(mb, db)| {
                let allowed = close(db.iter().map(|x| x.0.clone()).collect());
                mb.iter().find(|(n, _)| n != "Sized" && !allowed.contains(n)).map(|x| Ok(x.1))
            });
            if by_trait.is_some() {
                return by_trait;
            }
            // (a type parameter of the impl's method bounded by `'static` where the trait's is not (`F: Debug +
            // 'static` for the trait's `F: Debug + 'a`): stricter, unless the impl's header gives the trait `'static`,
            // or the trait's parameters and result name `'static` and that type parameter, which may imply it (`x:
            // &'static T`; not `x: &'static u8`). At the first `'static` of the member's signature)
            let (ms_st, ds_st) = (static_params(&ms.generics), static_params(&ds.generics));
            // (the trait's parameters' types and its result's, by their structure: `x: &'static T` and `x:
            // W<'static, T>` may imply the bound; `x: T, s: &'static str` and `x: (T, &'static str)` do not)
            let names: Vec<&str> = ds.generics.params.iter().filter_map(|q| match q {
                GParam::Ty(n, ..) => Some(n.as_str()),
                _ => None,
            }).collect();
            let implied = |n: &str| ds.params.iter().map(|p| &p.1).chain(ds.ret.as_ref()).any(|t| static_implies(t, n));
            if ms_st.iter().zip(&ds_st).zip(&names).any(|((m, d), n)| *m && !*d && !implied(n)) && !header_static {
                let off = key.find("'static")?;
                let line = key[..off].matches('\n').count() + 1;
                let col = off - key[..off].rfind('\n').map_or(0, |k| k + 1);
                return Some(Ok(crate::parse::Pos { line: line as u32, col: col as u32 }));
            }
            None
        });
        match *at {
            Some(Ok(pos)) => {
                // (the position in the member's text: its first line starts where the member does)
                let line = m.line + pos.line as usize - 1;
                let col = if pos.line == 1 { m.start - src[..m.start].rfind('\n').map_or(0, |k| k + 1) + pos.col as usize + 1 } else { pos.col as usize + 1 };
                return Some((line, col, "impl has stricter requirements than trait (E0276)".to_string()));
            }
            // (E0049: `fn foo(x: bool)` for the trait's `fn foo<T: Default>(x: T)`)
            Some(Err((mc, dc, method))) => {
                let kind = if method { "method" } else { "associated function" };
                let n = |c: usize| if c == 1 { "1 type parameter".to_string() } else { format!("{c} type parameters") };
                return Some((m.line, 1, format!("{kind} `{name}` has {} but its trait declaration has {} (E0049)", n(mc), n(dc))));
            }
            None => {}
        }
    }
    None
}

/// Two items of one name in one impl block, in one namespace (a fn's and a const's are values, a type's is a type):
/// "duplicate definitions with name `bar`:" (E0201) in a trait's impl, "duplicate definitions with name `bar`"
/// (E0592) in an inherent one, at the second item's start (rustc 1.100). Read from the items' own identifiers
/// Types of std that a crate's inherent impl cannot name (E0116), unless the crate has a type of that name
const STD_ADTS: &[&str] = &[
    "Arc", "BTreeMap", "BTreeSet", "BinaryHeap", "Box", "Cell", "Cow", "Duration", "HashMap", "HashSet", "LinkedList", "Mutex", "NonNull",
    "Option", "PhantomData", "Pin", "Rc", "RefCell", "Result", "RwLock", "String", "Vec", "VecDeque", "Weak",
];

/// Where a fn item's body starts in its text: the first `{` outside its generics, parameters and brackets (None for
/// a declaration, `fn f();`)
fn fn_body_start(t: &str) -> Option<usize> {
    let b = t.as_bytes();
    let mut d = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // (not a bracket in a literal: `[u8; b'[' as usize]`)
            b'\'' | b'"' if quote_end(b, i).is_some() => i = quote_end(b, i).unwrap_or(i),
            b'<' | b'(' | b'[' => d += 1,
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            b'>' | b')' | b']' => d -= 1,
            b'{' if d <= 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// E0084: `#[repr(i32)] enum Foo {}`, an integer or `C` representation of an enum without variants, at the `repr`.
/// text: the item's, t: past its attributes and visibility
#[cold]
fn zero_variant_repr(it: &SrcItem, text: &str, t: &str) -> Option<String> {
    let body = t[t.find('{')? + 1..].trim_start();
    if !body.starts_with('}') {
        return None;
    }
    let attrs = &text[..text.len() - t.len()];
    let at = attrs.find("repr(")?;
    let list = &attrs[at + 5..at + 5 + attrs[at + 5..].find(')')?];
    const INTS: &[&str] = &["C", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize"];
    if !list.split(',').any(|h| INTS.contains(&h.trim())) {
        return None;
    }
    let line = it.line + attrs[..at].matches('\n').count();
    Some(format!("{line}:1: unsupported representation for zero-variant enum (E0084)"))
}

/// The keyword of an impl member (past its visibility and qualifiers): `fn`, `type` or `const`
fn x_kind<'a>(mut w: impl Iterator<Item = &'a str>) -> &'a str {
    let mut prev = "";
    for x in w.by_ref().take(8) {
        match x {
            "fn" | "type" => return x,
            "pub" | "crate" | "self" | "super" | "in" | "unsafe" | "async" | "extern" | "default" => {}
            "const" => prev = "const",
            _ if prev == "const" => return "const",
            _ => return "",
        }
    }
    prev
}

/// E0437 / E0438: member `name` (a type when `ty`) of the trait impl whose header is item `h`, when the impl is of a
/// trait of the program's (one of that name) that declares no type or const of that name
#[cold]
fn not_a_member<'a>(src: &'a str, items: &[SrcItem], it: &SrcItem, h: u32, ty: bool, name: &str, traits: &mut Option<TraitMembers<'a>>) -> Option<String> {
    let TraitMembers { traits: tmap, names, by_header } = traits.get_or_insert_with(|| TraitMembers::new(src, items));
    // (a name a trait of the program declares is taken as one of its impl's trait, without reading the impl's header: an
    // impl of another trait of the program that names it is missed)
    if names.contains(&(ty, name)) {
        return None;
    }
    // (else no trait of the program declares it: an error in an impl of one)
    (*by_header.entry(h).or_insert_with(|| {
        let tr = impl_trait_name(src, items.get(h as usize)?)?;
        *tmap.get(tr)?
    }))?;
    let tr = impl_trait_name(src, items.get(h as usize)?)?;
    let col = it.start - src[..it.start].rfind('\n').map_or(0, |k| k + 1) + 1;
    Some(if ty {
        format!("{}:{col}: type `{name}` is not a member of trait `{tr}` (E0437)", it.line)
    } else {
        format!("{}:{col}: const `{name}` is not a member of trait `{tr}` (E0438)", it.line)
    })
}

/// The trait's name of a trait impl's header (`impl<T> Tr<T> for X {` -> `Tr`); None for an inherent impl
fn impl_trait_name<'a>(src: &'a str, hi: &SrcItem) -> Option<&'a str> {
    let head = src.get(hi.start..hi.end)?;
    let head = item_text(head).strip_prefix("unsafe ").map_or(item_text(head), |r| r.trim_start());
    let rest = head.strip_prefix("impl")?;
    if !hi.idents.iter().any(|&(a, b)| src.get(a as usize..b as usize) == Some("for")) {
        return None;
    }
    Some(impl_self_name(rest))
}

/// The program's traits, read once for E0437 / E0438: each name's header (None for a name two traits have), the
/// (trait, a type?, name) of their associated types and consts, and each impl header's trait
struct TraitMembers<'a> {
    traits: FxMap<&'a str, Option<u32>>,
    /// (a type?, name) of their associated types and consts
    names: FxSet<(bool, &'a str)>,
    by_header: FxMap<u32, Option<u32>>,
}

impl<'a> TraitMembers<'a> {
    #[cold]
    fn new(src: &'a str, items: &[SrcItem]) -> Self {
        use crate::lexer::ItemKind as K;
        let mut traits: FxMap<&str, Option<u32>> = FxMap::default();
        let mut names = FxSet::default();
        for (i, t) in items.iter().enumerate() {
            match t.kind {
                K::TraitHeader => {
                    let mut w = t.idents.iter().map(|&(a, b)| src.get(a as usize..b as usize).unwrap_or(""));
                    if w.find(|&x| x == "trait").is_some() {
                        if let Some(n) = w.next() {
                            traits.entry(n).and_modify(|e| *e = None).or_insert(Some(i as u32));
                        }
                    }
                }
                // (a member by its text, past its attributes and visibility: `#[doc(hidden)] type Buffer: 'static;`)
                K::TraitMember(th) => {
                    let raw = src.get(t.start..t.end).unwrap_or("");
                    // (a fn by its first byte, before reading past attributes)
                    if !matches!(raw.as_bytes().first(), Some(b't' | b'c' | b'#' | b'p')) {
                        continue;
                    }
                    let text = item_text(raw);
                    let (is_ty, r) = match (text.strip_prefix("type"), text.strip_prefix("const")) {
                        (Some(r), _) => (true, r),
                        (_, Some(r)) => (false, r),
                        _ => continue,
                    };
                    if !r.starts_with(char::is_whitespace) {
                        continue;
                    }
                    let r = r.trim_start();
                    let n = &r[..r.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(r.len())];
                    if !n.is_empty() && n != "fn" {
                        names.insert((is_ty, n));
                    }
                }
                _ => {}
            }
        }
        TraitMembers { traits, names, by_header: FxMap::default() }
    }
}

/// Errors of an item's declaration, read from its text: an inherent impl of a type of std (E0116: `impl Vec<u8> {}`,
/// named or through a type alias of the crate, which only std can have; `foreign`: when the crate has no
/// dependencies) or of a type parameter (E0118), a type parameter of a type alias that the alias's type does not
/// name (E0091: `type Foo<T> = u32;`; a lifetime or a const parameter may go unused, and so may the parameters of an
/// `impl Trait` alias)
fn item_decl_err(src: &str, items: &[SrcItem], foreign: bool) -> Option<String> {
    use crate::lexer::ItemKind as K;
    let mut aliases = None;
    // (the program's types and traits with lifetime parameters, read once when an impl is met)
    let mut lts: Option<crate::fxhash::FxSet<&str>> = None;
    let sb = src.as_bytes();
    for it in items {
        if !matches!(it.kind, K::ImplHeader | K::Normal | K::ImplMember(_)) {
            continue;
        }
        // (by the first byte: an attribute, a visibility, `impl`, `type`, `async` or `unsafe`; `pub fn` and the like by
        // the fifth; an impl's member only for `async`)
        let b = &sb[it.start.min(sb.len())..it.end.min(sb.len())];
        if !matches!(b.first(), Some(b'i' | b't' | b'p' | b'#' | b'u' | b'a'))
            || b.get(3) == Some(&b' ') && !matches!(b.get(4), Some(b't' | b'a')) && b[0] == b'p'
            || matches!(it.kind, K::ImplMember(_)) && !matches!(b.first(), Some(b'a' | b'p' | b'#'))
        {
            continue;
        }
        let text = src.get(it.start..it.end).unwrap_or("");
        let t = item_text(text);
        if t.starts_with("async") {
            // (E0726: an `async fn`'s parameter of a type with lifetime parameters written without them)
            let l = lts.get_or_insert_with(|| lt_names(src, items));
            if !l.is_empty() {
                if let Some(e) = elided_async_lt(it, text, t, l) {
                    return Some(e);
                }
            }
            continue;
        }
        if matches!(it.kind, K::ImplMember(_)) {
            continue;
        }
        if let Some(rest) = t.strip_prefix("impl") {
            // (E0726: `impl MyTrait for Foo` of a `Foo<'a>` / a `MyTrait<'a>`)
            let l = lts.get_or_insert_with(|| lt_names(src, items));
            // (only an impl whose header names one of them: by its identifiers)
            if !l.is_empty() && it.idents.iter().any(|&(a, e)| src.get(a as usize..e as usize).is_some_and(|w| l.contains(w))) {
                if let Some(e) = elided_impl_lt(text, it, rest, l) {
                    return Some(e);
                }
            }
            if let Some(e) = inherent_impl_err(src, items, it, rest, foreign, &mut aliases) {
                return Some(e);
            }
        } else if let Some(rest) = t.strip_prefix("unsafe").and_then(|r| r.trim_start().strip_prefix("impl")) {
            // (E0197: `unsafe impl S {}`, an inherent impl)
            let head_end = it.end - rest.len() + rest.find('{').unwrap_or(rest.len());
            if !it.idents.iter().take_while(|&&(a, _)| (a as usize) < head_end).any(|&(a, b)| src.get(a as usize..b as usize) == Some("for")) {
                let line = it.line + text[..text.len() - t.len()].matches('\n').count();
                return Some(format!("{line}:1: inherent impls cannot be unsafe (E0197)"));
            }
        } else if b[0] == b'#' && it.kind == K::Normal && t.starts_with("enum") {
            if let Some(e) = zero_variant_repr(it, text, t) {
                return Some(e);
            }
        } else if it.kind == K::Normal && t.starts_with("type") {
            let at = text.len() - t.len();
            let Some(lt) = t.find(['<', '=', ';']).filter(|&k| t.as_bytes()[k] == b'<') else { continue };
            if let Some(e) = unused_alias_param_in(src, items, it, at + lt) {
                return Some(e);
            }
        }
    }
    None
}

/// The names of the program's structs, enums, unions and traits declared with lifetime parameters (`struct Foo<'a>`)
fn lt_names<'a>(src: &'a str, items: &[SrcItem]) -> crate::fxhash::FxSet<&'a str> {
    use crate::lexer::ItemKind as K;
    let mut out = crate::fxhash::FxSet::default();
    for it in items {
        if !matches!(it.kind, K::Normal | K::TraitHeader) {
            continue;
        }
        let raw = src.get(it.start..it.end).unwrap_or("");
        // (by the first byte before reading past attributes and visibility: a fn or an impl is none of them)
        if !matches!(raw.as_bytes().first(), Some(b's' | b'e' | b'u' | b't' | b'#' | b'p')) || !raw.as_bytes()[..raw.len().min(256)].contains(&b'\'') {
            continue;
        }
        let t = item_text(raw);
        let t = t.strip_prefix("unsafe ").unwrap_or(t);
        if !matches!(t.as_bytes().first(), Some(b's' | b'e' | b'u' | b't')) {
            continue;
        }
        let Some(r) = ["struct ", "enum ", "union ", "trait "].iter().find_map(|k| t.strip_prefix(k)) else { continue };
        let r = r.trim_start();
        let n = &r[..r.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(r.len())];
        if !n.is_empty() && r[n.len()..].trim_start().starts_with("<'") {
            out.insert(n);
        }
    }
    out
}

/// E0726 of an impl header (the text past `impl`): its trait's or self type's path names one of `lts` with no
/// arguments
#[cold]
fn elided_impl_lt(text: &str, it: &SrcItem, rest: &str, lts: &crate::fxhash::FxSet<&str>) -> Option<String> {
    let r = rest.trim_start();
    let b = r.as_bytes();
    let mut i = 0;
    if b.first() == Some(&b'<') {
        let mut d = 0;
        while i < b.len() {
            match b[i] {
                b'<' => d += 1,
                b'>' if i > 0 && b[i - 1] != b'-' => d -= 1,
                _ => {}
            }
            i += 1;
            if d == 0 {
                break;
            }
        }
    }
    let head = &r[i.min(r.len())..];
    let head = &head[..head.find('{').unwrap_or(head.len())];
    let head = &head[..head.find(" where ").unwrap_or(head.len())];
    for part in head.split(" for ") {
        let p = part.trim_start().trim_start_matches(['&', ' ']);
        let p = p.strip_prefix("dyn ").unwrap_or(p).trim_start();
        let end = p.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(p.len());
        let name = p[..end].rsplit("::").next().unwrap_or("");
        if lts.contains(name) && !p[end..].trim_start().starts_with('<') {
            let line = it.line + text[..text.len() - rest.len()].matches('\n').count();
            return Some(format!("{line}:1: implicit elided lifetime not allowed here (E0726)"));
        }
    }
    None
}

/// E0726 of an `async fn` (the item's text, and t past its attributes and visibility): a parameter's type naming one
/// of `lts` with no arguments
#[cold]
fn elided_async_lt(it: &SrcItem, text: &str, t: &str, lts: &crate::fxhash::FxSet<&str>) -> Option<String> {
    {
        let Some(r) = t.strip_prefix("async").filter(|r| r.trim_start().starts_with("fn ")) else { return None };
        let Some(open) = r.find('(') else { return None };
        let mut d = 0i32;
        let close = r[open..].char_indices().find(|&(_, c)| {
            match c {
                '(' => d += 1,
                ')' => d -= 1,
                _ => {}
            }
            d == 0
        });
        let Some((cl, _)) = close else { return None };
        let params = &r[open + 1..open + cl];
        let mut k = 0;
        while k < params.len() {
            let w = &params[k..];
            let n = w.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(w.len());
            if n == 0 {
                k += w.chars().next().map_or(1, |c| c.len_utf8());
                continue;
            }
            if lts.contains(&w[..n]) && !w[n..].trim_start().starts_with('<') {
                let line = it.line + text[..text.len() - t.len()].matches('\n').count();
                return Some(format!("{line}:1: implicit elided lifetime not allowed here (E0726)"));
            }
            k += n;
        }
    }
    None
}

/// E0118 / E0116 of an impl whose text past `impl` is `rest`: none for a trait impl (a `for` in its header)
fn inherent_impl_err<'a>(src: &'a str, items: &[SrcItem], it: &SrcItem, rest: &str, foreign: bool, aliases: &mut Option<crate::fxhash::FxSet<&'a str>>) -> Option<String> {
    let head_end = it.end - rest.len() + rest.find('{').unwrap_or(rest.len());
    if it.idents.iter().take_while(|&&(a, _)| (a as usize) < head_end).any(|&(a, b)| src.get(a as usize..b as usize) == Some("for")) {
        return None;
    }
    let name = impl_self_name(rest);
    if name.is_empty() {
        return None;
    }
    // (E0118: `impl<T> T {}`, the self type one of the impl's type parameters)
    if rest.trim_start().starts_with('<') && impl_param_self(rest, name) {
        let off = src.get(it.start..it.end)?.len() - rest.len();
        let line = it.line + src[it.start..it.start + off].matches('\n').count();
        return Some(format!("{line}:1: no nominal type found for inherent implementation (E0118)"));
    }
    if !foreign {
        return None;
    }
    if !STD_ADTS.contains(&name) {
        // (the crate's type aliases, read once: one of them may name a type of std)
        let al = aliases.get_or_insert_with(|| items.iter().filter_map(|a| alias_name(src, a)).collect());
        if !al.contains(name) {
            return None;
        }
    }
    foreign_inherent_impl_err(src, items, it, rest, name)
}

/// The last name of the self type's path of an inherent impl (the text past `impl`): `<T> a::B<T> {` -> `B`; ""
/// for a type that is no path
fn impl_self_name(rest: &str) -> &str {
    let b = rest.as_bytes();
    let mut i = 0;
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    if b.get(i) == Some(&b'<') {
        let mut d = 0;
        while i < b.len() {
            match b[i] {
                b'<' => d += 1,
                b'>' if b[i - 1] != b'-' => d -= 1,
                _ => {}
            }
            i += 1;
            if d == 0 {
                break;
            }
        }
    }
    let r = rest[i..].trim_start();
    let end = r.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).unwrap_or(r.len());
    let path = &r[..end];
    path.rsplit("::").next().unwrap_or("")
}

/// Whether the self type of an inherent impl (the text past `impl`, its generics first) is the single name of one of
/// its type parameters: `<T: Tr> T {`
fn impl_param_self(rest: &str, name: &str) -> bool {
    let r = rest.trim_start();
    let b = r.as_bytes();
    let (mut d, mut i, mut start) = (0i32, 0, true);
    let mut found = false;
    while i < b.len() {
        match b[i] {
            b'<' | b'(' | b'[' => d += 1,
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            b'>' | b')' | b']' => {
                d -= 1;
                if d == 0 {
                    i += 1;
                    break;
                }
            }
            b',' if d == 1 => start = true,
            c if start && d == 1 && !c.is_ascii_whitespace() => {
                start = false;
                found |= r[i..].starts_with(name) && !r[i + name.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
            }
            _ => {}
        }
        i += 1;
    }
    // (the self type is that name alone, not a path through it (`T::Assoc`) nor a type it is an argument of)
    let t = r[i.min(r.len())..].trim_start();
    found && t.starts_with(name) && t[name.len()..].trim_start().starts_with(['{', 'w'])
}

/// Whether `#[test]` may be on an item: a free fn (`async`, `const`, `unsafe` or `extern "C"` too), or a macro call
/// (a warning); not an associated fn, nor another kind of item
fn test_target(text: &str, kind: crate::lexer::ItemKind) -> bool {
    if kind != crate::lexer::ItemKind::Normal {
        return false;
    }
    let mut t = item_text(text);
    loop {
        let r = ["async ", "const ", "unsafe ", "safe "].iter().find_map(|q| t.strip_prefix(q));
        let r = r.or_else(|| t.strip_prefix("extern ").map(|r| r.trim_start().strip_prefix('"').and_then(|r| Some(&r[r.find('"')? + 1..])).unwrap_or(r)));
        match r {
            Some(r) => t = r.trim_start(),
            None => break,
        }
    }
    if t.starts_with("fn ") {
        return true;
    }
    let w = &t[..t.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(t.len())];
    !w.is_empty() && w != "macro_rules" && t[w.len()..].trim_start().starts_with('!')
}

/// An item's text past its attributes and visibility (`#[cfg(x)] pub(crate) type A = u8;` -> `type A = u8;`)
fn item_text(text: &str) -> &str {
    let t = if text.starts_with('#') { &text[leading_attrs(text).len()..] } else { text };
    let b = t.as_bytes();
    if !b.starts_with(b"pub") {
        return t;
    }
    let mut i = 3;
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    if b.get(i) == Some(&b'(') {
        i += b[i..].iter().position(|&c| c == b')').map_or(b.len() - i, |k| k + 1);
        while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
    }
    &t[i..]
}

/// Whether an item's text may start with `kw` past its attributes and visibility: by its first byte, before
/// item_text reads it
fn may_start(text: &str, kw: u8) -> bool {
    matches!(text.as_bytes().first(), Some(&c) if c == kw || c == b'#' || c == b'p')
}

/// The name of a type alias item (`pub type Function = ..;`)
fn alias_name<'a>(src: &'a str, a: &SrcItem) -> Option<&'a str> {
    let text = src.get(a.start..a.end)?;
    if a.kind != crate::lexer::ItemKind::Normal || !may_start(text, b't') {
        return None;
    }
    let t = item_text(text).strip_prefix("type")?;
    let t = t.trim_start();
    Some(&t[..t.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(t.len())])
}

/// Whether item `a` is `kw name` (`struct Vec`, `type Function`), past its attributes and visibility
fn item_named(src: &str, a: &SrcItem, kw: &str, name: &str) -> bool {
    let mut w = a.idents.iter().map(|&(x, y)| src.get(x as usize..y as usize).unwrap_or(""));
    while let Some(x) = w.next() {
        if x == kw {
            return w.next() == Some(name);
        }
    }
    false
}

#[cold]
fn foreign_inherent_impl_err(src: &str, items: &[SrcItem], it: &SrcItem, rest: &str, name: &str) -> Option<String> {
    use crate::lexer::ItemKind as K;
    // (a type alias of the crate: what it names)
    let mut name = name;
    if let Some(a) = items.iter().find(|a| a.kind == K::Normal && item_named(src, a, "type", name)) {
        let t = src.get(a.start..a.end).unwrap_or("");
        let rhs = t[t.find('=')? + 1..].trim_start().trim_end_matches(';');
        let end = rhs.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).unwrap_or(rhs.len());
        name = rhs[..end].rsplit("::").next().unwrap_or("");
    }
    let own = |a: &&SrcItem| a.kind == K::Normal && ["struct", "enum", "union", "trait"].iter().any(|kw| item_named(src, a, kw, name));
    let no_core = |a: &&SrcItem| a.kind == K::InnerAttr && src.get(a.start..a.end).is_some_and(|t| t.contains("no_core"));
    if !STD_ADTS.contains(&name) || items.iter().any(|a| own(&a) || no_core(&a)) {
        return None;
    }
    let off = src.get(it.start..it.end)?.len() - rest.len();
    let line = it.line + src[it.start..it.start + off].matches('\n').count();
    Some(format!("{line}:1: cannot define inherent `impl` for a type outside of the crate where the type is defined (E0116)"))
}

/// item_decl_err's E0091 for an alias whose generics start at byte `lt` of its text
#[cold]
fn unused_alias_param_in(src: &str, items: &[SrcItem], it: &SrcItem, lt: usize) -> Option<String> {
    let text = src.get(it.start..it.end)?;
    let b = text.as_bytes();
    // (the parameters: the name each one starts with, past the `<` or a `,` at depth 1)
    let mut params: Vec<usize> = vec![];
    let (mut d, mut i, mut start) = (0i32, lt, true);
    while i < b.len() {
        match b[i] {
            b'<' | b'(' | b'[' => d += 1,
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            b'>' | b')' | b']' => {
                d -= 1;
                if d == 0 {
                    i += 1;
                    break;
                }
            }
            b',' if d == 1 => start = true,
            c if start && d == 1 && !c.is_ascii_whitespace() => {
                start = false;
                if c.is_ascii_alphabetic() || c == b'_' {
                    params.push(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    let eq = i + text[i..].find('=')?;
    let used = |n: &str| it.idents.iter().any(|&(x, y)| x as usize >= it.start + eq && src.get(x as usize..y as usize) == Some(n));
    if used("impl") {
        return None;
    }
    // (not when the type may not resolve: rustc reports that instead. `type A<T> = B;` of another alias of the crate
    // (`type B = _;`), a path through modules (`<m::Alias as n::Trait>::Archived`, m and n unknown))
    let rhs = text[eq + 1..].trim().trim_end_matches(';').trim_end();
    if rhs.contains("::") || !rhs.is_empty() && rhs.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') && items.iter().any(|a| alias_name(src, a) == Some(rhs)) {
        return None;
    }
    for p in params {
        let n = &text[p..p + text[p..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(0)];
        if n.is_empty() || n == "const" || used(n) {
            continue;
        }
        let line = it.line + text[..p].matches('\n').count();
        return Some(format!("{line}:1: type parameter `{n}` is never used (E0091)"));
    }
    None
}

fn duplicate_impl_item(src: &str, items: &[SrcItem]) -> Option<String> {
    use crate::lexer::ItemKind as K;
    let mut seen: crate::fxhash::FxSet<(u32, bool, &str)> = Default::default();
    // (the program's traits and their members, read once)
    let mut traits: Option<TraitMembers> = None;
    for it in items {
        let K::ImplMember(h) = it.kind else { continue };
        let ids = || it.idents.iter().map(|&(a, b)| src.get(a as usize..b as usize).unwrap_or(""));
        // (the item's keyword and name, past its visibility and qualifiers: `pub const fn f`, `const C`, `type T`)
        let mut w = ids().take(8).peekable();
        let mut found = None;
        while let Some(x) = w.next() {
            match x {
                "fn" | "type" => {
                    found = w.next().map(|n| (x == "type", n));
                    break;
                }
                "const" if !matches!(w.peek(), Some(&("fn" | "unsafe" | "async" | "extern"))) => {
                    found = w.next().map(|n| (false, n));
                    break;
                }
                "pub" | "crate" | "self" | "super" | "in" | "const" | "unsafe" | "async" | "extern" | "default" => {}
                _ => break,
            }
        }
        let Some((ty, name)) = found else { continue };
        if name == "_" {
            continue;
        }
        // (E0437 / E0438: an associated type or const of a trait impl that the program's trait does not declare)
        if x_kind(ids()) != "fn" {
            if let Some(e) = not_a_member(src, items, it, h, ty, name, &mut traits) {
                return Some(e);
            }
        }
        if !seen.insert((h, ty, name)) {
            let head = items.get(h as usize).map_or("", |hi| src.get(hi.start..hi.end).unwrap_or(""));
            let head = &head[..head.find('{').unwrap_or(head.len())];
            let trait_impl = head.contains(" for ");
            let col = it.start - src[..it.start].rfind('\n').map_or(0, |k| k + 1) + 1;
            return Some(if trait_impl {
                format!("{}:{col}: duplicate definitions with name `{name}`: (E0201)", it.line)
            } else {
                format!("{}:{col}: duplicate definitions with name `{name}` (E0592)", it.line)
            });
        }
    }
    None
}

/// std's traits (zustc's prelude has others, by `__` names); the ones std's prelude does not bring into scope are
/// NON_PRELUDE_TRAITS
const STD_TRAITS: &[&str] = &[
    "Sized", "Send", "Sync", "Unpin", "Copy", "Clone", "Fn", "FnMut", "FnOnce", "Drop", "Iterator", "IntoIterator", "DoubleEndedIterator",
    "ExactSizeIterator", "Extend", "FromIterator", "PartialEq", "Eq", "PartialOrd", "Ord", "Default", "AsRef", "AsMut", "Into", "From", "TryFrom",
    "TryInto", "ToOwned", "ToString", "Future", "IntoFuture",
];
const NON_PRELUDE_TRAITS: &[&str] = &[
    "Debug", "Display", "Hash", "Hasher", "BuildHasher", "Error", "FromStr", "Read", "Write", "BufRead", "Seek", "Deref", "DerefMut", "Add", "Sub",
    "Mul", "Div", "Rem", "Neg", "Not", "BitAnd", "BitOr", "BitXor", "Shl", "Shr", "AddAssign", "SubAssign", "MulAssign", "DivAssign", "RemAssign",
    "BitAndAssign", "BitOrAssign", "BitXorAssign", "ShlAssign", "ShrAssign", "Index", "IndexMut", "Borrow", "BorrowMut", "FusedIterator", "Any",
    "Sum", "Product", "Pointer", "LowerHex", "UpperHex", "Binary", "Octal", "LowerExp", "UpperExp", "RangeBounds", "Termination", "Step",
    "UnwindSafe", "RefUnwindSafe", "Unsize", "CoerceUnsized", "DispatchFromDyn", "Tuple",
];

/// A `#[macro_export]` macro whose name a crate-root `pub use` of std's (or core's, alloc's) also brings as a macro:
/// `pub use std::panic;` re-exports the module and the macro `panic!`, so `#[macro_export] macro_rules! panic` is
/// "the name `panic` is defined multiple times" (E0255), at the macro. Only std's macros of that name count (a
/// module or fn of the name is in another namespace)
#[cold]
#[inline(never)]
fn export_beside_std_reexport(src0: &str, exported: &[String]) -> Option<String> {
    const STD_MACROS: &[&str] = &[
        "panic", "assert", "assert_eq", "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne", "vec", "format", "print", "println", "eprint",
        "eprintln", "write", "writeln", "todo", "unimplemented", "unreachable", "matches", "dbg", "concat", "stringify", "line", "file", "column", "env",
        "include", "include_str", "include_bytes", "compile_error", "format_args", "thread_local",
    ];
    // (the crate root's `pub use std::..::name;` / `pub use core::..::name as other;`: at a line's start)
    let mut names: Vec<&str> = vec![];
    for line in src0.lines() {
        let Some(r) = line.strip_prefix("pub use ") else { continue };
        let r = r.trim_end().trim_end_matches(';').trim();
        let r = r.strip_prefix("::").unwrap_or(r);
        if !(r.starts_with("std::") || r.starts_with("core::") || r.starts_with("alloc::")) || r.contains(['{', '*']) {
            continue;
        }
        let (path, alias) = match r.split_once(" as ") {
            Some((p, a)) => (p.trim(), Some(a.trim())),
            None => (r, None),
        };
        let last = path.rsplit("::").next().unwrap_or("");
        if STD_MACROS.contains(&last) {
            names.push(alias.unwrap_or(last));
        }
    }
    if names.is_empty() {
        return None;
    }
    // (the exported macros the expansion read, at their `macro_rules!` in the source as written)
    let name = exported.iter().find(|n| names.contains(&n.as_str()))?;
    let at = src0.match_indices("macro_rules!").map(|(k, _)| k).find(|&k| {
        let r = src0[k + 12..].trim_start();
        r.starts_with(name.as_str()) && r[name.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })?;
    let line = 1 + src0[..at].matches('\n').count();
    let col = at - src0[..at].rfind('\n').map_or(0, |p| p + 1) + 1;
    Some(format!("{line}:{col}: the name `{name}` is defined multiple times (E0255)"))
}

/// A type an impl's header names that nothing declares (`impl A<B> {}`): "cannot find type `B` in this scope" (rustc
/// 1.100's E0412), at the name. Read from the header's tokens: a capitalized name in the self type or in the trait's
/// arguments, not the trait itself, not a path's segment (`T::A`, `E::V`), an associated type's name (`Item = ..`), a
/// bound (after `dyn`, `impl`, `:`, `+`), a macro, `Self` or one of the impl's parameters. A single file's, with no
/// glob `use` (a glob may bring any name); a name the program, its prelude or a `use` gives is no error
#[cold]
#[inline(never)]
fn unknown_impl_type(prog: &Program, src: &str, src0: &str, items: &[SrcItem], user: usize) -> Option<String> {
    let mut globs: Option<bool> = None;
    let known = |n: &str| {
        prog.struct_index.contains_key(n) || prog.aliases.contains_key(n) || prog.traits.contains_key(n) || prog.user_traits.contains(n) || prog.use_names.contains(n)
            || matches!(n, "Box" | "Vec" | "String" | "Option" | "Result" | "Self")
            // (std's traits: a bound past a binder, `dyn for<'x> Fn(&'x A)`)
            || STD_TRAITS.contains(&n)
            || prog.consts.iter().any(|c| c.name == n)
    };
    for it in items[..user.min(items.len())].iter().filter(|it| matches!(it.kind, crate::lexer::ItemKind::ImplHeader)) {
        // (the header's identifiers first: one whose capitalized names are all known is not tokenized)
        if !it.idents.iter().any(|&(a, b)| src.get(a as usize..b as usize).is_some_and(|n| n.starts_with(|c: char| c.is_ascii_uppercase()) && !known(n))) {
            continue;
        }
        let text = src.get(it.start..it.end)?;
        let tb = text.as_bytes();
        let toks = crate::lexer::tokenize(tb);
        let tk = |k: usize| toks.get(k).map_or("", |t| &text[t.1..t.2]);
        let sym = |k: usize, c: u8| toks.get(k).is_some_and(|t| t.0 == 1 && t.2 == t.1 + 1 && tb[t.1] == c);
        // (a `>` that closes an angle: not the `>` of `->` (`F: Fn(&i32) -> &i32`))
        let close = |k: usize| sym(k, b'>') && !(k >= 1 && sym(k - 1, b'-') && toks[k - 1].2 == toks[k].1);
        let Some(imp) = (0..toks.len()).find(|&k| toks[k].0 == 0 && tk(k) == "impl") else { continue };
        // (the impl's own parameters: a name after `<` or `,` at depth 1, past `const`)
        let mut params: Vec<&str> = vec![];
        let mut k = imp + 1;
        if sym(k, b'<') {
            let mut d = 0i32;
            while k < toks.len() {
                if sym(k, b'<') {
                    d += 1;
                } else if close(k) {
                    d -= 1;
                    if d == 0 {
                        k += 1;
                        break;
                    }
                } else if d == 1 && (sym(k - 1, b'<') || sym(k - 1, b',')) && toks[k].0 == 0 {
                    let n = if tk(k) == "const" { tk(k + 1) } else { tk(k) };
                    params.push(n);
                }
                k += 1;
            }
        }
        // (the trait: the first name before a `for` at depth 0)
        let mut d = 0i32;
        let has_for = (k..toks.len()).any(|j| {
            if sym(j, b'<') || sym(j, b'(') || sym(j, b'[') {
                d += 1;
            } else if close(j) || sym(j, b')') || sym(j, b']') {
                d -= 1;
            }
            d == 0 && toks[j].0 == 0 && tk(j) == "for" && !sym(j + 1, b'<')
        });
        let trait_at = if has_for { Some(k) } else { None };
        while k < toks.len() && !sym(k, b'{') && tk(k) != "where" {
            let n = tk(k);
            let named = toks[k].0 == 0 && n.starts_with(|c: char| c.is_ascii_uppercase());
            // (a name after `'` is a lifetime (`for<'A> fn(&'A ())`); one before `:` is a path segment (`Foo::`), a
            // fn pointer's parameter name (`fn(Arg: u8)`) or an associated type's bound)
            let skip = !named
                || (toks[k].1 > 0 && tb[toks[k].1 - 1] == b'\'')
                || Some(k) == trait_at
                || n == "Self"
                || params.contains(&n)
                || sym(k + 1, b':')
                || (k >= 2 && sym(k - 1, b':') && sym(k - 2, b':'))
                || sym(k + 1, b'!')
                || sym(k + 1, b'=')
                || (k >= 1 && (sym(k - 1, b':') || sym(k - 1, b'+') || matches!(tk(k - 1), "dyn" | "impl")));
            if !skip && !known(n) {
                // (read once: whether a `use` of the source as written is a glob)
                if *globs.get_or_insert_with(|| src0.contains("::*") || src0.contains("::{*")) {
                    return None;
                }
                // (an item of the name anywhere in the source as written: a const generic argument's const
                // (`Print<L>`), an item in a block or a macro's; read only for a name nothing else gives)
                let declared = ["const ", "static ", "struct ", "enum ", "union ", "type ", "trait ", "fn ", "mod "].iter().any(|kw| {
                    src0.match_indices(kw).any(|(i, _)| {
                        let r = &src0[i + kw.len()..];
                        r.starts_with(n) && r[n.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
                    })
                });
                if declared {
                    k += 1;
                    continue;
                }
                let off = it.start + toks[k].1;
                let line = src[..off].matches('\n').count() + 1;
                let col = off - src[..off].rfind('\n').map_or(0, |p| p + 1) + 1;
                return Some(format!("{line}:{col}: cannot find type `{n}` in this scope (E0412)"));
            }
            k += 1;
        }
    }
    None
}

/// A trait bound of a fn of the program's (its own type parameters', inline or in its where clause, and its `impl Tr`
/// parameters') that names no trait in scope: "cannot find trait `Nope` in this scope", at the name (rustc 1.100's
/// E0405; E0404 for `Debug` and `Hash`, std's derive macros of those names being in scope). Not a trait the program
/// declares nor one of std's, which zustc may not have loaded; std's traits outside its prelude (`Debug`, `Display`,
/// `Hash`, ..) only where the bound writes the name alone, no `use` of the program's writes it, and none is a glob
#[cold]
#[inline(never)]
fn unknown_trait_bound(prog: &Program, src: &str, src0: &str, items: &[SrcItem], user: usize) -> Option<String> {
    // (the names the `use`s of the source as written write, read once at the first std trait outside the prelude:
    // the preprocessing takes std's `use`s away; and whether one is a glob)
    let mut used: Option<(bool, crate::fxhash::FxSet<&str>)> = None;
    let mut imported = |n: &str| {
        let (globs, names) = used.get_or_insert_with(|| {
            let mut names: crate::fxhash::FxSet<&str> = Default::default();
            let mut globs = false;
            // (each `use ` that starts a declaration (after its line's indentation and a `pub`), to its `;`)
            for (k, _) in src0.match_indices("use ") {
                let ls = src0[..k].rfind('\n').map_or(0, |i| i + 1);
                let before = src0[ls..k].trim();
                if !(before.is_empty() || before.starts_with("pub")) {
                    continue;
                }
                let r = &src0[k + 4..src0[k..].find(';').map_or(src0.len(), |e| k + e)];
                globs |= r.contains('*');
                names.extend(r.split(|c: char| !(c == '_' || c.is_ascii_alphanumeric())).filter(|w| !w.is_empty()));
            }
            (globs, names)
        });
        *globs || names.contains(n)
    };
    for (i, sg) in prog.sigs.iter().enumerate() {
        let Some(ii) = prog.item_of[i].filter(|&ii| (ii as usize) < user) else { continue };
        for (_, bs) in &sg.generics {
            for b in bs {
                let name = b.split('<').next().unwrap_or("").trim();
                // (not a lifetime, `?Sized`, a path or a higher-ranked bound, nor `use<..>` (precise capturing))
                if name.is_empty() || name.starts_with('\'') || name.starts_with('?') || name.contains(' ') || name.contains("__") || matches!(name, "use" | "for" | "const" | "async") {
                    continue;
                }
                let known = prog.traits.contains_key(name) || prog.user_traits.contains(name) || STD_TRAITS.contains(&name);
                let std_outside = NON_PRELUDE_TRAITS.contains(&name);
                if known && !std_outside {
                    continue;
                }
                // (a name a `use` brings, std's that zustc does not know too (`use std::marker::ConstParamTy_;`))
                if prog.use_names.contains(name) || prog.user_traits.contains(name) {
                    continue;
                }
                // (where the item writes the name alone, not as a path's last segment (`fmt::Debug`); a std trait as
                // a bound (after `:`, `+`, `impl`, `dyn`), not a derive's expansion's)
                let it = &items[ii as usize];
                let text = src.get(it.start..it.end).unwrap_or("");
                let tb = text.as_bytes();
                let id = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
                let bound_at = |k: usize| {
                    let before = text[..k].trim_end();
                    !std_outside || before.ends_with([':', '+']) || before.ends_with("impl") || before.ends_with("dyn")
                };
                let Some(at) = text.match_indices(name).map(|(k, _)| k).find(|&k| {
                    (k == 0 || !id(tb[k - 1])) && tb.get(k + name.len()).is_none_or(|&c| !id(c)) && !text[..k].ends_with("::") && !text[k + name.len()..].starts_with("::")
                        && !text[k + name.len()..].trim_start().starts_with('!')
                        && bound_at(k)
                }) else {
                    continue;
                };
                // (a name a `use` brings, std's that zustc does not know too (`use std::marker::ConstParamTy_;`); read
                // last, as it reads the whole source)
                if imported(name) {
                    continue;
                }
                let off = it.start + at;
                let line = src[..off].matches('\n').count() + 1;
                let col = off - src[..off].rfind('\n').map_or(0, |k| k + 1) + 1;
                let code = if matches!(name, "Debug" | "Hash") { "E0404" } else { "E0405" };
                return Some(format!("{line}:{col}: cannot find trait `{name}` in this scope ({code})"));
            }
        }
    }
    None
}

/// A merge's diff marker in the source (`<<<<<<<` at a line's start, outside strings and comments) in place of the
/// analysis's error e: rustc's parser stops at it ("encountered diff marker", at its first column), after the lexer's
/// errors of delimiters only
#[cold]
#[inline(never)]
fn diff_marker_first(src: &str, e: String) -> String {
    let lexer = ["this file contains an unclosed delimiter", "closing delimiter: `"].iter().any(|m| e.contains(m));
    match crate::lexer::diff_marker_line(src.as_bytes()) {
        Some(l) if !lexer && src.contains("<<<<<<<") => format!("{l}:1: encountered diff marker"),
        _ => e,
    }
}

/// e with its leading line `L:` made l
fn with_line(e: &str, l: usize) -> String {
    let lead = e.trim_start();
    let n = lead.find(':').unwrap_or(0);
    format!("{l}{}", &lead[n..])
}

/// An error at a line of an item hoisted out of a fn body (prep::HOIST_MARK): at the line it was written on
pub(crate) fn unhoist_err(a: &Analysis, e: String) -> String {
    let d = Diagnostic::parse(&e);
    let lead = e.trim_start();
    if d.line == 0 || !lead.starts_with(|c: char| c.is_ascii_digit()) || (a.prelude_line0 > 0 && d.line >= a.prelude_line0) {
        return e;
    }
    match crate::prep::hoisted_line(&a.src, d.line) {
        Some(l) => with_line(&e, l),
        None => e,
    }
}

/// When (line, col) of src is a macro call (`name!(..)`), the place of the name n in its arguments. Read by tokens, so
/// a delimiter or the name inside a literal or a comment does not count. None when the program's definition of the
/// macro writes n itself (`($unused:ident) => { x }`): the name may come from its body, and stays at the call
#[cold]
#[inline(never)]
fn macro_arg_at(src: &str, line: usize, col: usize, n: &str) -> Option<(usize, usize)> {
    let ls = if line <= 1 { 0 } else { src.match_indices('\n').nth(line - 2)?.0 + 1 };
    let at = ls + col.checked_sub(1)?;
    let b = src.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let tk = |k: usize| toks.get(k).map_or("", |t| &src[t.1..t.2]);
    let sym = |k: usize, c: u8| toks.get(k).is_some_and(|t| t.0 == 1 && t.2 == t.1 + 1 && b[t.1] == c);
    // (the group that opens at token k: the index of its closing token)
    let group = |k: usize| {
        let (open, close) = match toks.get(k).filter(|t| t.0 == 1).map(|t| b[t.1])? {
            b'(' => (b'(', b')'),
            b'[' => (b'[', b']'),
            b'{' => (b'{', b'}'),
            _ => return None,
        };
        let mut d = 0i32;
        for e in k..toks.len() {
            if sym(e, open) {
                d += 1;
            } else if sym(e, close) {
                d -= 1;
                if d == 0 {
                    return Some(e);
                }
            }
        }
        None
    };
    let c = toks.iter().position(|t| t.1 == at && t.0 == 0)?;
    if !sym(c + 1, b'!') {
        return None;
    }
    let e = group(c + 2)?;
    let name = tk(c);
    if let Some(j) = (0..toks.len()).find(|&j| tk(j) == "macro_rules" && sym(j + 1, b'!') && tk(j + 2) == name) {
        let end = group(j + 3)?;
        if (j + 4..end).any(|q| toks[q].0 == 0 && tk(q) == n && !sym(q - 1, b'$')) {
            return None;
        }
    }
    let q = (c + 3..e).find(|&q| toks[q].0 == 0 && tk(q) == n)?;
    let off = toks[q].1;
    Some((src[..off].matches('\n').count() + 1, off - src[..off].rfind('\n').map_or(0, |p| p + 1) + 1))
}

/// 位置付きのエラー (行・桁は 1 始まり)。check / LSP が返す
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl Diagnostic {
    /// 内部の "L:C: msg" (か "L: msg") の形の文字列から作る
    pub fn parse(s: &str) -> Diagnostic {
        let b = s.as_bytes();
        let num = |from: usize| -> (usize, usize) {
            let mut i = from;
            while i < b.len() && b[i] == b' ' {
                i += 1;
            }
            let st = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            (s[st..i].parse().unwrap_or(0), i)
        };
        let (line, i) = num(0);
        if line > 0 && i < b.len() && b[i] == b':' {
            let (col, j) = num(i + 1);
            if col > 0 && j < b.len() && b[j] == b':' {
                return Diagnostic { line, col, msg: s[j + 1..].trim().to_string() };
            }
            return Diagnostic { line, col: 1, msg: s[i + 1..].trim().to_string() };
        }
        Diagnostic { line: 1, col: 1, msg: s.to_string() }
    }
}

impl Session {
    /// 型検査だけを行い、全診断を返す (`zustc check` と LSP)。到達しない関数も、総称関数は要求されたインスタンスごとに
    /// 検査する (コード生成までしてエラーを集め、結果はキャッシュに入れる: 次の検査・ビルドはそれを使う)。prelude の中の診断は出さない
    pub fn check(&mut self, src: &str) -> Vec<Diagnostic> {
        let user_lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
        self.checking = true;
        let r = self.analyze(src);
        self.checking = false;
        let mut a = match r {
            Ok(a) => a,
            Err(e) => return vec![Diagnostic::parse(&self.unhoist_analysis_err(src, diff_marker_first(src, e)))],
        };
        // 検査だけ: コード生成 (acg) で型を検査し、機械語は作らない (ZUSTC_CHECK_CODEGEN=1 で機械語まで作る)。
        // 検査済みの印は、要求したインスタンスへの再配置だけを持つ空の Blob を、ビルドとは別のキーで覚える
        // (ビルドの Blob があればそれも検査済みとして使う)
        let check_only = std::env::var_os("ZUSTC_CHECK_CODEGEN").is_none();
        a.inl.check_only = check_only;
        let mut tm = Timer { on: std::env::var_os("ZUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
        let (mut n_keys, mut n_hit, mut n_checked, mut n_memo) = (0, 0, 0, 0);
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut errs: Vec<String> = vec![];
        let mut done: FxSet<u32> = FxSet::default();
        // 前回の check の記録: 宣言の指紋が同じなら、本体のアイテムが変わっていない関数は検査済み (キーも読み戻しも要らない)。
        // 要求していたインスタンスは名前で辿り、本体が変わったものだけを要求し直す
        let mut old = match self.chk.take() {
            Some(m) if m.fp == a.iface_fp && m.lint == lint_bits(&a.prog) && check_only && std::env::var_os("ZUSTC_NO_CHECK_MEMO").is_none() => Some(m),
            _ => None,
        };
        // (記録を作るのは、続けて検査する (キャッシュを使う) セッションだけ: 1 度だけの `zustc check` では作らない)
        let keep = check_only && !self.opts.no_cache;
        let mut memo = ChkMemo { fp: a.iface_fp, lint: lint_bits(&a.prog), ents: FxMap::default() };
        let mut seen: FxSet<String> = FxSet::default();
        let name_of = |g: &Insts, id: u32| -> String {
            match g.get(id) {
                Some(i) => format!("{}\u{1}{}", a.prog.names[i.base as usize], i.sig.symbol),
                None => a.prog.names[id as usize].clone(),
            }
        };
        let body_hash = |name: &str| -> Option<Key> {
            let &id = a.prog.index.get(name)?;
            let ii = a.prog.item_of[id as usize]?;
            Some(a.items[ii as usize].hash)
        };
        let n = a.prog.names.len();
        let mut wave: Vec<u32> = (0..n)
            .filter(|&i| concrete(&a.prog, i) && a.prog.item_of[i].is_some_and(|ii| ii < a.prog.prelude_from) && !a.is_ext(i as u32))
            .map(|i| i as u32)
            .collect();
        // (then, once: the program's generic fns nothing instantiated, with a placeholder type for each type
        // parameter: acg keeps only the borrow and region checks' errors of such an instance)
        let mut placeholders = !check_only || std::env::var_os("ZUSTC_NO_PLACEHOLDERS").is_some();
        // (in the placeholders' round)
        let mut ph_round = false;
        let mut ph_depth = 0;
        // (a wave's functions to check, and what each gives the record: id, name, body, requested, ok)
        let mut todo: Vec<(u32, String, String, Key, Key)> = vec![];
        let mut recs: Vec<(u32, String, String, Option<Vec<u32>>, bool)> = vec![];
        loop {
            while !wave.is_empty() {
                for &id in &wave {
                    if !done.insert(id) {
                        continue;
                    }
                    let (name, body) = {
                        let g = insts.lock().unwrap();
                        (name_of(&*g, id), a.prog.names[g.body_of(id) as usize].clone())
                    };
                    if !seen.insert(name.clone()) {
                        continue;
                    }
                    // (記録から: 本体が同じなら、要求していたインスタンスを名前で辿る)
                    // (an entry taken is moved to the new record, not copied: each is taken once, as `seen` keeps a
                    // name from being looked up again)
                    if let Some(o) = &mut old {
                        if o.ents.get(&name).is_some_and(|e| e.ok && Some(e.body_hash) == body_hash(&e.body)) {
                            if let Some(e) = o.ents.remove(&name) {
                                n_memo += 1;
                                let mut st: Vec<String> = e.children.clone();
                                memo.ents.insert(name.clone(), e);
                                while let Some(c) = st.pop() {
                                    if seen.contains(&c) {
                                        continue;
                                    }
                                    let Some(ce) = o.ents.get(&c) else { continue };
                                    if ce.ok && Some(ce.body_hash) == body_hash(&ce.body) {
                                        seen.insert(c.clone());
                                        n_memo += 1;
                                        let Some(ce) = o.ents.remove(&c) else { continue };
                                        st.extend(ce.children.iter().cloned());
                                        memo.ents.insert(c, ce);
                                    } else {
                                        // (本体が変わったインスタンス: 要求し直して検査する)
                                        let _ = crate::build_cache::decode_blob(&ce.me, &a.prog, &insts);
                                    }
                                }
                                continue;
                            }
                        }
                    }
                    let k = self.key_of(&a, &insts, id);
                    let kc = k ^ CHECK_SALT;
                    n_keys += 1;
                    // (キャッシュにあれば検査済み: 読み戻すと要求していたインスタンスも要求し直す)
                    let hit = match self.get(k).and_then(|d| crate::build_cache::decode_blob(&d, &a.prog, &insts)) {
                        Some(b) => Some(b),
                        None if check_only => self.get(kc).and_then(|d| crate::build_cache::decode_blob(&d, &a.prog, &insts)),
                        None => None,
                    };
                    match hit {
                        Some(b) => {
                            n_hit += 1;
                            let req = b.relocs.iter().filter_map(|r| match r.target { acg::Target::Func(i) => Some(i), _ => None }).collect();
                            recs.push((id, name, body, Some(req), true));
                        }
                        None => todo.push((id, name, body, k, kc)),
                    }
                }
                // (the wave's misses: checked here or, when there are many, in forked children (check_many); their
                // results are taken in the wave's order either way, so the diagnostics do not depend on -j)
                let ids: Vec<u32> = todo.iter().map(|t| t.0).collect();
                let res = self.check_many(&a, &insts, &ids, check_only);
                for ((id, name, body, k, kc), r) in todo.drain(..).zip(res) {
                    n_checked += 1;
                    let mut ok = true;
                    let req = match r {
                        Ok((req, enc)) => {
                            if let Some(e) = enc.filter(|_| !self.opts.no_cache) {
                                self.put(if check_only { kc } else { k }, e);
                            }
                            Some(req)
                        }
                        Err(e) => {
                            // (a fn that reads a constant: its type errors are the constant's, at its line; what
                            // zustc cannot evaluate in a constant is not an error of the program)
                            match body.strip_prefix(self.const_fn.as_str()).and_then(|k| k.parse::<usize>().ok()).filter(|_| !self.const_lines.is_empty()) {
                                Some(k) => {
                                    // (nor a mismatch zustc's coercions may make: of a raw pointer, or of a type
                                    // that has no size, which rustc rejects another way)
                                    // (nor a reference to a type with a Deref, whose coercion zustc does not make)
                                    // (a cycle of constants, E0391, is the constant's too)
                                    let m = e.find("cycle detected when const-evaluating").or_else(|| e.find("mismatched types").filter(|&m| {
                                        let tys = &e[m..];
                                        let unsized_ty = |t: &str| t == "str" || t.starts_with('[') && !t.contains(';') || t.starts_with("dyn ");
                                        let derefs = |t: &str| {
                                            let n = t.strip_prefix('&').map(|x| x.strip_prefix("mut ").unwrap_or(x)).unwrap_or("");
                                            a.prog.struct_index.get(n).is_some_and(|&sid| a.prog.method(sid, "deref").is_some())
                                        };
                                        // (a coercion to a raw pointer zustc may not make: the expected type's)
                                        let expected = tys.split('`').nth(1).unwrap_or("");
                                        !expected.contains("*const") && !expected.contains("*mut") && !tys.split('`').skip(1).step_by(2).any(|t| unsized_ty(t) || derefs(t))
                                    }))
                                    // (`const BAD: i8 = -129;`: overflowing_literals, denied)
                                    .or_else(|| e.find("literal out of range for"));
                                    if let (Some(&l), Some(m)) = (self.const_lines.get(k), m) {
                                        errs.push(format!("{l}:1: {}", &e[m..]));
                                    }
                                }
                                None => errs.push(const_err_line(e, src, &a.prog)),
                            }
                            ok = false;
                            Some(vec![])
                        }
                    };
                    recs.push((id, name, body, req, ok));
                }
                // (記録)
                for (id, name, body, req, ok) in recs.drain(..) {
                    if !keep {
                        continue;
                    }
                    if let (Some(req), Some(bh)) = (req, body_hash(&body)) {
                        let g = insts.lock().unwrap();
                        let children: Vec<String> = req.iter().filter(|&&i| g.get(i).is_some()).map(|&i| name_of(&*g, i)).collect();
                        let mut me = vec![];
                        if g.get(id).is_some() {
                            // (a closure's body: with what its parent's lowering, which the record stands for, told it)
                            let bo = g.body_of(id);
                            let clo_facts = if g.kind_free.contains(&bo) || g.closure_rg.contains_key(&bo) { g.facts_of(bo) } else { vec![] };
                            let b = Blob { align: 1, code: vec![], relocs: vec![acg::BlobReloc { offset: 0, kind: 4, addend: 0, target: acg::Target::Func(id) }], datas: vec![], vtables: vec![], traps: vec![], line_base: None, code_ref: None, clo_facts };
                            crate::build_cache::encode_blob(&b, &a.prog, &*g, &mut me);
                        }
                        memo.ents.insert(name, ChkEnt { body, body_hash: bh, children, me, ok });
                    }
                }
                wave = std::mem::take(&mut insts.lock().unwrap().pending);
                wave.retain(|id| !done.contains(id));
                // (the placeholders' round follows the closures their bodies make, a few levels: an instance of
                // another generic fn they request may make another (`foo(p, |x| f(x))` in foo), and the generic fn
                // has a placeholder instance of its own)
                if ph_round {
                    ph_depth += 1;
                    let g = insts.lock().unwrap();
                    wave.retain(|&id| ph_depth < 4 && a.prog.closures.contains_key(&g.body_of(id)));
                }
            }
            if placeholders {
                break;
            }
            placeholders = true;
            ph_round = true;
            wave = placeholder_wave(&a, &insts);
            wave.retain(|id| !done.contains(id));
            if wave.is_empty() {
                break;
            }
        }
        if keep {
            self.chk = Some(memo);
        }
        tm.lap("check", format!("{n_memo} from the last check, {n_keys} keys, {n_hit} cached, {n_checked} checked"));
        // (E0080: each integer or bool constant of the program is evaluated, used or not, as rustc's; not only where a
        // fn reads it, whose check stops at its first error (`let _ = (A, B);` reports A's only). At the operation, of
        // the constant that has it: not of one that reads that constant. The same error found in a fn is one.
        // One with no operator or call of its own (`= 1;`, `= u8::MAX;`, `= A;`) has none. One that is not placed is
        // at the constant's line, as before)
        if !self.const_lines.is_empty() {
            let mut placed = vec![];
            // (each one's error, and whether it is placed at its operation; the others' at their lines)
            let mut errs_of: FxMap<&str, (String, bool)> = FxMap::default();
            let mut at_lines: Vec<(&str, String)> = vec![];
            let own = |c: &crate::program::ConstDef| c.item < a.prog.prelude_from && !a.ext_items.get(c.item as usize).copied().unwrap_or(false) && !c.name.contains("::");
            // (an operator, `<<` and `==` too, or a call: `T::W & 1` reads a failing associated constant)
            let ops = |e: &str| e.bytes().any(|b| matches!(b, b'+' | b'-' | b'*' | b'/' | b'%' | b'<' | b'>' | b'(' | b'&' | b'|' | b'^' | b'!')) || e.contains("==");
            for c in a.prog.consts.iter().filter(|c| (c.ty.is_int() || c.ty == Ty::Bool) && own(c) && ops(&c.expr)) {
                let Err(e) = a.prog.const_int(&c.name, 0) else { continue };
                if !e.ends_with("(E0080)") {
                    continue;
                }
                // (at its line in the source, or its item's (one a macro wrote))
                let eq = const_init_eq(src, &c.name);
                let at_line = match eq {
                    Some(eq) => format!("{}:1: in constant `{}`: {e}", src[..eq.min(src.len())].matches('\n').count() + 1, c.name),
                    None => unhoist_err(&a, format!("{}:1: in constant `{}`: {e}", a.items[c.item as usize].line, c.name)),
                };
                let at_op = eq.and_then(|eq| const_op_err(&a.prog, &c.name, src, eq)).flatten();
                errs_of.insert(&c.name, (e, at_op.is_some()));
                match at_op {
                    Some(m) => placed.push(m),
                    None => at_lines.push((&c.name, at_line)),
                }
            }
            // (and not at a constant whose error is that of a constant it reads that has it placed, `const X: u8 = Y +
            // 0;` (where a fn read it too); one that reads an associated constant has it at its line)
            let reads_placed = |n: &str, m: &str| const_reads(&a.prog, n).iter().any(|r| const_err_placed(&a.prog, &errs_of, r, m, 0));
            errs.retain(|e| !(e.ends_with("(E0080)") && e.split_once(": in constant `").and_then(|x| x.1.split_once("`: ")).is_some_and(|(n, m)| reads_placed(n, m))));
            errs.extend(placed);
            errs.extend(at_lines.into_iter().filter(|(n, e)| !e.split_once("`: ").is_some_and(|(_, m)| reads_placed(n, m))).map(|x| x.1));
        }
        self.const_lines.clear();
        if errs.is_empty() {
            errs.extend(a.late_err.clone());
        }
        // (E0170 is a lint: not where an attribute an item starts with allows it, read only when there is one)
        if errs.iter().any(|e| e.ends_with("(E0170)")) {
            allow_variant_bindings(src, &mut errs);
        }
        let mut d: Vec<Diagnostic> = errs.iter().map(|e| Diagnostic::parse(e)).collect();
        // (a name a macro call's argument writes, reported at the call (`foo!(\n    x\n)` of `($id:ident) => { $id }`):
        // at the argument, as rustc)
        for x in d.iter_mut() {
            if let Some(n) = x.msg.strip_prefix("cannot find value `").and_then(|r| r.split('`').next()) {
                if let Some((l, c)) = macro_arg_at(src, x.line, x.col, n) {
                    x.line = l;
                    x.col = c;
                }
            }
        }
        // (an item moved out of a fn body (`fn main() { fn helper() { .. } }`) is past the source's last line: its borrow
        // and region checks' errors are at the lines it was moved from, and the type errors that hold wherever it is
        // (hoisted_check_err_holds). Not its other errors: typeck does not know all such an item may name where it was
        // (`Self` of the impl around it), so they are left out as before)
        let holds = |m: &str| acg::borrow_check_error(m) || hoisted_check_err_holds(m);
        if d.iter().any(|x| x.line > user_lines && holds(&x.msg)) {
            if let Some(moved) = self.moved_lines(src) {
                for x in &mut d {
                    if x.line > user_lines && holds(&x.msg) {
                        if let Some(l) = moved_line(&moved, user_lines, x.line) {
                            x.line = l;
                            x.msg = moved_msg_lines(&x.msg, &moved, user_lines);
                        }
                    }
                }
            }
        }
        d.retain(|d| d.line <= user_lines);
        d.sort_by(|x, y| (x.line, x.col).cmp(&(y.line, y.col)));
        d.dedup();
        d
    }
}

/// An error of a constant's evaluation that has no place (`0:1: in constant `X`: overflow in constant`): at the line
/// of its initializer in the source (`const X: u8 = ..`, the `=`), as rustc's E0080; a position inside the message is
/// left out. rustc's E0080 of an operation (`attempt to compute ..`) is at the operation, without the constant's name
fn const_err_line(e: String, src: &str, prog: &Program) -> String {
    let Some(rest) = e.strip_prefix("0:1: in constant `") else { return e };
    let Some((full, tail)) = rest.split_once("`: ") else { return e };
    // (what rustc reports at the constant: an overflow, a transmute of sizes, mismatched types, a value made a `dyn
    // Tr + Send` / `+ Sync` whose type is not (E0277); not what zustc does not evaluate)
    let placed = ["overflow in constant", "cannot transmute between types of different sizes", "mismatched types", "cannot be sent between threads safely", "cannot be shared between threads safely"];
    if !placed.iter().any(|m| tail.contains(m)) && !tail.ends_with("(E0080)") {
        return e;
    }
    let name = full.rsplit("::").next().unwrap_or(full);
    let Some(eq) = const_init_eq(src, name) else { return e };
    let line = src[..eq.min(src.len())].matches('\n').count() + 1;
    // (a position inside: `21:2: overflow in constant`)
    let msg = match tail.split_once(": ") {
        Some((pos, m)) if pos.split(':').all(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit())) => m,
        _ => tail,
    };
    // (at the line, as before, when the operation is in another constant it reads or is not placed)
    if let Some(Some(m)) = msg.ends_with("(E0080)").then(|| const_op_err(prog, full, src, eq)).flatten() {
        return m;
    }
    format!("{line}:1: in constant `{name}`: {msg}")
}

/// Where the literal or comment at b[i] ends (past it): a string or char literal (literal_end), `//` to the line's
/// end, `/* .. */` (nested); None for anything else
fn literal_or_comment_end(b: &[u8], i: usize) -> Option<usize> {
    if let Some(e) = literal_end(b, i) {
        return Some(e);
    }
    if b[i] != b'/' {
        return None;
    }
    match b.get(i + 1) {
        Some(b'/') => Some(b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p)),
        // (`/* .. /* .. */ .. */` nests)
        Some(b'*') => {
            let (mut end, mut n) = (i, 0usize);
            while end < b.len() {
                if b[end..].starts_with(b"/*") {
                    n += 1;
                    end += 2;
                } else if b[end..].starts_with(b"*/") {
                    n -= 1;
                    end += 2;
                    if n == 0 {
                        break;
                    }
                } else {
                    end += 1;
                }
            }
            Some(end)
        }
        _ => None,
    }
}

/// Where the `=` of the initializer of the constant or static `name` is in the source (`const X: u8 = ..`; not one
/// inside its type's brackets, `[u8; N]`), or its `;` when it has none
fn const_init_eq(src: &str, name: &str) -> Option<usize> {
    let b = src.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    // (outside comments and literals: not `// const A: u8 = 1 / 0;`)
    let mut at = 0;
    loop {
        if at >= b.len() {
            return None;
        }
        if let Some(e) = literal_or_comment_end(b, at) {
            at = e;
            continue;
        }
        if b[at..].starts_with(name.as_bytes()) && (at == 0 || !word(b[at - 1])) && b.get(at + name.len()).is_none_or(|&c| !word(c)) {
            let before = src[..at].trim_end();
            if before.ends_with("const") || before.ends_with("static") {
                break;
            }
        }
        at += 1;
    }
    let mut eq = at + name.len();
    let mut depth = 0i32;
    while eq < b.len() {
        match b[eq] {
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => depth -= 1,
            b';' if depth <= 0 => break,
            b'=' if depth <= 0 && !matches!(b.get(eq + 1), Some(b'=') | Some(b'>')) && !matches!(b.get(eq.wrapping_sub(1)), Some(b'=') | Some(b'!') | Some(b'<') | Some(b'>')) => break,
            _ => {}
        }
        eq += 1;
    }
    Some(eq)
}

/// The constants constant `n`'s initializer names (`A`, `T::W`)
#[cold]
fn const_reads(prog: &Program, n: &str) -> Vec<String> {
    use crate::parse::TokKind;
    let Some(ts) = prog.const_index.get(n).and_then(|&ci| crate::parse::tokenize(&prog.consts[ci as usize].expr).ok()) else { return vec![] };
    let mut out = vec![];
    let colons = |k: usize| ts.get(k).is_some_and(|t| t.text == ":") && ts.get(k + 1).is_some_and(|t| t.text == ":");
    for (k, t) in ts.iter().enumerate().filter(|(_, t)| t.kind == TokKind::Ident) {
        // (`T::W`: an associated constant's two names; not `T` alone)
        if colons(k + 1) {
            continue;
        }
        let x = if k >= 3 && colons(k - 2) && ts[k - 3].kind == TokKind::Ident { format!("{}::{}", ts[k - 3].text, t.text) } else { t.text.clone() };
        // (once each: `C1 + C1` is followed once)
        if prog.const_index.contains_key(x.as_str()) && !out.contains(&x) {
            out.push(x);
        }
    }
    out
}

/// Whether the E0080 `msg` of a constant that reads constant `n` is n's, placed at n's operation, or that of a
/// constant n reads, placed so (errs_of: the error of each constant of the unused-constant pass, and whether it is
/// placed): so that it is not reported again at the constant that reads it
#[cold]
fn const_err_placed(prog: &Program, errs_of: &FxMap<&str, (String, bool)>, n: &str, msg: &str, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    match errs_of.get(n) {
        Some((m, true)) => m == msg,
        Some((m, false)) if m != msg => false,
        // (one the pass does not take, an associated constant: its error must be msg too)
        None if !prog.const_int(n, 0).is_err_and(|e| e == msg) => false,
        _ => const_reads(prog, n).iter().any(|r| const_err_placed(prog, errs_of, r, msg, depth + 1)),
    }
}

/// The E0080 of constant `name`, whose initializer follows the `=` at src[eq], at the operation: the initializer
/// evaluated again as parsed at its place in the source, Some(Some(`L:C: attempt to compute ..`)). Some(None) when
/// the operation is in another constant it reads (the error has no place in this one), None when the initializer is
/// not read back so (no `=`, a text that does not parse, or another result)
#[cold]
fn const_op_err(prog: &Program, name: &str, src: &str, eq: usize) -> Option<Option<String>> {
    use crate::parse::{parse_expr_at, pos_at, Pos};
    let b = src.as_bytes();
    if b.get(eq) != Some(&b'=') {
        return None;
    }
    // (to the `;` past brackets, literals (`b';'`) and comments; the text's own place)
    let close = |from: usize, stop: u8| {
        let (mut end, mut depth) = (from, 0i32);
        while end < b.len() {
            if let Some(e) = literal_or_comment_end(b, end) {
                end = e;
                continue;
            }
            match b[end] {
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' if depth == 0 => break,
                b']' | b')' | b'}' => depth -= 1,
                c if c == stop && depth == 0 => break,
                _ => {}
            }
            end += 1;
        }
        end
    };
    let parse_at = |from: usize, end: usize| parse_expr_at(src.get(from..end)?, pos_at(src, from, Pos { line: 1, col: 0 })).ok();
    let ty = prog.consts[*prog.const_index.get(name)? as usize].ty;
    let m = prog.eval_const_placed(&parse_at(eq + 1, close(eq + 1, b';'))?, ty).err().filter(|m| m.ends_with("(E0080)"))?;
    let place = m.split_once(": ").and_then(|(pos, msg)| Some((pos.split(':').map(|x| x.parse().ok()).collect::<Option<Vec<usize>>>()?, msg)));
    let Some((p, msg)) = place else { return Some(None) };
    let [line, col, al, ac] = p[..] else { return Some(None) };
    // (the operation's start: its line's start, then col - 1 chars)
    let ls = if line <= 1 { 0 } else { src.match_indices('\n').nth(line - 2)?.0 + 1 };
    let mut start = ls + src[ls..].char_indices().nth(col - 1)?.0;
    // (rustc's place of a parenthesized operation is its `(`, `1 + (255 + 1)`: a `(` before it whose text is that
    // operation (its operator at the same place), not a call's (`f(255 + 1)`))
    let at = Pos { line: al as u32, col: ac as u32 - 1 };
    loop {
        let before = src[..start].trim_end();
        let Some(o) = before.strip_suffix('(').map(str::len) else { break };
        if src[..o].trim_end().ends_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | ')' | ']' | '>' | '!')) {
            break;
        }
        let Some(mut x) = parse_at(o + 1, close(o + 1, 0)) else { break };
        while let Expr::Paren(y) = x {
            x = *y;
        }
        match x {
            Expr::Binary(_, _, _, sp) | Expr::Unary(_, _, sp) | Expr::MethodCall(.., sp) if sp.at == at => start = o,
            _ => break,
        }
    }
    let l = src[..start].matches('\n').count() + 1;
    let c = src[..start].rsplit('\n').next().map_or(0, |s| s.chars().count()) + 1;
    Some(Some(format!("{l}:{c}: {msg}")))
}

/// The line (1-based) in the source of line `line` of the prepared text, past the source's `user_lines`: in the
/// items moved out of fn bodies, appended in order, each after a newline (Prep::moved: each one's first line in the
/// source (0-based) and its newlines). None past them (the prelude's)
fn moved_line(moved: &[(usize, usize)], user_lines: usize, line: usize) -> Option<usize> {
    let mut start = user_lines + 1;
    for &(from, nl) in moved {
        if line >= start && line <= start + nl {
            return Some(from + 1 + (line - start));
        }
        start += nl + 1;
    }
    None
}

/// The positions `at L:C` a message names (`(first borrow at 14:22)`) on lines moved out of fn bodies: at the lines
/// they were moved from (moved_line)
fn moved_msg_lines(msg: &str, moved: &[(usize, usize)], user_lines: usize) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(i) = rest.find("at ") {
        out.push_str(&rest[..i + 3]);
        rest = &rest[i + 3..];
        let n = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
        if n > 0 && rest[n..].starts_with(':') {
            if let Some(l) = rest[..n].parse::<usize>().ok().filter(|&l| l > user_lines).and_then(|l| moved_line(moved, user_lines, l)) {
                out.push_str(&l.to_string());
                rest = &rest[n..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A bound whose text names a lifetime other than those a `for<..>` in it binds (a placeholder's bound is written
/// without its own `for<'a>`: `FnLike<&'a isize, &'a isize>` names 'a, `Get<for<'b> fn(&'b u32)>` none)
fn bound_names_lt(b: &str) -> bool {
    let nested = acg::regionck::nested_binder_lts(b);
    let cs: Vec<(usize, char)> = b.char_indices().collect();
    let mut k = 0;
    while k < cs.len() {
        if cs[k].1 == '\'' {
            let from = cs[k].0;
            let mut j = k + 1;
            while j < cs.len() && (cs[j].1.is_alphanumeric() || cs[j].1 == '_') {
                j += 1;
            }
            let to = cs.get(j).map_or(b.len(), |c| c.0);
            let l = &b[from..to];
            if l.len() > 1 && !nested.contains(&l) {
                return true;
            }
            k = j;
        } else {
            k += 1;
        }
    }
    false
}

/// The program's generic fns (not a trait's default methods, not a closure's body, not a dependency's) that no
/// instance was requested of: one instance of each with a placeholder for each type parameter (Program::placeholder),
/// as `zustc check` checks every fn. The ids requested
fn placeholder_wave(a: &Analysis, insts: &Mutex<Insts>) -> Vec<u32> {
    let p = &a.prog;
    let mut g = insts.lock().unwrap();
    g.lenient = true;
    let mut made: FxMap<u32, Vec<Vec<Ty>>> = FxMap::default();
    for i in &g.list {
        made.entry(i.base).or_default().push(i.args.clone());
    }
    // (the names of the program's own structs and enums that bound a type parameter by 'static (`struct St<T:
    // 'static>`): a fn of which instances are made whose body names one is checked with placeholders too, for the
    // T: 'static its types need (an instance with `&'x u8` for T does not tell: its regions are fresh there))
    let mut static_adts: Vec<&str> = p
        .adt_regions
        .iter()
        .filter(|&(&b, r)| r.ty_outlives.iter().any(|&(_, c)| c == crate::types::R_STATIC) && p.struct_item.get(b as usize).is_some_and(|&i| i < p.prelude_from))
        .map(|(&b, _)| {
            let n = p.structs[b as usize].name.as_str();
            let n = n.split('<').next().unwrap_or(n);
            n.rsplit("::").next().unwrap_or(n)
        })
        .collect();
    static_adts.sort_unstable();
    static_adts.dedup();
    for id in 0..p.names.len() {
        let sig = &p.sigs[id];
        // (a trait's default method no impl uses: its `Self` is a placeholder too, bounded by the trait)
        if !sig.is_generic() || !sig.has_body || p.closures.contains_key(&(id as u32)) {
            continue;
        }
        // (a fn of which instances are made: checked with them. Unless every instance gives a `Copy` type for a
        // parameter its bounds do not make `Copy` (`dup(1u32)` of `fn dup<T>(x: T) -> (T, T) { (x, x) }`): those hide
        // what moving a T does, so the body is checked with placeholders too)
        let (recheck, recheck_static) = match made.get(&(id as u32)) {
            Some(insts) => {
                let hides = |args: &Vec<Ty>| args.iter().zip(&sig.generics).any(|(&a, (_, bs))| !bs.iter().any(|b| b == "Copy") && acg::borrowck::is_copy(p, a, 0));
                let moves = insts.iter().all(hides);
                // (and one whose body names a struct or an enum that bounds a type parameter by 'static, while one of
                // its type parameters is not bounded by 'static)
                let statics = !static_adts.is_empty()
                    && sig.generics.iter().any(|(_, bs)| !bs.iter().any(|b| b == "'static"))
                    && p.item_of[id].and_then(|ii| a.items.get(ii as usize)).and_then(|it| a.src.get(it.start..it.end)).is_some_and(|body| static_adts.iter().any(|n| names_word(body, n)));
                if !moves && !statics {
                    continue;
                }
                (moves, statics)
            }
            None => (false, false),
        };
        // (not a dependency's: `is_ext` knows only concrete fns, the item's flag tells)
        if !p.item_of[id].is_some_and(|ii| ii < p.prelude_from && !a.ext_items.get(ii as usize).copied().unwrap_or(false)) {
            continue;
        }
        // (a placeholder has no trait's methods: a fn whose type parameters are bounded by more than marker traits
        // would most likely fail its typeck after the work of it. Not made)
        // (a placeholder calls its bounds' methods as the trait declares them, which says little of the regions; a fn
        // whose body calls a method a bound's trait declares is not made: its typeck is the cost, its check little)
        let ii = p.item_of[id].unwrap_or(0) as usize;
        let body = a.items.get(ii).and_then(|it| a.src.get(it.start..it.end)).unwrap_or("");
        // (a signature that names a lifetime or `'static` (`-> Box<dyn X + 'static>`): what its body does with the
        // associated types of its parameters' bounds may be required of those regions. Not others: checking each
        // generic fn that calls such a method costs its typeck (+4.9% of the traits KPI))
        let names_regions = sig.regions.as_deref().is_some_and(|r| {
            !r.lts.is_empty() || r.slots.iter().flatten().any(|&c| c == crate::types::R_STATIC) || r.ty_outlives.iter().any(|x| x.1 == crate::types::R_STATIC)
        });
        let calls_bound = sig.generics.iter().flat_map(|(_, bs)| bs.iter()).any(|b| {
            // (`Fn(..)` / `FnMut(..)` / `FnOnce(..)`: a call of the parameter is checked against the bound itself)
            if matches!(b.as_str(), "Copy" | "Sized" | "?Sized" | "Send" | "Sync" | "Unpin") || crate::ast_summary::parse_fn_bound(b).is_some() {
                return false;
            }
            // (nor where the bound's arguments name lifetimes (`F: FnLike<&'a isize, &'a isize>`): the call's types are
            // of them. Not those a type in them binds (`Get<for<'b> fn(&'b u32) -> &'b u32>`): a local the call's result
            // is held in takes them as its own regions, not bound ones)
            if bound_names_lt(b) {
                return false;
            }
            let tr = b.split('<').next().unwrap_or(b).trim();
            let tail = format!("::{tr}");
            let decls = p.traits.get(tr).or_else(|| p.traits.iter().find(|(k, _)| k.ends_with(&tail)).map(|(_, v)| v));
            // (a trait not known: as calling)
            // (but a method whose declaration names lifetimes (the trait's `'a`, its own): its call says something of
            // the regions. So does one that returns an associated type of its `Self` (`fn into_item(self) ->
            // Self::Item`): the placeholder's associated type, whose region is a universal one (`Box::new(v.into_item())`
            // as a `Box<dyn X + 'static>`, regions/regions-close-associated-type-into-object.rs))
            decls.is_none_or(|ds| {
                ds.iter().any(|&d| {
                    let called = p.names[d as usize].rsplit("::").next().is_some_and(|m| body.contains(&format!(".{m}(")) || body.contains(&format!("::{m}(")));
                    let self_assoc = names_regions && matches!(p.sigs[d as usize].ret, Ty::Assoc(x) if matches!(x.base, Ty::Param(_)));
                    called && !self_assoc && !p.sigs[d as usize].regions.as_deref().is_some_and(|r| !r.lts.is_empty())
                })
            })
        });
        if calls_bound {
            continue;
        }
        let args: Vec<Ty> = sig.generics.iter().enumerate().map(|(k, (n, bs))| Ty::Struct(p.placeholder(id as u32, k as u32, n, bs))).collect();
        // (a where clause of the fn's, or of its impl's header: `impl<'a, T> Tr for &'a W<T> where &'a T: Tr`)
        let header = match a.items.get(ii).map(|it| it.kind) {
            Some(crate::lexer::ItemKind::ImplMember(h) | crate::lexer::ItemKind::TraitMember(h)) => a.items.get(h as usize).and_then(|it| a.src.get(it.start..it.end)).unwrap_or(""),
            _ => "",
        };
        let wheres = body.contains("where") || header.contains("where");
        let r = g.request(p, id as u32, args);
        if let Ok(iid) = r {
            if recheck {
                g.recheck.insert(iid);
            }
            if recheck_static {
                g.recheck_static.insert(iid);
            }
            if wheres {
                g.where_bound.insert(iid);
            }
        }
    }
    std::mem::take(&mut g.pending)
}

/// Whether a text names w as a word of its own (`St` in `St::<T>(x)`, not in `Store`)
fn names_word(text: &str, w: &str) -> bool {
    let id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let b = text.as_bytes();
    text.match_indices(w).any(|(i, _)| !(i > 0 && id(b[i - 1])) && !b.get(i + w.len()).is_some_and(|&c| id(c)))
}

/// A generic associated type's lifetimes and the outlives among them its text writes, inline (`<'a: 'b>`) or in its
/// where clause (`where 'a: 'b + 'c`): (lifetime names in order, (from, to, offset in the text) with `'static` as -1,
/// whether one names a lifetime not its own (the trait's, the impl's), which is left out)
fn gat_outlives(t: &str) -> (Vec<String>, Vec<(i32, i32, usize)>, bool) {
    let mut names = vec![];
    let mut raw: Vec<(String, String, usize)> = vec![];
    let Some(lt) = t.find('<') else { return (names, vec![], false) };
    let mut depth = 0i32;
    let mut gt = None;
    for (i, c) in t[lt..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    gt = Some(lt + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(gt) = gt else { return (names, vec![], false) };
    // (a predicate `'a: 'b + 'c` at offset base of the text: its pairs)
    let mut preds = |s: &str, base: usize, raw: &mut Vec<(String, String, usize)>| {
        let mut start = 0;
        let mut d = 0i32;
        let bytes = s.as_bytes();
        for i in 0..=s.len() {
            let c = bytes.get(i).copied();
            match c {
                Some(b'<' | b'(' | b'[') => d += 1,
                Some(b'>' | b')' | b']') => d -= 1,
                _ => {}
            }
            if c.is_none() || (c == Some(b',') && d == 0) {
                let part = &s[start..i];
                // (at the bound, as rustc points: `'a2` of `'b2: 'a2`)
                if let Some(c) = part.find(':') {
                    let l = part[..c].trim();
                    if l.starts_with('\'') {
                        let mut at = c + 1;
                        for b in part[c + 1..].split('+') {
                            let lead = b.len() - b.trim_start().len();
                            if b.trim().starts_with('\'') {
                                raw.push((l.to_string(), b.trim().to_string(), base + start + at + lead));
                            }
                            at += b.len() + 1;
                        }
                    }
                }
                start = i + 1;
            }
        }
    };
    let gens = &t[lt + 1..gt];
    for g in gens.split(',') {
        let g = g.trim();
        if g.starts_with('\'') {
            names.push(g.split(':').next().unwrap_or(g).trim().to_string());
        }
    }
    preds(gens, lt + 1, &mut raw);
    // (the where clause: its last `where`, to the `;`)
    let rest = &t[gt..];
    if let Some(w) = rest.rfind("where") {
        let ws = gt + w + 5;
        let we = t[ws..].find(';').map_or(t.len(), |k| ws + k);
        preds(&t[ws..we], ws, &mut raw);
    }
    let idx = |n: &str| if n == "'static" { Some(-1) } else { names.iter().position(|x| x == n).map(|k| k as i32) };
    let rel: Vec<(i32, i32, usize)> = raw.iter().filter_map(|(a, b, o)| Some((idx(a)?, idx(b)?, *o))).collect();
    let outer = rel.len() < raw.len();
    (names, rel, outer)
}

/// E0276 for a generic associated type: the offset in the impl's text of an outlives between its own lifetimes that
/// the trait's declaration does not imply (by the lifetimes' places; the same number of them)
fn gat_stricter_outlives(decl: &str, imp: &str) -> Option<usize> {
    let (dn, drel, d_outer) = gat_outlives(&blank_comments(decl));
    let (mn, mrel, _) = gat_outlives(&blank_comments(imp));
    // (a declaration's outlives through a lifetime not its own (`'a: 't, 't: 'b` with the trait's 't) may imply one
    // of the impl's (`'a: 'b`): the header's arguments that relate them are not read, so it is not compared)
    if dn.len() != mn.len() || d_outer {
        return None;
    }
    let implied = |a: i32, b: i32| {
        if a == b || a == -1 {
            return true;
        }
        let (mut seen, mut work) = (vec![a], vec![a]);
        while let Some(x) = work.pop() {
            for &(p, q, _) in &drel {
                if p == x && !seen.contains(&q) {
                    if q == b || q == -1 {
                        return true;
                    }
                    seen.push(q);
                    work.push(q);
                }
            }
        }
        false
    };
    mrel.iter().find(|&&(a, b, _)| !implied(a, b)).map(|x| x.2)
}

/// The where clause of an item's header text: (subject, bounds) of each predicate, split at depth 0 (None: no where)
fn where_preds(head: &str) -> Option<Vec<(String, Vec<String>)>> {
    let ok = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
    let (w, _) = head.match_indices("where").find(|(i, _)| ok(head[..*i].chars().next_back()) && ok(head[i + 5..].chars().next()))?;
    let split = |s: &str, sep: char| -> Vec<String> {
        let mut depth = 0i32;
        let mut out = vec![String::new()];
        let mut prev = ' ';
        for c in s.chars() {
            // (the `>` of `->` (`W: Fn() -> u8`) closes nothing)
            let arrow = c == '>' && prev == '-';
            prev = c;
            match c {
                '>' if arrow => {}
                '<' | '(' | '[' => depth += 1,
                '>' | ')' | ']' => depth -= 1,
                c if c == sep && depth == 0 => {
                    out.push(String::new());
                    continue;
                }
                _ => {}
            }
            if let Some(l) = out.last_mut() {
                l.push(c);
            }
        }
        out.into_iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
    };
    let mut preds = vec![];
    for p in split(&head[w + 5..], ',') {
        // (the first `:` at depth 0 that is not half of a `::`)
        let b = p.as_bytes();
        let mut depth = 0i32;
        let mut at = None;
        for i in 0..b.len() {
            match b[i] {
                b'>' if i > 0 && b[i - 1] == b'-' => {}
                b'<' | b'(' | b'[' => depth += 1,
                b'>' | b')' | b']' => depth -= 1,
                b':' if depth == 0 && b.get(i + 1) != Some(&b':') && (i == 0 || b[i - 1] != b':') => {
                    at = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(at) = at else { continue };
        preds.push((p[..at].trim().to_string(), split(&p[at + 1..], '+')));
    }
    Some(preds)
}

/// Whether the parameter `n` of an item may be unsized: `n: ?Sized` / `n: Tr + ?Sized`, in the parameter list or a
/// where clause, read from the item's text (the `:` before a `?Sized` with only bounds between, and the parameter's
/// name before the `:`)
fn param_maybe_unsized(text: &str, n: &str) -> bool {
    {
        // (from the tokens: not a `?Sized` in a string, `#[doc = "K: ?Sized"]`)
        let b = text.as_bytes();
        let t = crate::lexer::tokenize(b);
        let w = |k: usize| if t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" };
        let sym = |k: usize, c: u8| t[k].0 == 1 && b[t[k].1] == c;
        (1..t.len()).any(|k| {
            if !(w(k) == "Sized" && sym(k - 1, b'?')) {
                return false;
            }
            // (back over the other bounds, `Tr<A, B> + 'a + std::fmt::Debug + Iterator<Item = u8> + FnOnce(u8) -> u8
            // +`, to the `:` after the name: anything inside a bound's `<..>` or `(..)`)
            let mut j = k - 1;
            let mut angle = 0i32;
            let mut paren = 0i32;
            while j > 0 {
                j -= 1;
                if sym(j, b')') {
                    paren += 1;
                    continue;
                }
                if sym(j, b'(') {
                    paren -= 1;
                    continue;
                }
                if paren > 0 {
                    continue;
                }
                if sym(j, b'>') && !(j > 0 && sym(j - 1, b'-')) {
                    angle += 1;
                    continue;
                }
                if sym(j, b'<') {
                    angle -= 1;
                    continue;
                }
                if angle > 0 {
                    continue;
                }
                // (the `->` of a `Fn(..) -> R` bound)
                if sym(j, b'-') && j + 1 < t.len() && sym(j + 1, b'>') {
                    continue;
                }
                if sym(j, b':') {
                    if j > 0 && sym(j - 1, b':') {
                        j -= 1;
                        continue;
                    }
                    return j > 0 && w(j - 1) == n;
                }
                if !(t[j].0 == 0 || sym(j, b'+') || sym(j, b'<') || sym(j, b'>') || sym(j, b',') || sym(j, b'?')) {
                    return false;
                }
            }
            false
        })
    }
}

/// E0277 "the size for values of type `[u8]` cannot be known at compilation time": an unsized type (`[T]`, `str`,
/// `dyn Tr`) written where a type must be Sized. Read from the written types of the program's items (their fn
/// signatures, fields and impl headers): an argument of std's containers that need Sized (`Vec<[i32]>`,
/// `Option<[u8]>`), or of a type or trait of the program's whose parameter does not say `?Sized`; an enum variant's
/// field, and a struct's field other than the last
fn sized_error(sums: &[Summary], src: &str, items: &[SrcItem], user: usize) -> Option<String> {
    use crate::types::TySyn;
    // (the last segment of a path: `std::vec::Vec` → `Vec`, a char search, not a str one)
    fn last(n: &str) -> &str {
        n.rfind(':').map_or(n, |i| &n[i + 1..])
    }
    fn unsized_ty(t: &TySyn) -> bool {
        matches!(t, TySyn::Slice(_) | TySyn::Str | TySyn::Dyn(_))
    }
    fn show(t: &TySyn) -> String {
        match t {
            TySyn::Slice(i) => format!("[{}]", show(i)),
            TySyn::Str => "str".into(),
            TySyn::Dyn(d) if d.contains('\'') => format!("(dyn {d})"),
            TySyn::Dyn(d) => format!("(dyn {d} + 'static)"),
            TySyn::Prim(p) => format!("{p:?}").to_lowercase(),
            TySyn::Named(n) => n.rsplit("::").next().unwrap_or(n).replace("__u_", ""),
            TySyn::App(n, a) => format!("{}<{}>", n.rsplit("::").next().unwrap_or(n).replace("__u_", ""), a.iter().map(show).collect::<Vec<_>>().join(", ")),
            TySyn::Ref(m, i) => format!("&{}{}", if *m { "mut " } else { "" }, show(i)),
            TySyn::Ptr(m, i) => format!("*{} {}", if *m { "mut" } else { "const" }, show(i)),
            TySyn::Tuple(ts) => format!("({})", ts.iter().map(show).collect::<Vec<_>>().join(", ")),
            _ => "_".into(),
        }
    }
    // (the quick look first: a written type with an unsized type where it may be one that needs Sized; not std's types
    // that allow one, `Box<dyn Fn()>`, `Rc<str>`)
    fn may(t: &TySyn) -> bool {
        match t {
            TySyn::App(n, a) => {
                let open = matches!(last(n), "Box" | "Rc" | "Arc" | "Weak" | "Cell" | "RefCell" | "UnsafeCell" | "Mutex" | "RwLock" | "PhantomData" | "NonNull" | "ManuallyDrop" | "Cow" | "Pin");
                a.iter().any(|x| (unsized_ty(x) && !open) || may(x))
            }
            TySyn::Slice(i) | TySyn::Array(i, _) => unsized_ty(i) || may(i),
            TySyn::Ref(_, i) | TySyn::Ptr(_, i) | TySyn::Assoc(i, _) => may(i),
            TySyn::Tuple(ts) => ts.iter().enumerate().any(|(k, x)| (k + 1 < ts.len() && unsized_ty(x)) || may(x)),
            _ => false,
        }
    }
    let user_sums = &sums[..user.min(sums.len()).min(items.len())];
    // (an item's written types, in order: its fns' signatures, its fields, its impl header)
    fn sig_tys(sm: &Summary) -> impl Iterator<Item = &TySyn> {
        sm.fns
            .iter()
            .flat_map(|(_, sig)| sig.params.iter().chain(std::iter::once(&sig.ret)))
            .chain(sm.structs.iter().flat_map(|(_, fs)| fs.iter().map(|f| &f.1)))
            .chain(sm.enums.iter().flat_map(|(_, vs)| vs.iter().flat_map(|x| x.fields.iter().map(|f| &f.1))))
            .chain(sm.impl_self_ty.iter())
            .chain(sm.impl_trait_args.iter())
    }
    // (a field that must be Sized (a variant's, a struct's before the last) of a type that is not, or (`param`) of a
    // parameter of the item's, which is one when the item says `?Sized` for it)
    let fields_unsized = |sm: &Summary, param: bool| {
        let bad = |t: &TySyn| if param { matches!(t, TySyn::Named(n) if sm.adt_generics.iter().any(|g| g == n)) } else { unsized_ty(t) };
        sm.enums.iter().any(|(_, vs)| vs.iter().any(|x| x.fields.iter().any(|f| bad(&f.1))))
            || sm.structs.iter().any(|(_, fs)| fs.len() > 1 && fs[..fs.len() - 1].iter().any(|f| bad(&f.1)))
    };
    // (without collecting: most items have nothing to look at)
    // (a primitive or a name has nothing in it: no call)
    #[inline(always)]
    fn may0(t: &TySyn) -> bool {
        !matches!(t, TySyn::Prim(_) | TySyn::Named(_) | TySyn::Str | TySyn::Infer) && may(t)
    }
    let any_may = |sm: &Summary| {
        sm.fns.iter().any(|(_, sig)| sig.params.iter().any(may0) || may0(&sig.ret))
            || sm.structs.iter().any(|(_, fs)| fs.iter().any(|f| may0(&f.1)))
            || sm.enums.iter().any(|(_, vs)| vs.iter().any(|x| x.fields.iter().any(|f| may0(&f.1))))
            || sm.impl_self_ty.as_ref().is_some_and(may0)
            || sm.impl_trait_args.iter().any(may0)
    };
    // (the items to read: a written type that may have one, or a field of a parameter of an item that writes `?Sized`)
    let text_of = |ii: usize| src.get(items[ii].start..items[ii].end).unwrap_or("");
    let cands: Vec<usize> = (0..user_sums.len())
        .filter(|&ii| {
            let sm = &user_sums[ii];
            any_may(sm) || fields_unsized(sm, false) || (fields_unsized(sm, true) && text_of(ii).contains("?Sized"))
        })
        .collect();
    if cands.is_empty() {
        return None;
    }
    // (std's containers whose parameters need Sized; the program's own types and traits by their text)
    const STD_SIZED: &[&str] = &["Vec", "Option", "Result", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque", "BinaryHeap", "LinkedList", "MaybeUninit"];
    let needs_sized = |name: &str, k: usize| -> bool {
        let n = last(name).trim_start_matches("__u_");
        for (ii, sm) in user_sums.iter().enumerate() {
            let own_adt = sm.structs.iter().any(|x| last(&x.0) == n) || sm.enums.iter().any(|x| last(&x.0) == n);
            let own_trait = sm.trait_decl.as_deref().is_some_and(|t| last(t) == n);
            if !own_adt && !own_trait {
                continue;
            }
            let gens: Vec<&str> = if own_adt { sm.adt_generics.iter().map(String::as_str).collect() } else { sm.trait_generics.iter().map(|g| g.0.as_str()).filter(|g| !g.starts_with('#')).collect() };
            let Some(g) = gens.get(k) else { return false };
            let text = src.get(items[ii].start..items[ii].end).unwrap_or("");
            return !param_maybe_unsized(text, g);
        }
        STD_SIZED.contains(&n)
    };
    // (the first argument of a written type that is unsized where it must not be: the type it is given to, and it)
    fn find<'a>(t: &'a TySyn, needs: &dyn Fn(&str, usize) -> bool) -> Option<(Option<&'a str>, &'a TySyn)> {
        match t {
            TySyn::App(n, a) => a.iter().enumerate().find_map(|(k, x)| if unsized_ty(x) && needs(n, k) { Some((Some(n.as_str()), x)) } else { find(x, needs) }),
            TySyn::Slice(i) | TySyn::Array(i, _) => if unsized_ty(i) { Some((None, &**i)) } else { find(i, needs) },
            TySyn::Ref(_, i) | TySyn::Ptr(_, i) | TySyn::Assoc(i, _) => find(i, needs),
            TySyn::Tuple(ts) => ts.iter().enumerate().find_map(|(k, x)| if k + 1 < ts.len() && unsized_ty(x) { Some((None, x)) } else { find(x, needs) }),
            _ => None,
        }
    }
    let ident = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
    for ii in cands {
        let sm = &user_sums[ii];
        let it = &items[ii];
        let text = text_of(ii);
        // (where the item writes `name`, after its attributes: the line and column)
        let at = |name: &str| -> String {
            // (as the prepared text writes it: a type of the program's renamed, `__u_Vec`, or as written)
            let raw = last(name);
            let b = text.as_bytes();
            let from = leading_attrs(text).len();
            let find = |w: &str| text[from..].match_indices(w).map(|(i, _)| from + i).find(|&i| (i == 0 || !ident(b[i - 1])) && b.get(i + w.len()).is_none_or(|&c| !ident(c)));
            let pos = find(raw).or_else(|| find(raw.trim_start_matches("__u_")));
            match pos {
                Some(i) => {
                    let abs = it.start + i;
                    format!("{}:{}", it.line + text[..i].matches('\n').count(), abs - src[..abs].rfind('\n').map_or(0, |p| p + 1) + 1)
                }
                None => format!("{}:1", it.line),
            }
        };
        let msg = |t: &TySyn| format!("the size for values of type `{}` cannot be known at compilation time (E0277)", show(t));
        // (an enum variant's field, a struct's field before the last: at the field. A field of the item's parameter
        // that says `?Sized` is one too)
        let unsized_field = |t: &TySyn| unsized_ty(t) || matches!(t, TySyn::Named(n) if sm.adt_generics.iter().any(|g| g == n) && param_maybe_unsized(text, n));
        for (_, vs) in &sm.enums {
            for (vi, v) in vs.iter().enumerate() {
                if let Some((fi, f)) = v.fields.iter().enumerate().find(|(_, f)| unsized_field(&f.1)) {
                    let (l, c) = field_pos(text, true, vi, fi).unwrap_or((0, 0));
                    return Some(format!("{}:{}: {}", it.line + l, c + 1, msg(&f.1)));
                }
            }
        }
        for (_, fs) in &sm.structs {
            if sm.unions.is_empty() && fs.len() > 1 {
                if let Some((fi, f)) = fs[..fs.len() - 1].iter().enumerate().find(|(_, f)| unsized_field(&f.1)) {
                    let (l, mut c) = field_pos(text, false, 0, fi).unwrap_or((0, 0));
                    // (a named field's type, after its `name:`, as rustc points)
                    if let Some(line) = text.lines().nth(l).filter(|_| !f.0.chars().all(|ch| ch.is_ascii_digit())) {
                        if let Some(k) = line.get(c..).and_then(|r| r.find(':')) {
                            c += k + 1 + line[c + k + 1..].len() - line[c + k + 1..].trim_start().len();
                        }
                    }
                    return Some(format!("{}:{}: {}", it.line + l, c + 1, msg(&f.1)));
                }
            }
        }
        for t in sig_tys(sm) {
            if !may(t) {
                continue;
            }
            if let Some((outer, x)) = find(t, &needs_sized) {
                let pos = outer.map_or_else(|| format!("{}:1", it.line), |o| at(o));
                return Some(format!("{pos}: {}", msg(x)));
            }
        }
    }
    None
}

/// An item's header: its text before the first `{` token (not one in a string or a comment: `#[doc = "{"]`)
fn header_text(text: &str) -> &str {
    let b = text.as_bytes();
    let t = crate::lexer::tokenize(b);
    match t.iter().find(|x| x.0 == 1 && b[x.1] == b'{') {
        Some(x) => &text[..x.1],
        None => text,
    }
}

/// E0277, Sized: the program's definitions and signatures that put a maybe-unsized type where only a sized one may be
fn sized_wf_error(prog: &Program, sums: &[Summary], src: &str, items: &[SrcItem], user: usize) -> Option<String> {
    let user_sm = |ii: usize| ii < user;
    // (a parameter declared `?Sized` whose other bounds imply Sized is sized: `T: Clone + ?Sized`)
    let implies_sized = |bs: &[String]| bs.iter().any(|b| {
        let tr = b.split('<').next().unwrap_or(b).trim();
        let tr = tr.rsplit("::").next().unwrap_or(tr);
        matches!(tr, "Sized" | "Clone" | "Copy" | "Default" | "From" | "Into" | "TryFrom" | "TryInto" | "FromStr" | "FromIterator")
            || prog.bound_implies(b, &["Sized", "Clone", "Copy", "Default"], false, 0)
    });
    let maybe_unsized = param_maybe_unsized;
    // (E0277, Sized: a type parameter declared `?Sized`, or an unsized type, where only a sized type may be: a struct's
    // field other than the last, an enum's or a union's field, an argument for a parameter not declared `?Sized`, a
    // tuple's element other than the last, a slice's or array's element, a free fn's argument or result)
    let size_msg = |t: Ty, gens: &[String]| {
        // (a struct unsized by a parameter in its tail is named by that parameter, as rustc does)
        let t = match t {
            Ty::Struct(s) => prog.structs[s as usize].args.iter().rev().copied().find(|a| matches!(a, Ty::Param(_))).unwrap_or(t),
            t => t,
        };
        let n = match t {
            Ty::Param(i) => gens.get(i as usize).cloned().unwrap_or_default(),
            t => crate::traits::display_ty(prog, t),
        };
        format!("the size for values of type `{n}` cannot be known at compilation time (E0277)")
    };
    let mut sids: Vec<u32> = (0..prog.structs.len() as u32).filter(|&s| prog.structs[s as usize].base.is_none() && prog.struct_item.get(s as usize).is_some_and(|&ii| user_sm(ii as usize))).collect();
    sids.sort_by_key(|&s| prog.struct_item[s as usize]);
    for sid in sids {
        let sd = &prog.structs[sid as usize];
        if prog.is_closure(sid) || prog.is_tuple(sid) || sd.placeholder.is_some() {
            continue;
        }
        let ii = prog.struct_item[sid as usize] as usize;
        let (Some(it), Some(sm)) = (items.get(ii), sums.get(ii)) else { continue };
        let union = sm.unions.iter().any(|u| *u == sd.name || sd.name.ends_with(&format!("::{u}")));
        let adt_b = |g: &str| sm.adt_bounds.iter().find(|(p, _)| p == g).map(|(_, b)| b.clone()).unwrap_or_default();
        let param_may = |t: Ty| matches!(t, Ty::Param(i) if sm.adt_generics.get(i as usize).is_some_and(|g| sm.adt_unsized.contains(g) && !implies_sized(&adt_b(g))));
        let may = |t: Ty| prog.maybe_unsized_with(t, &param_may);
        let fields: Vec<&[(String, Ty, u32)]> = if sd.is_enum { sd.variants.iter().map(|v| v.fields.as_slice()).collect() } else { vec![sd.fields.as_slice()] };
        for (vi, fs) in fields.iter().enumerate() {
            for (fi, (_, ft, _)) in fs.iter().enumerate() {
                let bad = if (sd.is_enum || union || fi + 1 < fs.len()) && may(*ft) { Some(*ft) } else { prog.sized_wf_with(*ft, &may, 0) };
                if let Some(b) = bad {
                    let text = blank_comments(src.get(it.start..it.end).unwrap_or(""));
                    let (l, c) = field_pos(&text, sd.is_enum, vi, fi).unwrap_or((0, 0));
                    return Some(format!("{}:{}: {}", it.line + l, c + 1, size_msg(b, &sm.adt_generics)));
                }
            }
        }
    }
    // (the first declaration of each trait of the program's, by name: built at the first impl of a trait. A search of
    // every summary for each impl was quadratic in a program of many impls)
    let mut decl_of: Option<FxMap<&str, usize>> = None;
    for (ii, sm) in sums.iter().enumerate().take(user.min(items.len())) {
        // (an impl's self type and trait arguments: `impl Foo for Option<[u8]>`)
        if let Some(st) = &sm.impl_self_ty {
            if matches!(items[ii].kind, ItemKind::Normal | ItemKind::ImplHeader) {
                let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                let gens: Vec<String> = gn.iter().map(|s| s.to_string()).collect();
                // (the header's text only where the item writes `?Sized`: most impls do not, and reading every item's
                // tokens costs the check of a large program)
                let raw = src.get(items[ii].start..items[ii].end).unwrap_or("");
                let head = if raw.contains("?Sized") { header_text(&blank_comments(raw)).to_string() } else { String::new() };
                let param_may = |t: Ty| matches!(t, Ty::Param(i) if gens.get(i as usize).is_some_and(|n| maybe_unsized(&head, n)) && !implies_sized(&sm.impl_generics[i as usize].1));
                let may = |t: Ty| prog.maybe_unsized_with(t, &param_may);
                for t in std::iter::once(st).chain(sm.impl_trait_args.iter()) {
                    let Ok(t) = prog.resolve_g(t, &gn) else { continue };
                    if let Some(b) = prog.sized_wf_with(t, &may, 0) {
                        return Some(format!("{}:1: {}", items[ii].line, size_msg(b, &gens)));
                    }
                }
                // (a trait argument that may be unsized for a parameter of the program's trait not declared `?Sized`:
                // `impl<T: ?Sized> Tr<T> for ()` with `trait Tr<T> {}`)
                let tr = sm.impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
                let tr = tr.map(|t| t.rsplit("::").next().unwrap_or(t));
                let decls = decl_of.get_or_insert_with(|| {
                    let mut m: FxMap<&str, usize> = FxMap::default();
                    for (di, d) in sums[..user.min(sums.len())].iter().enumerate() {
                        if let Some(t) = d.trait_decl.as_deref() {
                            m.entry(t).or_insert(di);
                        }
                    }
                    m
                });
                if let Some((di, d)) = tr.and_then(|tr| decls.get(tr)).map(|&di| (di, &sums[di])) {
                    let tparams: Vec<&String> = d.trait_generics.iter().map(|g| &g.0).filter(|g| !g.starts_with('#') && *g != "Self").collect();
                    // (the arguments written fill the leading parameters: the rest have defaults, `trait Tr<T, U = ()>`)
                    if sm.impl_trait_args.len() <= tparams.len() {
                        let dhead = || header_text(&blank_comments(src.get(items[di].start..items[di].end).unwrap_or(""))).to_string();
                        for (k, a) in sm.impl_trait_args.iter().enumerate() {
                            let Ok(a) = prog.resolve_g(a, &gn) else { continue };
                            if may(a) && !maybe_unsized(&dhead(), tparams[k]) {
                                return Some(format!("{}:1: {}", items[ii].line, size_msg(a, &gens)));
                            }
                        }
                    }
                }
            }
            continue;
        }
        if sm.trait_decl.is_some() || !matches!(items[ii].kind, ItemKind::Normal | ItemKind::ImplMember(_)) {
            continue;
        }
        // (a fn's signature, free or of an impl: `fn iceman(c: Vec<[i32]>)`, `fn g1<X: ?Sized>(x: X)`, `-> [u8]`. An
        // impl's parameters are taken as sized: their `?Sized` is not known here)
        for (_, sig) in &sm.fns {
            if sig.trait_of.is_some() || sig.is_extern || !sig.has_body {
                continue;
            }
            let gn: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
            let gens: Vec<String> = gn.iter().map(|s| s.to_string()).collect();
            let param_may = |t: Ty| matches!(t, Ty::Param(i) if gens.get(i as usize).is_some_and(|n| sig.own_unsized.contains(n)) && !implies_sized(&sig.generics[i as usize].1));
            let may = |t: Ty| prog.maybe_unsized_with(t, &param_may);
            for (k, t) in sig.params.iter().chain([&sig.ret]).enumerate() {
                let Ok(t) = prog.resolve_g(t, &gn) else { continue };
                // (a value of the type itself: an argument, or the result)
                let by_value = may(t) && !(k == sig.params.len() && matches!(t, Ty::Dyn(_)));
                if let Some(b) = if by_value { Some(t) } else { prog.sized_wf_with(t, &may, 0) } {
                    // (at the argument's type or the result's, as rustc: a signature may span lines)
                    let part = if k == sig.params.len() { "r".to_string() } else { format!("p{k}") };
                    let it = &items[ii];
                    let at = src.get(it.start..it.end.min(src.len())).and_then(|t| sig_part(t, &part)).map(|o| it.start + o);
                    let pos = match at {
                        Some(at) => format!("{}:{}", src[..at].matches('\n').count() + 1, at - src[..at].rfind('\n').map_or(0, |p| p + 1) + 1),
                        None => format!("{}:1", it.line),
                    };
                    return Some(format!("{pos}: {}", size_msg(b, &gens)));
                }
            }
        }
    }
    None
}

/// Well-formedness of the program's struct fields and impl headers: a type that names a struct of the program's whose
/// parameter is bounded (`struct S<T: Tr>`) needs the bound of what it gives the parameter (E0277). A parameter of
/// the enclosing item holds it when its own bounds give it, or when the item names the trait anywhere (a where clause
/// zustc does not read, a bound with arguments); a concrete type when bound_holds says so (the program's traits, and
/// the prelude's for the types `implements` judges exactly)
fn wf_error(prog: &Program, sums: &[Summary], src: &str, items: &[SrcItem], user: usize) -> Option<String> {
    let user_sm = |ii: usize| ii < user;
    // (the bounded parameters of the program's structs named in t, with what t gives them)
    fn needs(prog: &Program, sums: &[Summary], user: usize, t: Ty, out: &mut Vec<(Ty, String)>, depth: u32) {
        if depth > 16 {
            return;
        }
        match t {
            Ty::Struct(s) => {
                let sd = &prog.structs[s as usize];
                if let Some(b) = sd.base {
                    if let Some(sm) = prog.struct_item.get(b as usize).map(|&bi| bi as usize).filter(|&bi| bi < user).and_then(|bi| sums.get(bi)) {
                        for (pn, trs) in &sm.adt_bounds {
                            let Some(&a) = sm.adt_generics.iter().position(|g| g == pn).and_then(|k| sd.args.get(k)) else { continue };
                            out.extend(trs.iter().filter(|tr| !tr.starts_with(crate::types::CONST_GENERIC)).map(|tr| (a, tr.clone())));
                        }
                    }
                    for &a in &sd.args {
                        needs(prog, sums, user, a, out, depth + 1);
                    }
                }
            }
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => needs(prog, sums, user, *i, out, depth + 1),
            Ty::Fn(f) => {
                for &p in f.params.iter().chain([&f.ret]) {
                    needs(prog, sums, user, p, out, depth + 1);
                }
            }
            _ => {}
        }
    }
    // (`n: ?Sized` / `n: Tr + ?Sized`, in the parameter list or a where clause: the `:` before a `?Sized` with only
    // bounds between, and the parameter's name before the `:`)
    let maybe_unsized = |text: &str, n: &str| param_maybe_unsized(text, n);
    // (a bound whose trait has `Sized` as a supertrait: std's `Clone`, `Copy`, `Default`, the conversions, or one of
    // the program's traits through its supertraits)
    let implies_sized = |b: &str| {
        let tr = b.split('<').next().unwrap_or(b).trim();
        let tr = tr.rsplit("::").next().unwrap_or(tr);
        matches!(tr, "Sized" | "Clone" | "Copy" | "Default" | "From" | "Into" | "TryFrom" | "TryInto" | "FromStr" | "FromIterator")
            || prog.bound_implies(b, &["Sized", "Clone", "Copy", "Default"], false, 0)
    };
    let word = |text: &str, w: &str| text.match_indices(w).any(|(i, _)| {
        let ok = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        ok(text[..i].chars().next_back()) && ok(text[i + w.len()..].chars().next())
    });
    // (the first need that fails, with the enclosing item's parameters (names, bounds) and its text)
    let failing = |ns: &[(Ty, String)], gens: &[String], bounds: &dyn Fn(&str) -> Vec<String>, text: &str| -> Option<String> {
        for (a, tr) in ns {
            match *a {
                Ty::Param(i) => {
                    let Some(n) = gens.get(i as usize) else { continue };
                    // (a parameter is Sized unless it says `?Sized` (which the recorded bounds leave out): then only a
                    // bound that implies Sized holds it, not the word `Sized` of its `?Sized`)
                    let bs = bounds(n);
                    let held = if tr == "Sized" {
                        bs.iter().any(|b| implies_sized(b)) || !maybe_unsized(text, n)
                    } else {
                        bs.iter().any(|b| b == tr || prog.bound_implies(b, &[tr.as_str()], true, 0)) || word(text, tr)
                    };
                    if !held {
                        return Some(format!("the trait bound `{n}: {tr}` is not satisfied (E0277)"));
                    }
                }
                a if !prog.has_param(a) => {
                    let own = sums[..user.min(sums.len())].iter().any(|s| s.trait_decl.as_deref() == Some(tr.as_str()));
                    let judged = a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char | Ty::Unit) || matches!(a, Ty::Struct(x) if prog.structs[x as usize].base.is_none() && prog.structs[x as usize].generics.is_empty());
                    let prelude = !own && judged && sums.iter().any(|s| s.trait_decl.as_deref() == Some(tr.as_str()));
                    if (own || prelude) && !prog.bound_holds(sums, user, a, tr, 0) {
                        let n = prog.tn(a).replace("__u_", "");
                        // (std's message for Iterator: `u8` is not an iterator)
                        return Some(if tr == "Iterator" { format!("`{n}` is not an iterator (E0277)") } else { format!("the trait bound `{n}: {tr}` is not satisfied (E0277)") });
                    }
                }
                _ => {}
            }
        }
        None
    };
    // (the fields of the program's structs and enums)
    let mut sids: Vec<u32> = (0..prog.structs.len() as u32).filter(|&s| prog.structs[s as usize].base.is_none() && prog.struct_item.get(s as usize).is_some_and(|&ii| user_sm(ii as usize))).collect();
    sids.sort_by_key(|&s| prog.struct_item[s as usize]);
    for sid in sids {
        let sd = &prog.structs[sid as usize];
        if prog.is_closure(sid) || prog.is_tuple(sid) {
            continue;
        }
        let ii = prog.struct_item[sid as usize] as usize;
        let (Some(it), Some(sm)) = (items.get(ii), sums.get(ii)) else { continue };
        let fields: Vec<&[(String, Ty, u32)]> = if sd.is_enum { sd.variants.iter().map(|v| v.fields.as_slice()).collect() } else { vec![sd.fields.as_slice()] };
        // (comments blanked: a trait's name in one is no bound)
        let text = blank_comments(src.get(it.start..it.end).unwrap_or(""));
        let bounds = |n: &str| sm.adt_bounds.iter().find(|(p, _)| p == n).map(|(_, b)| b.clone()).unwrap_or_default();
        for (vi, fs) in fields.iter().enumerate() {
            for (fi, (_, ft, _)) in fs.iter().enumerate() {
                let mut ns = vec![];
                needs(prog, sums, user, *ft, &mut ns, 0);
                if let Some(m) = failing(&ns, &sm.adt_generics, &bounds, &text) {
                    let (l, c) = field_pos(&text, sd.is_enum, vi, fi).unwrap_or((0, 0));
                    return Some(format!("{}:{}: {m}", it.line + l, c + 1));
                }
            }
        }
    }
    // (the self types of the program's impls)
    for (ii, sm) in sums.iter().enumerate().take(user.min(items.len())) {
        // (the impl's header once: an impl of members is split into its header and them)
        let Some(st) = &sm.impl_self_ty else { continue };
        if !matches!(items[ii].kind, ItemKind::Normal | ItemKind::ImplHeader) {
            continue;
        }
        let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
        let Ok(t) = prog.resolve_g(st, &gn) else { continue };
        let mut ns = vec![];
        needs(prog, sums, user, t, &mut ns, 0);
        // (the trait's arguments: the types written in them, and the bounds the trait puts on its parameters
        // (`trait Getter<T: Clone2>` with `impl Getter<isize> for isize` needs `isize: Clone2`), for a trait of the
        // program's)
        let tr = sm.impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
        let decl = tr.and_then(|tr| sums[..user.min(sums.len())].iter().find(|s| s.trait_decl.as_deref() == Some(tr)));
        let tparams: Vec<&(String, Vec<String>)> = decl.map(|d| d.trait_generics.iter().filter(|g| !g.0.starts_with('#') && g.0 != "Self").collect()).unwrap_or_default();
        for (k, a) in sm.impl_trait_args.iter().enumerate() {
            let Ok(a) = prog.resolve_g(a, &gn) else { continue };
            needs(prog, sums, user, a, &mut ns, 0);
            // (only when the arguments line up with the trait's parameters)
            if tparams.len() == sm.impl_trait_args.len() {
                ns.extend(tparams[k].1.iter().filter(|b| !b.starts_with(crate::types::CONST_GENERIC) && !b.contains('<')).map(|b| (a, b.clone())));
            }
        }
        if ns.is_empty() {
            continue;
        }
        let gens: Vec<String> = gn.iter().map(|s| s.to_string()).collect();
        let bounds = |n: &str| sm.impl_generics.iter().find(|g| g.0 == n).map(|g| g.1.clone()).unwrap_or_default();
        let text = blank_comments(src.get(items[ii].start..items[ii].end).unwrap_or(""));
        if let Some(m) = failing(&ns, &gens, &bounds, &text) {
            return Some(format!("{}:1: {m}", items[ii].line));
        }
    }
    // (the signatures of the program's free fns: `fn explode(x: Foo<u32>)` with `struct Foo<T: Trait>`)
    for (ii, sm) in sums.iter().enumerate().take(user.min(items.len())) {
        if sm.impl_self_ty.is_some() || sm.trait_decl.is_some() || !matches!(items[ii].kind, ItemKind::Normal) {
            continue;
        }
        for (_, sig) in &sm.fns {
            if sig.impl_of.is_some() || sig.trait_of.is_some() || sig.is_extern {
                continue;
            }
            let gn: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
            let mut ns = vec![];
            for t in sig.params.iter().chain([&sig.ret]) {
                if let Ok(t) = prog.resolve_g(t, &gn) {
                    needs(prog, sums, user, t, &mut ns, 0);
                }
            }
            if ns.is_empty() {
                continue;
            }
            let gens: Vec<String> = gn.iter().map(|s| s.to_string()).collect();
            let bounds = |n: &str| sig.generics.iter().find(|g| g.0 == n).map(|g| g.1.clone()).unwrap_or_default();
            let text = blank_comments(src.get(items[ii].start..items[ii].end).unwrap_or(""));
            // (the signature only: a bound written in the body is no bound of the fn's)
            let head = text.split('{').next().unwrap_or("").to_string();
            if let Some(m) = failing(&ns, &gens, &bounds, &head) {
                return Some(format!("{}:1: {m}", items[ii].line));
            }
        }
    }
    None
}

/// A derive of the program's whose field does not have the derived trait: `line:col: ..` of the first such field
/// (`#[derive(Hash)] struct S { x: Error }`, `#[derive(Eq)]` of an f64). Only what is sure: a field of a struct or enum
/// of the program's that neither derives nor implements the trait, and a float (inside an Option, an array, a tuple,
/// a Box, a Vec or a reference too) for Eq, Ord and Hash
fn derive_field_error(prog: &Program, src: &str, items: &[SrcItem]) -> Option<String> {
    fn float_in(p: &Program, t: Ty, depth: u32) -> Option<&'static str> {
        match t {
            Ty::F32 => Some("f32"),
            Ty::F64 => Some("f64"),
            Ty::Array(e, _) | Ty::Slice(e) | Ty::Box(e) | Ty::Ref(_, e) if depth < 8 => float_in(p, *e, depth + 1),
            Ty::Struct(s) if depth < 8 => {
                let sd = &p.structs[s as usize];
                let std_wrap = sd.base.is_some_and(|b| p.is_tuple(b) || matches!(p.structs[b as usize].name.as_str(), "Option" | "Vec"));
                sd.args.iter().filter(|_| std_wrap).find_map(|&a| float_in(p, a, depth + 1))
            }
            _ => None,
        }
    }
    let mut ds: Vec<(u32, &Vec<String>)> = prog.derived.iter().map(|(&sid, d)| (sid, d)).collect();
    ds.sort_by_key(|&(sid, _)| prog.struct_item.get(sid as usize).copied().unwrap_or(u32::MAX));
    for (sid, derives) in ds {
        let Some(&ii) = prog.struct_item.get(sid as usize) else { continue };
        let Some(it) = items.get(ii as usize) else { continue };
        let sd = &prog.structs[sid as usize];
        let fields: Vec<&[(String, Ty, u32)]> = if sd.is_enum { sd.variants.iter().map(|v| v.fields.as_slice()).collect() } else { vec![sd.fields.as_slice()] };
        let mut bad: Option<(usize, usize, String)> = None;
        'fields: for (vi, fs) in fields.iter().enumerate() {
            for (fi, (_, ft, _)) in fs.iter().enumerate() {
                if prog.has_param(*ft) {
                    continue;
                }
                for d in derives {
                    let d = d.as_str();
                    let float = if matches!(d, "Eq" | "Ord" | "Hash") { float_in(prog, *ft, 0) } else { None };
                    let own_missing = matches!(d, "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Clone" | "Debug") && matches!(*ft, Ty::Struct(f) if {
                        let fd = &prog.structs[f as usize];
                        fd.base.is_none() && fd.placeholder.is_none() && !prog.is_tuple(f) && prog.struct_item.get(f as usize).is_some_and(|&fi| (fi as usize) < items.len()) && !prog.implements(*ft, d)
                    });
                    if float.is_some() || own_missing {
                        // (the name as written: not the renamer's `__u_Error`)
                        let n = float.map_or_else(|| prog.tn(*ft).replace("__u_", ""), str::to_string);
                        let msg = match d {
                            "PartialEq" => format!("binary operation `==` cannot be applied to type `{n}` (E0369)"),
                            "PartialOrd" => format!("can't compare `{n}` with `{n}` (E0277)"),
                            "Debug" => format!("`{n}` doesn't implement `Debug` (E0277)"),
                            _ => format!("the trait bound `{n}: {d}` is not satisfied (E0277)"),
                        };
                        bad = Some((vi, fi, msg));
                        break 'fields;
                    }
                }
            }
        }
        let Some((vi, fi, msg)) = bad else { continue };
        let text = blank_comments(src.get(it.start..it.end).unwrap_or(""));
        let (l, c) = field_pos(&text, sd.is_enum, vi, fi).unwrap_or((0, 0));
        return Some(format!("{}:{}: {msg}", it.line + l, c + 1));
    }
    None
}

/// The line (from the item's first) and column of field fi of variant vi (of the struct's fields when not an enum)
/// in a struct's or enum's text, comments blanked
fn field_pos(text: &str, is_enum: bool, vi: usize, fi: usize) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let pos = |i: usize| {
        let l = text[..i].matches('\n').count();
        (l, i - text[..i].rfind('\n').map_or(0, |k| k + 1))
    };
    // (the body: past the item's attributes (`#[doc = "struct "]`), the name and its generics (`<T: Fn(u8) -> u8>`),
    // the first `{` or `(`)
    let kw = if is_enum { "enum " } else { "struct " };
    let mut from = text.len() - text.trim_start().len();
    while text[from..].starts_with("#[") {
        from += 2 + attr_end(&text[from + 2..]);
        from += text[from.min(text.len())..].len() - text[from.min(text.len())..].trim_start().len();
        from = from.min(text.len());
    }
    let mut i = from + text[from..].find(kw)? + kw.len();
    let mut angle = 0i32;
    while i < b.len() {
        match b[i] {
            b'<' => angle += 1,
            b'>' if i > 0 && b[i - 1] != b'-' => angle -= 1,
            b'{' | b'(' if angle == 0 => break,
            _ => {}
        }
        i += 1;
    }
    // (the items of a list from b[i] (an opening bracket) to its close: where each starts, past attributes)
    fn items_of(b: &[u8], open: usize) -> (Vec<usize>, usize) {
        let mut starts = vec![];
        let mut depth = 0i32;
        let mut j = open + 1;
        let mut at_start = true;
        while j < b.len() {
            let c = b[j];
            if at_start && !c.is_ascii_whitespace() && depth == 0 {
                if c == b'#' {
                    // (an attribute: to its `]`)
                    let mut d = 0;
                    while j < b.len() {
                        match b[j] {
                            b'[' => d += 1,
                            b']' => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    j += 1;
                    continue;
                }
                if !matches!(c, b')' | b'}' | b']') {
                    starts.push(j);
                }
                at_start = false;
            }
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b'<' => depth += 1,
                b'>' if b[j - 1] != b'-' && depth > 0 => depth -= 1,
                b')' | b']' | b'}' => {
                    if depth == 0 {
                        return (starts, j);
                    }
                    depth -= 1;
                }
                b',' if depth == 0 => at_start = true,
                _ => {}
            }
            j += 1;
        }
        (starts, j)
    }
    if !is_enum {
        let (starts, _) = items_of(b, i);
        return starts.get(fi).map(|&k| pos(k));
    }
    let (variants, _) = items_of(b, i);
    let v = *variants.get(vi)?;
    // (the variant's fields: its `(` or `{`)
    let mut k = v;
    while k < b.len() && !matches!(b[k], b'(' | b'{' | b',' | b'=' | b'}') {
        k += 1;
    }
    if k >= b.len() || !matches!(b[k], b'(' | b'{') {
        return Some(pos(v));
    }
    let (starts, _) = items_of(b, k);
    starts.get(fi).map(|&s| pos(s))
}

/// `#![deny(deprecated)]`: the first path of the program's through `std::intrinsics` / `core::intrinsics` to an
/// intrinsic that is stable through another module (`transmute`, `copy`, `copy_nonoverlapping`, `write_bytes`), in a
/// `use` (a group too) or an expression: `line:col: use of deprecated ..`
fn first_accidentally_stable(src: &str, items: &[SrcItem]) -> Option<String> {
    const STABLE: [&str; 4] = ["transmute", "copy_nonoverlapping", "copy", "write_bytes"];
    for it in items {
        let raw = src.get(it.start..it.end)?;
        if !raw.contains("intrinsics") {
            continue;
        }
        // (comments and string literals are no paths)
        let mut text = blank_comments(raw).into_bytes();
        let mut i = 0;
        while i < text.len() {
            match literal_end(&text, i).filter(|_| text[i] != b'\'') {
                Some(e) => {
                    for c in &mut text[i..e] {
                        if *c != b'\n' {
                            *c = b' ';
                        }
                    }
                    i = e;
                }
                None => i += 1,
            }
        }
        let Ok(text) = String::from_utf8(text) else { continue };
        // (an item's `#[allow(deprecated)]` allows its uses, under the crate's deny; read past comments and literals)
        if text.contains("allow(deprecated") || text.contains("expect(deprecated") {
            continue;
        }
        let b = text.as_bytes();
        for (k, _) in text.match_indices("intrinsics") {
            // (`std::intrinsics::` / `core::intrinsics::`, the module a segment of its own)
            let before = text[..k].trim_end();
            let Some(before) = before.strip_suffix("::") else { continue };
            let root = before.trim_end();
            if !(root.ends_with("std") || root.ends_with("core")) || root.len() > 4 && b[root.len() - if root.ends_with("std") { 4 } else { 5 }].is_ascii_alphanumeric() {
                continue;
            }
            let Some(after) = text[k + "intrinsics".len()..].trim_start().strip_prefix("::") else { continue };
            let after = after.trim_start();
            // (the names it brings: one, or a group's)
            let names: Vec<(usize, &str)> = if let Some(g) = after.strip_prefix('{') {
                let g_at = text.len() - g.len();
                let end = g.find('}').unwrap_or(g.len());
                g[..end].split(',').scan(g_at, |at, part| {
                    let here = *at + (part.len() - part.trim_start().len());
                    *at += part.len() + 1;
                    Some((here, part.trim()))
                }).collect()
            } else {
                // (at the path's first segment, as rustc)
                vec![(root.len() - if root.ends_with("std") { 3 } else { 4 }, after)]
            };
            for (at, name) in names {
                let id: String = name.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if STABLE.contains(&id.as_str()) {
                    let line = it.line + text[..at].matches('\n').count();
                    let col = at - text[..at].rfind('\n').map_or(0, |n| n + 1) + 1;
                    return Some(format!("{line}:{col}: use of deprecated import through accidentally stabilized module `intrinsics`: `{id}` (deprecated)"));
                }
            }
        }
    }
    None
}

/// The lint levels codegen checks (`#![deny(unused_must_use)]`, `trivial_casts` ..) as one byte for the cache keys: a
/// function compiled before such a `#![deny]` was added is not reused without its check
fn lint_bits(p: &Program) -> u16 {
    (p.deny_must_use as u8 | (p.deny_trivial_casts.0 as u8) << 1 | (p.deny_trivial_casts.1 as u8) << 2 | (p.deny_unreachable as u8) << 3 | (p.deny_unused_alloc as u8) << 4 | ((p.edition >= 2024) as u8) << 5
        // (and what a closure captures: `let _ = s;` captures s before 2021 only (ast_closure::WILD_FAKE_READS))
        | ((p.edition >= 2021) as u8) << 6
        | (p.allow_overflowing_literals as u8) << 7) as u16
        | (p.deny_unused_mut as u16) << 8
        | (p.allow_implicit_autorefs as u16) << 9
}

/// The attributes behind a feature gate, and the feature
const GATED: &[(&str, &str)] = &[
    ("default_lib_allocator", "allocator_internals"),
    ("export_stable", "export_stable"),
    ("ffi_const", "ffi_const"),
    ("ffi_pure", "ffi_pure"),
    ("rustc_align", "fn_align"),
    ("rustc_align_static", "static_align"),
    ("fundamental", "fundamental"),
    ("move_size_limit", "large_assignments"),
    ("marker", "marker_trait_attr"),
    ("no_core", "no_core"),
    ("optimize", "optimize_attribute"),
    ("patchable_function_entry", "patchable_function_entry"),
    ("register_tool", "register_tool"),
    ("thread_local", "thread_local"),
];

/// An attribute that needs a `#![feature(..)]` the source does not have: `line:col: the `x` attribute is an
/// experimental feature (E0658)`. Read from the tokens of src[from..to] (literals and comments are not attributes);
/// the features from the whole source's (a `#![feature]` may follow the attribute it enables)
fn gated_attr(src: &str, from: usize, to: usize, feats: &mut Option<Vec<String>>) -> Option<String> {
    let b = src.as_bytes();
    let toks = lexer::tokenize(&b[from..to]);
    let tx = |k: usize| &src[from + toks[k].1..from + toks[k].2];
    let sym = |k: usize, c: &str| k < toks.len() && toks[k].0 == 1 && tx(k) == c;
    for k in 0..toks.len() {
        if !sym(k, "#") {
            continue;
        }
        let mut j = k + 1;
        if sym(j, "!") {
            j += 1;
        }
        if !sym(j, "[") {
            continue;
        }
        j += 1;
        // (`#[unsafe(ffi_const)]`)
        if j + 1 < toks.len() && toks[j].0 == 0 && tx(j) == "unsafe" && sym(j + 1, "(") {
            j += 2;
        }
        if j >= toks.len() || toks[j].0 != 0 || sym(j + 1, ":") {
            continue;
        }
        let name = tx(j);
        let at = |k: usize| {
            let i = from + toks[k].1;
            let line = src[..i].matches('\n').count() + 1;
            (line, i - src[..i].rfind('\n').map_or(0, |n| n + 1) + 1)
        };
        // (`#[target_feature(enable = "apxf,avx2")]`: a feature of the target that is unstable)
        if name == "target_feature" {
            let mut q = j + 1;
            while q < toks.len() && !sym(q, "]") {
                if let Some(text) = (toks[q].0 == 2).then(|| str_lit_value(tx(q))).flatten() {
                    for tf in text.split(',').map(str::trim) {
                        let Some(gate) = unstable_target_feature(tf) else { continue };
                        let enabled = feats.get_or_insert_with(|| enabled_features(src));
                        if !enabled.iter().any(|f| f == gate) {
                            let (line, col) = at(k);
                            return Some(format!("{line}:{col}: the target feature `{tf}` is currently unstable (E0658)"));
                        }
                    }
                }
                q += 1;
            }
            continue;
        }
        // (the first-level names of its arguments: `doc(cfg(..))` / `repr(C, simd)`)
        let mut args: Vec<&str> = vec![];
        if sym(j + 1, "(") {
            let (mut q, mut d) = (j + 2, 1);
            while q < toks.len() && d > 0 {
                if sym(q, "(") || sym(q, "[") {
                    d += 1;
                } else if sym(q, ")") || sym(q, "]") {
                    d -= 1;
                } else if d == 1 && toks[q].0 == 0 {
                    args.push(tx(q));
                }
                q += 1;
            }
        }
        // (`#[repr(transparent)] union U`: rustc points at the union)
        if name == "repr" && args.contains(&"transparent") {
            // (this attribute's `]`)
            let (mut e, mut d) = (j + 1, 0i32);
            while e < toks.len() {
                if sym(e, "(") || sym(e, "[") {
                    d += 1;
                } else if sym(e, ")") {
                    d -= 1;
                } else if sym(e, "]") {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                }
                e += 1;
            }
            let start = if e < toks.len() { from + toks[e].2 } else { to };
            // (a window past the attributes after it, however long: doubled until the item's keyword is in it)
            let mut len = 512;
            let (rest, rt) = loop {
                let mut end = src.len().min(start + len);
                while !src.is_char_boundary(end) {
                    end -= 1;
                }
                let rest = &src[start..end];
                let rt = lexer::tokenize(rest.as_bytes());
                // (complete when an identifier other than `pub` follows the attributes, or at the source's end)
                let (rb, mut q, mut d) = (rest.as_bytes(), 0, 0i32);
                let mut done = end == src.len();
                while q < rt.len() && !done {
                    let c = (rt[q].0 == 1).then(|| rb[rt[q].1]);
                    match c {
                        Some(b'[' | b'(') => d += 1,
                        Some(b']' | b')') => d -= 1,
                        // (a whole identifier: one that ends where the window does may be cut)
                        _ if d == 0 && rt[q].0 == 0 && (rt[q].2 < rest.len() || end == src.len()) && &rest[rt[q].1..rt[q].2] != "pub" => done = true,
                        _ => {}
                    }
                    q += 1;
                }
                if done {
                    break (rest, rt);
                }
                len *= 2;
            };
            let rb = rest.as_bytes();
            let rw = |q: usize| if q < rt.len() && rt[q].0 == 0 { &rest[rt[q].1..rt[q].2] } else { "" };
            let rs = |q: usize, c: u8| q < rt.len() && rt[q].0 == 1 && rb[rt[q].1] == c;
            // (the group that opens at q: the index after its close)
            let skip_group = |q: usize| -> usize {
                let mut d = 0i32;
                for k in q..rt.len() {
                    if rs(k, b'(') || rs(k, b'[') {
                        d += 1;
                    } else if rs(k, b')') || rs(k, b']') {
                        d -= 1;
                        if d == 0 {
                            return k + 1;
                        }
                    }
                }
                rt.len()
            };
            let mut q = 0;
            loop {
                if rs(q, b'#') && rs(q + 1, b'[') {
                    q = skip_group(q + 1);
                } else if rw(q) == "pub" {
                    q += 1;
                    if rs(q, b'(') {
                        q = skip_group(q);
                    }
                } else {
                    break;
                }
            }
            if rw(q) == "union" && !feats.get_or_insert_with(|| enabled_features(src)).iter().any(|f| f == "transparent_unions") {
                let i = start + rt[q].1;
                let line = src[..i].matches('\n').count() + 1;
                return Some(format!("{line}:{}: transparent unions are unstable (E0658)", i - src[..i].rfind('\n').map_or(0, |n| n + 1) + 1));
            }
        }
        let Some((feat, msg)) = attr_gate(name, &args) else { continue };
        // (the source's enabled features: read once, at the first gated attribute)
        let enabled = feats.get_or_insert_with(|| enabled_features(src));
        if !enabled.iter().any(|f| f == feat) {
            let (line, col) = at(k);
            return Some(format!("{line}:{col}: {msg} (E0658)"));
        }
    }
    None
}

/// The feature gate of an attribute named `name` with the first-level argument names `args`, and rustc's message
/// when the source does not enable it (rustc 1.100)
fn attr_gate(name: &str, args: &[&str]) -> Option<(&'static str, String)> {
    if let Some(&(_, feat)) = GATED.iter().find(|g| g.0 == name) {
        return Some((feat, format!("the `{name}` attribute is an experimental feature")));
    }
    let experimental = |feat: &'static str| Some((feat, format!("the `{name}` attribute is an experimental feature")));
    match name {
        "allow_internal_unstable" => Some(("allow_internal_unstable", "the `allow_internal_unstable` attribute side-steps feature gating and stability checks".into())),
        "allow_internal_unsafe" => Some(("allow_internal_unsafe", "the `allow_internal_unsafe` attribute side-steps the `unsafe_code` lint".into())),
        "compiler_builtins" => Some(("compiler_builtins", "the `compiler_builtins` attribute is used to identify the `compiler_builtins` crate which contains compiler-rt intrinsics and will never be stable".into())),
        "profiler_runtime" => Some(("profiler_runtime", "the `profiler_runtime` attribute is used to identify the `profiler_builtins` crate which contains the profiler runtime and will never be stable".into())),
        "prelude_import" => Some(("prelude_import", "the `prelude_import` attribute is for use by rustc only".into())),
        "linkage" => Some(("linkage", "the `linkage` attribute is experimental and not portable across platforms".into())),
        "lang" => Some(("lang_items", "lang items are subject to change".into())),
        "needs_allocator" => experimental("allocator_internals"),
        "panic_runtime" => experimental("panic_runtime"),
        "needs_panic_runtime" => experimental("needs_panic_runtime"),
        "rustc_unroll" => experimental("loop_hints"),
        "rustc_splat" => experimental("splat"),
        // (stability attributes: the standard library's)
        "stable" | "unstable" | "rustc_const_stable" | "rustc_const_unstable" | "rustc_default_body_unstable" | "unstable_feature_bound" => {
            Some(("staged_api", "stability attributes may not be used outside of the standard library".into()))
        }
        "pattern_complexity_limit" => Some(("rustc_attrs", "use of an internal attribute".into())),
        n if n.starts_with("rustc_") => Some(("rustc_attrs", "use of an internal attribute".into())),
        "repr" if args.contains(&"simd") => Some(("repr_simd", "SIMD types are experimental and possibly buggy".into())),
        "doc" => {
            if args.iter().any(|a| matches!(*a, "cfg" | "auto_cfg")) {
                Some(("doc_cfg", "the `doc(cfg)` and `doc(auto_cfg)` attributes are experimental".into()))
            } else if args.contains(&"masked") {
                Some(("doc_masked", "the `doc(masked)` attribute is experimental".into()))
            } else if args.contains(&"notable_trait") {
                Some(("doc_notable_trait", "the `doc(notable_trait)` attribute is experimental".into()))
            } else if args.iter().any(|a| matches!(*a, "rust_logo" | "keyword" | "attribute" | "fake_variadic" | "search_unbox")) {
                Some(("rustdoc_internals", "this subset of the `doc` attribute is meant for internal use only".into()))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A const or a static of the program's whose value names an extern block's static: (its index, the static's name).
/// Read only for a program with an extern static
fn extern_static_in_value(prog: &crate::program::Program) -> Option<(usize, String)> {
    let ext: Vec<&str> = prog.consts.iter().filter(|c| c.expr.starts_with(crate::parse::EXTERN_STATIC_SYM)).map(|c| c.name.as_str()).collect();
    if ext.is_empty() {
        return None;
    }
    prog.consts.iter().enumerate().find_map(|(ci, c)| {
        if c.item >= prog.prelude_from || c.expr.starts_with(crate::parse::EXTERN_STATIC_SYM) {
            return None;
        }
        let b = c.expr.as_bytes();
        let toks = lexer::tokenize(b);
        let tx = |k: usize| &b[toks[k].1..toks[k].2];
        // (a read of it: not its address, `&X`, `&mut X`, `&raw const X`, `addr_of!(X)`, unless the address is read
        // again, `*&X`)
        let read = |k: usize| {
            let mut j = k;
            if j >= 1 && matches!(tx(j - 1), b"mut" | b"const") {
                j -= 1;
            }
            if j >= 1 && tx(j - 1) == b"raw" {
                j -= 1;
            }
            if j >= 1 && tx(j - 1) == b"&" {
                return j >= 2 && tx(j - 2) == b"*";
            }
            !(k >= 3 && tx(k - 1) == b"(" && tx(k - 2) == b"!" && matches!(tx(k - 3), b"addr_of" | b"addr_of_mut"))
        };
        let n = ext.iter().find(|n| (0..toks.len()).any(|k| toks[k].0 == 0 && tx(k) == n.as_bytes() && read(k)))?;
        Some((ci, n.to_string()))
    })
}

/// The ABIs rustc 1.100 knows, stable or behind a feature (`extern "rust-intrinsic"` is no longer one)
const KNOWN_ABIS: &[&str] = &[
    "Rust", "C", "C-unwind", "cdecl", "cdecl-unwind", "stdcall", "stdcall-unwind", "fastcall", "fastcall-unwind", "vectorcall", "vectorcall-unwind",
    "thiscall", "thiscall-unwind", "aapcs", "aapcs-unwind", "win64", "win64-unwind", "sysv64", "sysv64-unwind", "ptx-kernel", "msp430-interrupt",
    "x86-interrupt", "gpu-kernel", "efiapi", "avr-interrupt", "avr-non-blocking-interrupt", "riscv-interrupt-m", "riscv-interrupt-s",
    "cmse-nonsecure-call", "cmse-nonsecure-entry", "C-cmse-nonsecure-call", "C-cmse-nonsecure-entry", "system", "system-unwind", "rust-call",
    "unadjusted", "rust-cold", "rust-invalid", "custom", "tail", "rust-preserve-none", "llvm-intrinsic", "Swift", "interrupt",
];

/// An ABI behind a feature gate the source does not enable, `extern "rust-cold" fn f()` (rustc 1.100): `line:col:
/// the extern "x" ABI is experimental and subject to change (E0658)`, at the ABI's string
fn gated_abi(src: &str, from: usize, to: usize, feats: &mut Option<Vec<String>>) -> Option<String> {
    let toks = lexer::tokenize(&src.as_bytes()[from..to]);
    let tx = |k: usize| &src[from + toks[k].1..from + toks[k].2];
    // (the item's head, before its first `{`, `(` or `;`: an ABI there is the item's, not a fn pointer type's or a macro
    // call's input)
    let head = (0..toks.len()).find(|&k| toks[k].0 == 1 && matches!(tx(k), "{" | "(" | ";")).unwrap_or(toks.len());
    for k in 1..toks.len() {
        if toks[k].0 != 2 || toks[k - 1].0 != 0 || tx(k - 1) != "extern" {
            continue;
        }
        let Some(abi) = str_lit_value(tx(k)) else { continue };
        // (E0703: a name no ABI of rustc's has, `extern "fictional" fn`; a literal with a suffix, `"C"__`, is rustc's
        // error of the literal first)
        if k < head && !KNOWN_ABIS.contains(&abi.as_str()) {
            let i = from + toks[k].1;
            let line = src[..i].matches('\n').count() + 1;
            let col = i - src[..i].rfind('\n').map_or(0, |n| n + 1) + 1;
            if toks.get(k + 1).is_some_and(|n| n.0 == 0 && n.1 == toks[k].2) {
                return Some(format!("{line}:{col}: suffixes on string literals are invalid"));
            }
            return Some(format!("{line}:{col}: invalid ABI: found `{abi}` (E0703)"));
        }
        let (feat, how) = match abi.as_str() {
            "rust-cold" => ("rust_cold_cc", "is experimental and subject to change"),
            "tail" => ("rust_tail_cc", "is experimental and subject to change"),
            "rust-preserve-none" => ("rust_preserve_none_cc", "is experimental and subject to change"),
            "llvm-intrinsic" => ("link_llvm_intrinsics", "is an implementation detail and perma-unstable"),
            _ => continue,
        };
        if feats.get_or_insert_with(|| enabled_features(src)).iter().any(|f| f == feat) {
            continue;
        }
        let i = from + toks[k].1;
        let line = src[..i].matches('\n').count() + 1;
        let col = i - src[..i].rfind('\n').map_or(0, |n| n + 1) + 1;
        return Some(format!("{line}:{col}: the extern \"{abi}\" ABI {how} (E0658)"));
    }
    None
}

/// The parts of s between its top-level commas (not those inside `<..>` / `(..)` / `[..]`): a `where` clause's
/// predicates
fn top_level_split(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut from) = (vec![], 0i32, 0);
    let mut prev = '\0';
    for (i, c) in s.char_indices() {
        let arrow = c == '>' && prev == '-';
        prev = c;
        match c {
            // (the `>` of `-> R` closes nothing)
            '>' if arrow => {}
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[from..]);
    out
}

/// The value of a string literal token: `"x87"`, `r"x87"` / `r#"x87"#`, with the escapes of an ordinary one decoded
/// (`"x\x38\x37"`). None for another literal
fn str_lit_value(t: &str) -> Option<String> {
    if let Some(r) = t.strip_prefix('r') {
        let h = r.len() - r.trim_start_matches('#').len();
        return Some(r.get(h + 1..r.len().checked_sub(h + 1)?)?.to_string());
    }
    let inner = t.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut cs = inner.chars();
    while let Some(c) = cs.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match cs.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            'x' => out.push(u8::from_str_radix(&cs.by_ref().take(2).collect::<String>(), 16).ok()? as char),
            'u' => {
                // (`\u{3_8}`: underscores may separate the digits)
                let hex: String = cs.by_ref().skip(1).take_while(|&c| c != '}').filter(|&c| c != '_').collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            '\n' => {
                while cs.clone().next().is_some_and(char::is_whitespace) {
                    cs.next();
                }
            }
            c => out.push(c),
        }
    }
    Some(out)
}

/// The feature gate of an x86 target feature that is unstable (rustc 1.100): `#[target_feature(enable = "x87")]`
/// needs `#![feature(x87_target_feature)]`. Only those whose gate tests/ui checks; the others count as stable
fn unstable_target_feature(tf: &str) -> Option<&'static str> {
    if tf.starts_with("amx-") {
        return Some("x86_amx_intrinsics");
    }
    Some(match tf {
        "apxf" => "apx_target_feature",
        "avx10.1" | "avx10.2" => "avx10_target_feature",
        "clflushopt" => "clflushopt_target_feature",
        "fma4" => "fma4_target_feature",
        "movrs" => "movrs_target_feature",
        "x87" => "x87_target_feature",
        "xop" => "xop_target_feature",
        _ => return None,
    })
}

/// The attributes `#[..]` / `#![..]` at the start of b, with the space and comments between them: Some(their length)
/// when one is behind a feature gate, None when none is. A raw string or a character literal inside one gives up and
/// reads all of b (the tokens then tell what they are)
fn gated_attrs_len(b: &[u8]) -> Option<usize> {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut i = 0;
    let mut gated = false;
    loop {
        i = skip_space(b, i);
        if i >= b.len() || b[i] != b'#' {
            // (an extern block after its outer attributes: its items' attributes are read too, the whole item)
            if extern_block(b, i) {
                return Some(b.len());
            }
            return gated.then_some(i);
        }
        i += 1;
        let mut depth = 0;
        while i < b.len() {
            match b[i] {
                b'[' if depth == 0 => {
                    depth = 1;
                    // (the attribute's name: `#[name`, `#[unsafe(name`; not a path `#[tool::name`)
                    let mut j = i + 1;
                    let skip = |j: &mut usize| *j = skip_space(b, *j);
                    skip(&mut j);
                    if b[j..].starts_with(b"unsafe") && !b.get(j + 6).is_some_and(|&c| ident(c)) {
                        let mut k = j + 6;
                        skip(&mut k);
                        if b.get(k) == Some(&b'(') {
                            depth = 2;
                            j = k + 1;
                            skip(&mut j);
                        }
                    }
                    let n0 = j;
                    while j < b.len() && ident(b[j]) {
                        j += 1;
                    }
                    let name = &b[n0..j];
                    skip(&mut j);
                    // (a name some gate is on: `attr_gate`'s, or one whose arguments decide, `doc(..)` / `repr(..)`)
                    let gate_named = std::str::from_utf8(name).is_ok_and(|n| attr_gate(n, &[]).is_some() || matches!(n, "doc" | "repr" | "target_feature"));
                    if !name.is_empty() && b.get(j) != Some(&b':') && gate_named {
                        gated = true;
                    }
                    i = j;
                    continue;
                }
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' => {
                    depth -= 1;
                    if depth <= 0 {
                        i += 1;
                        break;
                    }
                }
                b'"' => {
                    i += 1;
                    while i < b.len() && b[i] != b'"' {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                }
                b'\'' => return Some(b.len()),
                b'r' if matches!(b.get(i + 1), Some(b'#' | b'"')) => return Some(b.len()),
                _ => {}
            }
            i += 1;
        }
        i = i.min(b.len());
    }
}

/// An extern block starts at b[i..]: `extern "C" {` / `unsafe extern {`, not an `extern "C" fn` (whose body's macro
/// arguments are no attributes)
fn extern_block(b: &[u8], i: usize) -> bool {
    // (most items start with neither `extern` nor `unsafe`)
    if !matches!(b.get(i), Some(b'e' | b'u')) {
        return false;
    }
    let mut j = i;
    if b[j..].starts_with(b"unsafe") {
        j = skip_space(b, j + 6);
    }
    if !b[j..].starts_with(b"extern") {
        return false;
    }
    j = skip_space(b, j + 6);
    // (the ABI: `"C"`, or a raw string `r"C"` / `r#"C"#`)
    let mut hashes = 0;
    if b.get(j) == Some(&b'r') {
        j += 1;
        while b.get(j) == Some(&b'#') {
            hashes += 1;
            j += 1;
        }
    }
    if b.get(j) == Some(&b'"') {
        j += 1;
        while j < b.len() && !(b[j] == b'"' && b[j + 1..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes) {
            j += 1;
        }
        j = skip_space(b, j + 1 + hashes);
    }
    b.get(j) == Some(&b'{')
}

/// The index of the first byte at or after i that is not whitespace or a comment (`//..`, nested `/* .. */`)
fn skip_space(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i..).is_some_and(|r| r.starts_with(b"//")) {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b.get(i..).is_some_and(|r| r.starts_with(b"/*")) {
            let mut depth = 0;
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else {
            return i.min(b.len());
        }
    }
}

/// The features `#![feature(..)]` enables, read from the source's tokens (not a comment's or a string's text)
fn enabled_features(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let toks = lexer::tokenize(b);
    let tx = |k: usize| &src[toks[k].1..toks[k].2];
    let sym = |k: usize, c: &str| k < toks.len() && toks[k].0 == 1 && tx(k) == c;
    let mut out = vec![];
    for k in 0..toks.len() {
        if !(sym(k, "#") && sym(k + 1, "!") && sym(k + 2, "[") && k + 3 < toks.len() && tx(k + 3) == "feature" && sym(k + 4, "(")) {
            continue;
        }
        let mut j = k + 5;
        while j < toks.len() && !sym(j, ")") {
            if toks[j].0 == 0 {
                out.push(tx(j).to_string());
            }
            j += 1;
        }
    }
    out
}
