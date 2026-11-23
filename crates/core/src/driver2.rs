//! syn と Cranelift を使わないドライバ: ソース → (lexer / macros / prelude) → ast_summary → Program →
//! acg (自前の構文木のコード生成) → minclif の ELF の .o → リンク。
//! driver.rs と同じ手順を、外部クレートに依存しないモジュールだけで行う (zustc 自身でもコンパイルできる)。
//! キャッシュと並列化はまだない。

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
    /// 宣言の指紋: fn アイテムは要約 (シグネチャ・クロージャ)、ほかのアイテムはテキストのハッシュを並べたもの。
    /// fn の本体だけを変えた編集では変わらない (check の記憶 ChkMemo を使ってよいかの印)
    pub iface_fp: u64,
    pub src: String,
    pub items: Vec<SrcItem>,
    pub prog: Program,
    pub refs: Vec<Vec<u32>>,
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
    Session2::new(Options2 { opt, no_cache: true, jobs: 1, ..Default::default() }).analyze(src0)
}

/// 機械語にするバックエンド (Cranelift: zust-clif の compile)。feature `build` なしでは使わない
#[cfg(feature = "build")]
pub type Backend = crate::lower::ClBackend;
#[cfg(not(feature = "build"))]
pub type Backend = fn();

/// ドライバの設定
#[derive(Clone, Debug)]
pub struct Options2 {
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
}

/// Writes the output from the functions and data build.rs lays out (in place of the ELF object)
pub type ModuleWriter = fn(&[(String, Vec<u8>, Vec<crate::ir::Reloc>, bool)], &[(String, Vec<u8>, Vec<(u32, String, i64)>, bool)]) -> Result<Vec<u8>, String>;

impl Default for Options2 {
    fn default() -> Self {
        Options2 { opt: false, jobs: cpu_count(), linker: None, lib_dirs: vec![], no_cache: false, time: false, backend: None, features: vec![], module_writer: None, edition: 2021 }
    }
}

impl Options2 {
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
            cfgs: cfgs.to_vec(),
            env: env.to_vec(),
        }
    }
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
    /// このクレートの feature (ワークスペース。None なら Options2 の features)
    pub features: Option<Vec<String>>,
    /// このクレートの cfg と `env!` の環境 (ワークスペース: CrateSpec の cfgs / env)
    pub cfgs: Vec<(String, Option<String>)>,
    pub env: Vec<(String, String)>,
    /// 依存するクレートのテキストの範囲 (結合したソースの行 [開始, 終わり)) と、そこから見えるクレート (この表の番号。自分も)
    pub dep_ranges: Vec<(u32, u32, Vec<usize>, u32)>,
}

/// ビルドの結果
#[derive(Clone, Debug, Default)]
pub struct Report2 {
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
    h.add("driver2-1");
    if r == 0 {
        // (x86-64: st_size は 48、st_mtim は 88 / 96)
        h.add(&buf[48..56]);
        h.add(&buf[88..104]);
    }
    h.finish()
}

/// 時間の計測 (-time)

/// キャッシュを持つドライバ (driver.rs の Session と同じ使い方)
pub struct Session2 {
    pub opts: Options2,
    cid: Key,
    disk: crate::cache2::Cache2,
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
    /// In a `check`: its analysis adds const_check_item
    checking: bool,
    /// The lines of the constants that the check's const_check_item reads, in its order
    const_lines: Vec<usize>,
    /// their fns' names' prefix (CONST_CHECK_FN, or another when the program writes that)
    const_fn: String,
}

impl Session2 {
    pub fn new(opts: Options2) -> Session2 {
        Session2 { opts, cid: compiler_id(), disk: crate::cache2::Cache2::empty(), mem: FxMap::default(), used: FxSet::default(), fresh: vec![], ctx: CrateCtx::default(), last_map: None, sel: Default::default(), sum_fp: FxMap::default(), chk: None, externs: vec![], checking: false, const_lines: vec![], const_fn: String::new() }
    }

    pub fn load_cache(&mut self, path: &std::path::Path) {
        if !self.opts.no_cache {
            self.disk = crate::cache2::Cache2::open(path);
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
        self.disk = crate::cache2::Cache2::empty();
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
        let n = crate::cache2::Cache2::save(path, &refs)?;
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
        let cfg = self.opts.cfg_with(self.ctx.features.as_ref().unwrap_or(&self.opts.features));
        let renamed = crate::prep::std_renames(src0);
        crate::prep::moved_lines(renamed.as_deref().unwrap_or(src0), &crates, Some(&cfg))
    }

    /// 要約はアイテムのハッシュで引く
    pub fn analyze(&mut self, src0: &str) -> R<Analysis> {
        // (closures' captures by edition: `let _ = s;` captures s before 2021 only (ast_closure::WILD_FAKE_READS))
        crate::ast_closure::WILD_FAKE_READS.with(|c| c.set(self.opts.edition >= 2021));
        let mut externs = std::mem::take(&mut self.externs);
        // (ファイルから読んだもの (load_crate) は読むときに cfg を評価してある)
        let stripped;
        let src0 = if self.ctx.map.inputs.is_empty() {
            let cfg = self.opts.cfg_with(&self.opts.features);
            let mut ext = vec![];
            match crate::cfg::strip_noting(src0, &cfg, &mut None, &mut ext) {
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
        let mut fix = vec![];
        let cfg = self.opts.cfg_of(self.ctx.features.as_ref().unwrap_or(&self.opts.features), &self.ctx.cfgs, &self.ctx.env);
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
        // (E0432: a top-level `use` of a path whose first segment names nothing; the `use` items come from the item
        // split, the names they could mean from the original text)
        if self.ctx.map.inputs.is_empty() {
            if let Some(e) = unresolved_import(src0, &src, &items[..user_src], &self.ctx.deps) {
                return Err(e);
            }
            if let Some(e) = duplicate_import(src0, &src, &items[..user_src]) {
                return Err(e);
            }
            // (E0428 of an inline module and an item: a whole-text pass, so not on each build of a crate loaded from
            // files, whose incremental builds would pay it every time)
            if let Some(e) = duplicate_name(src0, &externs, self.opts.edition < 2018) {
                return Err(e);
            }
            if let Some(e) = malformed_attr(src0, &src, &items[..user_src], self.ctx.deps.is_empty() && self.ctx.lib.is_none()) {
                return Err(e);
            }
        }
        let mut tm = Timer { on: std::env::var_os("ZUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
        if let Some(p) = std::env::var_os("ZUSTC_DUMP_SRC") {
            let _ = std::fs::write(p, &src);
        }
        let cid = self.cid;
        let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
        let mut n_parsed = 0;
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
                if let Some(sm) = self.get(k).and_then(|d| Summary::decode(&d)) {
                    sums[i] = sm;
                    continue;
                }
                miss.push((i, k));
            }
            // (多ければ fork して分けて作る: 子 k は j % jobs == k のものを作り、直列化して返す)
            let jobs = self.opts.jobs.min(miss.len() / 32).max(1);
            let mut got: Vec<Option<Summary>> = vec![None; miss.len()];
            if jobs > 1 {
                let mut kids: Vec<(i32, i32)> = vec![];
                for kk in 0..jobs {
                    let mut fds = [0i32; 2];
                    // SAFETY: fds は 2 つの int
                    if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                        continue;
                    }
                    let pid = unsafe { c_fork() };
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
                                crate::cache2::put_uv(&mut buf, j as u64);
                                crate::cache2::put_uv(&mut buf, e.len() as u64);
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
                    let mut r = crate::cache2::Rd { d: &d, pos: 0 };
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
                        crate::ast_summary::parse_and_summarize(&src, it, ow.as_ref()).map_err(|e| match (e.strip_prefix('\u{2}'), it.kind) {
                            // (`\u{2}msg`: an impl member's error that is at its impl's header)
                            (Some(m), ItemKind::ImplMember(h)) => format!("{}:1: {m}", items[h as usize].line),
                            _ => relocate(e.trim_start_matches('\u{2}').to_string(), it),
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
                let own = items[..user_src].iter().filter(|it| ol == 0 || it.line <= ol).count();
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
        // (what the program's types require of traits' `Self: 'x` bounds (E0478, E0310): the traits are known now)
        {
            let so: crate::fxhash::FxMap<&str, &[String]> = sums
                .iter()
                .filter_map(|sm| Some((sm.trait_decl.as_deref()?, sm.trait_generics.iter().find(|g| g.0 == crate::types::SELF_OUTLIVES)?.1.as_slice())))
                .collect();
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
        // (struct / enum / union definitions of the program: unused parameters, union fields ..)
        for (it, sm) in items[..user_src].iter().zip(&sums[..user_src]) {
            if let Some(e) = &sm.adt_err {
                // (`\u{3}line\u{3}msg`: at that line of the item)
                if let Some((l, m)) = e.strip_prefix('\u{3}').and_then(|r| r.split_once('\u{3}')) {
                    return Err(format!("{}:1: {m}", it.line + l.parse::<usize>().unwrap_or(1) - 1));
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
                    if !word(src.get(d.start..d.end).unwrap_or(""), "where") {
                        return Err(format!("{}:1: impl has stricter requirements than trait (E0276)", it.line));
                    }
                }
            }
        }
        // (`#![crate_type = "lib"]`: a library, no `main` needed)
        let mut deny_unreachable = false;
        let mut deny_unused_vars = false;
        let mut deny_dead_code = false;
        let mut deny_dead_pub = false;
        let mut deny_unused_imports = false;
        let mut deny_unreachable_code = false;
        let mut deny_must_use = false;
        let mut deny_unused_alloc = false;
        let mut deny_unused_mut = false;
        let mut deny_trivial_casts = (false, false);
        let mut deny_unused_lifetimes = (false, false);
        let mut deny_mismatched_lts = false;
        let mut deny_deprecated = false;
        // (non_snake_case, non_camel_case_types, non_upper_case_globals)
        let mut deny_style = [false; 3];
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
            deny_unused_vars = levels.iter().any(|l| matches!(*l, "unused_variables" | "unused" | "warnings")) && !c.contains("#![allow(unused");
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
            deny_trivial_casts = (lint("trivial_casts"), lint("trivial_numeric_casts"));
            deny_unused_lifetimes = (lint("unused_lifetimes"), lint("single_use_lifetimes"));
            deny_mismatched_lts = lint("mismatched_lifetime_syntaxes");
            deny_deprecated = lint("deprecated") || (levels.contains(&"warnings") && !c.contains("#![allow(deprecated"));
            deny_unused_alloc = lint("unused_allocation")
                || (!levels.contains(&"unused_allocation") && levels.iter().any(|l| matches!(*l, "unused" | "warnings")) && !c.contains("#![allow(unused"));
            deny_unused_mut = lint("unused_mut") || (!levels.contains(&"unused_mut") && levels.iter().any(|l| matches!(*l, "unused" | "warnings")) && !c.contains("#![allow(unused"));
            for (k, n) in ["non_snake_case", "non_camel_case_types", "non_upper_case_globals"].iter().enumerate() {
                deny_style[k] = levels.iter().any(|l| l == n || *l == "nonstandard_style" || *l == "warnings") && !c.contains(&format!("#![allow({n}"));
            }
            ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"].iter().any(|k| c.contains(&format!("#![crate_type=\"{k}\"]")))
        };
        // (dead_code, in a binary: the first fn / struct / enum / const / static / trait of the program's whose name
        // appears nowhere but in its own definition and in `impl` headers)
        if deny_dead_code && self.ctx.lib.is_none() {
            // (a `pub` item counts as used, except in a binary under `deny(dead_code_pub_in_binary)`)
            if let Some((line, msg)) = first_dead_item(&src, &items[..user_src], deny_dead_pub && !lib_attr) {
                return Err(format!("{line}:1: {msg}"));
            }
        }
        // (unused_imports: a top-level `use` whose name nothing else mentions)
        if deny_unused_imports && self.ctx.lib.is_none() {
            if let Some((line, col, msg)) = first_unused_import(&src, &items[..user_src]) {
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
            for it in &items[..user_src] {
                let text = src.get(it.start..it.end).unwrap_or("");
                if !text.contains("fn ") || text.contains("allow(unreachable") || text.contains("#[cfg") {
                    continue;
                }
                let Ok(Some(f)) = parse_fn(&src, it) else { continue };
                if let Some((p, what)) = first_unreachable(&f.block) {
                    if p.line > 0 {
                        return Err(format!("{}:{}: unreachable {what}", it.line + p.line as usize - 1, p.col + 1));
                    }
                }
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
        // (unused_variables: the first binding nothing reads, in the program's own fns)
        if deny_unused_vars {
            for it in &items[..user_src] {
                let text = src.get(it.start..it.end).unwrap_or("");
                if !text.contains("fn ") || text.contains("allow(unused") || text.contains("#[cfg") {
                    continue;
                }
                let Ok(Some(f)) = parse_fn(&src, it) else { continue };
                let params: Vec<String> = f.sig.params.iter().filter_map(|(p, _)| match p {
                    Pat::Ident(_, _, n, None) => Some(n.clone()),
                    _ => None,
                }).collect();
                if let Some((pos, n)) = crate::ast_closure::first_unused_binding(&params, &f.block) {
                    let (line, col) = match pos {
                        Some(p) if p.line > 0 => (it.line + p.line as usize - 1, p.col as usize + 1),
                        _ => (it.line, 1),
                    };
                    return Err(format!("{line}:{col}: unused variable: `{n}`"));
                }
            }
        }
        unsafe_impls(&src, &items[..user_src])?;
        let mut prog = Program::build_with(&items, &sums, self.ctx.lib.is_none() && !lib_attr, user).map_err(|e| name_line(&e, &src, &items))?;
        tm.lap("program", String::new());
        prog.prelude_from = user as u32;
        if let Some(e) = cross_kind_duplicate(&prog, &src, &items, &sums, user) {
            return Err(e);
        }
        // (in a check (zustc check, the LSP): a build stays as fast as it was, and reports it when it makes the vtable)
        if let Some(e) = self.checking.then(|| dyn_in_signature(&prog, &src, &items)).flatten() {
            return Err(e);
        }
        // (E0277: a static is reachable from every thread, so its type must be Sync)
        if let Some((ci, tn)) = crate::acg::static_not_sync(&prog) {
            let line = items.get(prog.consts[ci].item as usize).map_or(0, |it| it.line);
            return Err(format!("{line}:1: `{tn}` cannot be shared between threads safely: the type of a `static` must be Sync (E0277)"));
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
            prog.trait_scopes = lexer::trait_scopes_with(src0, has_mod_word);
            if !prog.trait_scopes.is_empty() {
                prog.item_lines = items.iter().map(|it| it.line as u32).collect();
            }
        }
        prog.overflow_checks = !self.opts.opt;
        prog.deny_unreachable = deny_unreachable;
        // (the program has modules: read from its source as written, before flattening blanks their headers)
        prog.has_mods = has_mod_word;
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
            ["allow(", "warn(", "expect("].iter().any(|a| head.trim_start().starts_with(a)) && !in_text(i)
        });
        prog.deny_trivial_casts = deny_trivial_casts;
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
                let end = sb[b..it.end.min(sb.len())].iter().position(|&c| c == b';').map_or(it.end, |k| b + k);
                // (its identifier tokens: not the words of a comment in it)
                prog.use_names.extend(it.idents.iter().map(|&(x, y)| (x as usize, y as usize)).filter(|&(x, y)| x > b && y <= end).filter_map(|(x, y)| src.get(x..y)).map(str::to_string));
            }
        }
        // (a field or an impl's self type that names a struct of the program's whose parameter's bound does not hold:
        // `struct Hoge<K> { s: S<K> }` with `struct S<T: Tr>`)
        // (only when one of them has a bounded parameter)
        if sums[..user.min(sums.len())].iter().any(|s| !s.adt_bounds.is_empty()) {
            if let Some(e) = wf_error(&prog, &sums, &src, &items, user) {
                return Err(e);
            }
        }
        // (a derive whose field has not the trait: `#[derive(Hash)] struct S { x: Error }`)
        if !prog.derived.is_empty() {
            if let Some(e) = derive_field_error(&prog, &src, &items[..user]) {
                return Err(e);
            }
        }
        let ol = self.ctx.own_lines as usize;
        let ext_items: Vec<bool> = items.iter().enumerate().map(|(i, it)| ol > 0 && i < user && it.line > ol).collect();
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
        for ii in &prog.item_of {
            let (f, st, c) = ii.map(|ii| prog.refs_in(&src, &items[ii as usize], Some(&names))).unwrap_or_default();
            refs.push(f);
            srefs.push(st);
            crefs.push(c);
        }
        tm.lap("refs", String::new());
        let mut a = Analysis { iface_fp, src, items, prog, refs, srefs, crefs, n_parsed, ext_items, inl: acg::Inline::default(), cl: self.opts.backend, prelude_line0, dep_ranges: self.ctx.dep_ranges.clone(), asts: Mutex::new(FxMap::default()), encl: Mutex::new(FxMap::default()), sig_keys: Mutex::new(Vec::new()) };
        if self.opts.opt && std::env::var_os("ZUSTC_NO_INLINE").is_none() {
            a.inl = inline_bodies(&a);
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

/// アイテムの中の `行:桁:` のエラーを、ファイルの中の行にする (driver.rs の relocate_err と同じ)。位置がなければアイテムの先頭の行
fn relocate(e: String, it: &SrcItem) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    format!("{}:1: {e}", it.line)
}

/// アイテムの関数を自前の構文木で読む (本体がなければ None)。引数のパターンは本体の先頭の let にする
/// `#![deny(unused_imports)]`: the first name a top-level `use` brings in that no other item mentions. Traits
/// (std's usual ones and the program's) are left alone: their methods use them without naming them
fn first_unused_import(src: &str, items: &[SrcItem]) -> Option<(usize, usize, String)> {
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
        let t = text.trim_start();
        if !t.starts_with("use ") || text.contains("allow(unused") || text.contains("#[cfg") || text.contains('*') {
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

/// `#![deny(unreachable_code)]`: the first statement of a block after one that always diverges (`return`,
/// `break`, `continue`, `panic!` and its kin, a `loop` with no `break`), and whether it is a statement or the
/// block's value
fn first_unreachable(b: &crate::parse::Block) -> Option<(crate::parse::Pos, &'static str)> {
    use crate::parse::{Expr, Stmt};
    fn diverges(e: &Expr) -> bool {
        match e {
            Expr::Return(_) | Expr::Break(..) | Expr::Continue(..) => true,
            Expr::Macro(p, _, _) => matches!(p.segs.last().map(|s| s.name.as_str()), Some("panic" | "unreachable" | "todo" | "unimplemented")),
            Expr::Loop(_, body) => !format!("{body:?}").contains("Break("),
            Expr::Paren(x) => diverges(x),
            _ => false,
        }
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
        match e {
            Expr::Block(_, _, b) | Expr::Loop(_, b) => block(b),
            Expr::If(_, t, els) => block(t).or_else(|| els.as_deref().and_then(inner)),
            Expr::While(_, _, b) | Expr::For(_, _, _, b) => block(b),
            _ => None,
        }
    }
    block(b)
}

/// "malformed `X` attribute input" (E0539 / E0565): a built-in attribute of a fixed shape written in another one.
/// Read are the file's inner attributes (its head) and the attributes in front of the items (no full-text scan)
fn malformed_attr(src0: &str, split: &str, items: &[SrcItem], standalone: bool) -> Option<String> {
    // (a crate with no dependency (so no proc macro) knows only the built-in attributes and its tools' paths)
    // (both looked up only when such an attribute is there: not a search of every file)
    let rustc_attrs = || src0.contains("rustc_attrs");
    let unknown = |name: &str| {
        const BUILTIN: &[&str] = &[
            "cfg", "cfg_attr", "test", "ignore", "should_panic", "bench", "derive", "automatically_derived", "macro_export", "macro_use",
            "proc_macro", "proc_macro_derive", "proc_macro_attribute", "allow", "warn", "deny", "forbid", "expect", "deprecated", "must_use",
            "diagnostic", "link", "link_name", "link_ordinal", "no_link", "repr", "export_name", "link_section", "no_mangle", "used", "crate_name",
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
        ];
        if name.contains("::") || name.is_empty() || (name.starts_with("rustc_") && rustc_attrs()) {
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
        let str_lit = |v: &str| v.starts_with('"') && v.ends_with('"') && v.len() >= 2;
        let idents = |v: &str| v.split(',').all(|x| {
            let x = x.trim();
            !x.is_empty() && x.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        match name {
            "crate_type" | "recursion_limit" | "type_length_limit" | "crate_name" => form == '=' && str_lit(inner),
            "inline" => form == ' ' || (form == '(' && matches!(inner, "always" | "never")),
            "must_use" => form == ' ' || (form == '=' && str_lit(inner)),
            "cold" | "track_caller" | "non_exhaustive" | "no_mangle" | "automatically_derived" | "no_implicit_prelude" => form == ' ',
            "repr" => form == '(' && !inner.is_empty() && !inner.contains('='),
            "deprecated" => form == ' ' || (form == '=' && str_lit(inner)) || form == '(',
            "link" => form == '(' && !inner.is_empty() && !inner.contains("cfg()"),
            "allow" | "warn" | "deny" | "forbid" | "expect" => form == '(',
            "macro_use" => form == ' ' || (form == '(' && idents(inner)),
            "cfg" => form == '(' && !inner.is_empty(),
            "cfg_attr" => form == '(' && inner.contains(','),
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
            let body = &r[open..e];
            let n_end = body.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(body.len());
            out.push((i + ws, &body[..n_end], &body[n_end..]));
            i += ws + e + 1;
        }
        out
    }
    let report = |text: &str, base_line: usize, at: usize, name: &str| {
        let line = base_line + text[..at].matches('\n').count();
        format!("{line}:1: malformed `{name}` attribute input")
    };
    // (the file's inner attributes)
    let mut head_end = src0.len().min(8192);
    while !src0.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let head = &src0[..head_end];
    // (comments between them: line by line, skipping `//` lines)
    let mut off = 0;
    for line in head.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("#![") {
            for (at, name, input) in leading(line) {
                if !check(name, input) {
                    return Some(report(head, 1, off + at, name));
                }
            }
        } else if !(t.is_empty() || t.starts_with("//")) && !t.starts_with("#[") {
            break;
        }
        off += line.len();
    }
    for it in items {
        let text = &split[it.start..it.end];
        if !text.trim_start().starts_with("#[") {
            continue;
        }
        let attrs = leading(text);
        for &(at, name, input) in &attrs {
            if !check(name, input) {
                return Some(report(text, it.line, at, name));
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
        let Some(after) = after else { continue };
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
            _ => continue,
        };
        for &(at, name, _) in &attrs {
            let ok = match name {
                "inline" | "cold" | "track_caller" => kw == "fn",
                "repr" => matches!(kw, "struct" | "enum" | "union"),
                "non_exhaustive" => matches!(kw, "struct" | "enum"),
                "must_use" => matches!(kw, "fn" | "struct" | "enum" | "union" | "trait"),
                "macro_use" => matches!(kw, "mod"),
                "no_mangle" => matches!(kw, "fn" | "static"),
                _ => true,
            };
            if !ok {
                let line = it.line + text[..at].matches('\n').count();
                return Some(format!("{line}:1: `#[{name}]` attribute cannot be used on {kind}"));
            }
        }
    }
    None
}

/// E0038: a fn of the program whose parameters or result name `dyn Tr` of a trait that is not dyn compatible (rustc
/// checks the types a signature writes, called or not): at the fn's line
fn dyn_in_signature(prog: &Program, src: &str, items: &[SrcItem]) -> Option<String> {
    // (only a program that writes `dyn`: one look at its own text)
    let user_end = items.get((prog.prelude_from as usize).wrapping_sub(1)).map_or(0, |it| it.end);
    if !src[..user_end.min(src.len())].contains("dyn") {
        return None;
    }
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
fn duplicate_name(src: &str, externs: &[(usize, String)], injected: bool) -> Option<String> {
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
    let toks = crate::lexer::tokenize(b);
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
    if let Some((line, f)) = firsts.into_iter().find(|(_, f)| !defined.contains(f)) {
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
fn first_dead_item(src: &str, items: &[SrcItem], check_pub: bool) -> Option<(usize, String)> {
    let (Some(first), Some(last)) = (items.first(), items.last()) else { return None };
    let region = &src[first.start..last.end];
    let b = region.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let word = |k: usize| -> &str { if toks[k].0 == 0 { &region[toks[k].1..toks[k].2] } else { "" } };
    let sym = |k: usize, c: u8| toks[k].0 == 1 && b[toks[k].1] == c;
    // (uses of each name: every occurrence but the definition's, the one after `impl` / `for` in an impl header)
    let mut uses: crate::fxhash::FxMap<&str, usize> = Default::default();
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
        let defn = k > 0 && matches!(word(k - 1), "fn" | "struct" | "enum" | "const" | "static" | "trait" | "union" | "type");
        let impl_self = in_impl_header && k > 0 && (word(k - 1) == "impl" || word(k - 1) == "for" || sym(k - 1, b'>'));
        if !defn && !impl_self {
            *uses.entry(w).or_default() += 1;
        }
    }
    for it in items {
        // (top-level items only: impl and trait members are a type's, not free items)
        if !matches!(it.kind, crate::lexer::ItemKind::Normal) {
            continue;
        }
        let text = &src[it.start..it.end];
        if text.contains("allow(dead_code") || text.contains("expect(dead_code") || text.contains("allow(unused") || text.contains("no_mangle") || text.contains("#[test]")
            || text.contains("#[derive") || text.contains("export_name") || text.contains("#[cfg")
        {
            continue;
        }
        let t = crate::lexer::tokenize(text.as_bytes());
        let tw = |k: usize| -> &str { if t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" } };
        // (the item's keyword and name, after its attributes and visibility)
        let mut k = 0;
        while k < t.len() && (tw(k) == "pub" || tw(k) == "unsafe" || tw(k) == "async" || tw(k) == "extern" || t[k].0 != 0) {
            if tw(k) == "extern" || tw(k) == "impl" || tw(k) == "mod" || tw(k) == "use" || tw(k) == "macro_rules" {
                break;
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
        if name.is_empty() || name.starts_with('_') || name == "main" {
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
            _ => continue,
        };
        if uses.get(name).copied().unwrap_or(0) == 0 {
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
    let f = crate::parse::parse_file(text).map_err(|e| relocate(e, it))?;
    let Some(item) = f.items.into_iter().next() else { return Ok(None) };
    let crate::parse::ItemKind::Fn(mut sig, Some(mut block)) = item.kind else { return Ok(None) };
    let mut lets = vec![];
    for (k, (p, _)) in sig.params.iter_mut().enumerate() {
        if matches!(p, Pat::Ident(..) | Pat::Wild) {
            continue;
        }
        let name = format!("__exr_arg{k}");
        let pat = std::mem::replace(p, Pat::Ident(false, false, name.clone(), None));
        lets.push(Stmt::Let(pat, None, Some(acg::path_expr(&name)), None, Default::default()));
    }
    if !lets.is_empty() {
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
            stmts.push(Stmt::Let(p.clone(), None, Some(acg::path_expr(&id)), None, Default::default()));
            Pat::Ident(false, false, id, None)
        };
        params.push((pin, unit.clone()));
    }
    match &**body {
        Expr::Block(None, false, b) => stmts.extend(b.stmts.iter().cloned()),
        e => stmts.push(Stmt::Expr(e.clone(), false)),
    }
    if in_unsafe {
        stmts = vec![Stmt::Expr(Expr::Block(None, true, Block { stmts }), false)];
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
    ItemFn { attrs: vec![], sig, block: Block { stmts } }
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

impl Session2 {
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
            for &r in &a.refs[id] {
                if in_u(a.prog.item_of[r as usize]) {
                    h.add(&a.sig_key(r));
                }
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
        if a.prog.closures.contains_key(&(id as u32)) {
            h.add(&a.prog.names[id]);
        }
        h.add(&a.sig_key(id as u32));
        for &r in &a.refs[id] {
            if closed && a.prog.item_of[r as usize].is_some_and(|ri| (ri as usize) < pf) {
                continue;
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
                        for &r2 in &a.refs[r as usize] {
                            if a.inl.user_lines.contains_key(&r2) {
                                if let Some(vis) = a.prog.item_of[r2 as usize].and_then(|ri2| a.prog.traits_at_item(ri2)) {
                                    h.add(&vis.join(","));
                                }
                            }
                        }
                    }
                }
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
        h.finish()
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
    let name = ["cannot find type `", "cannot find trait `", "cannot find value `", "cannot find struct `"]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
        .and_then(|r| r.split('`').next());
    let (Some(name), Some(it)) = (name, items.iter().find(|it| it.line == line)) else { return e.to_string() };
    let text = &src[it.start..it.end.min(src.len())];
    let word = |i: usize| {
        let before = text[..i].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let after = text[i + name.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        before && after
    };
    match text.match_indices(name).map(|(i, _)| i).find(|&i| word(i)) {
        Some(i) => format!("{}:1: {rest}", line + text[..i].matches('\n').count()),
        None => e.to_string(),
    }
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
        .map_err(|e| if acg::has_loc(&e) { untag_lines(&e, shift) } else { format!("{}:1: in `{}`: {e}", it.line, a.prog.names[body as usize]) })?;
    // (what it told the closures it makes goes with its Blob: one read back (a cache, another process) tells them too)
    let clo_facts = insts.lock().unwrap().facts_since(l0);
    // (check は機械語にしない。ビルドは記録をバックエンドで機械語にする)
    if a.inl.check_only {
        return Ok((Blob { clo_facts, ..acg::Compiled::empty_blob() }, req));
    }
    lower_blob(a, c, req).map(|(b, req)| (Blob { clo_facts, ..b }, req))
}

/// 記録を機械語にして Blob を作る (バックエンドなしでは作れない)
#[cfg(not(feature = "build"))]
fn lower_blob(_: &Analysis, _: acg::Compiled, _: Vec<u32>) -> R<(Blob, Vec<u32>)> { Err("internal: zust-core was built without the backend (feature `build`)".into()) }

impl Session2 {
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
    let named = ["cannot find type `", "cannot find value `", "`Self` is only available in impls", "`self` is only allowed in impls", "unresolved import `", "the placeholder `_` is not allowed", "expected identifier, found keyword"];
    let unused = msg.starts_with("function `") && msg.contains("is never used");
    // (a derive's field or a field's type without the bound it needs (derive_field_error, wf_error): a local
    // `#[derive(Eq)] struct S(f64);` too)
    let bounds = ["the trait bound `", "binary operation `==` cannot be applied to type `", "can't compare `"].iter().any(|n| msg.starts_with(n))
        || (msg.starts_with('`') && (msg.contains("` doesn't implement `Debug`") || msg.contains("` is not an iterator")));
    !msg.contains("__h") && !msg.contains("unsupported") && (named.iter().any(|n| msg.starts_with(n)) || (msg.starts_with("cannot find constant `") && !msg.starts_with("cannot find constant `_`")) || unused || bounds)
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

impl Session2 {
    /// 型検査だけを行い、全診断を返す (`zustc check` と LSP)。到達しない関数も、総称関数は要求されたインスタンスごとに
    /// 検査する (コード生成までしてエラーを集め、結果はキャッシュに入れる: 次の検査・ビルドはそれを使う)。prelude の中の診断は出さない
    pub fn check(&mut self, src: &str) -> Vec<Diagnostic> {
        let user_lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
        self.checking = true;
        let r = self.analyze(src);
        self.checking = false;
        let mut a = match r {
            Ok(a) => a,
            Err(e) => return vec![Diagnostic::parse(&self.unhoist_analysis_err(src, e))],
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
        let old = match self.chk.take() {
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
                    if let Some(o) = &old {
                        if let Some(e) = o.ents.get(&name) {
                            if e.ok && Some(e.body_hash) == body_hash(&e.body) {
                                n_memo += 1;
                                let mut st: Vec<String> = e.children.clone();
                                memo.ents.insert(name.clone(), e.clone());
                                while let Some(c) = st.pop() {
                                    if seen.contains(&c) {
                                        continue;
                                    }
                                    let Some(ce) = o.ents.get(&c) else { continue };
                                    if ce.ok && Some(ce.body_hash) == body_hash(&ce.body) {
                                        seen.insert(c.clone());
                                        n_memo += 1;
                                        st.extend(ce.children.iter().cloned());
                                        memo.ents.insert(c, ce.clone());
                                    } else {
                                        // (本体が変わったインスタンス: 要求し直して検査する)
                                        let _ = crate::cache2::decode_blob(&ce.me, &a.prog, &insts);
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
                    let hit = match self.get(k).and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)) {
                        Some(b) => Some(b),
                        None if check_only => self.get(kc).and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)),
                        None => None,
                    };
                    let mut ok = true;
                    let req: Option<Vec<u32>> = match hit {
                        Some(b) => {
                            n_hit += 1;
                            Some(b.relocs.iter().filter_map(|r| match r.target { acg::Target::Func(i) => Some(i), _ => None }).collect())
                        }
                        None => {
                            n_checked += 1;
                            match compile_id_req(&a, &insts, id) {
                                Ok((b, req)) => {
                                    if !self.opts.no_cache {
                                        let b = if check_only {
                                            let relocs = req.iter().map(|&i| acg::BlobReloc { offset: 0, kind: 4, addend: 0, target: acg::Target::Func(i) }).collect();
                                            Blob { relocs, ..b }
                                        } else {
                                            b
                                        };
                                        let mut e = vec![];
                                        {
                                            let g = insts.lock().unwrap();
                                            crate::cache2::encode_blob(&b, &a.prog, &*g, &mut e);
                                        }
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
                                        None => errs.push(e),
                                    }
                                    ok = false;
                                    Some(vec![])
                                }
                            }
                        }
                    };
                    // (記録)
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
                            crate::cache2::encode_blob(&b, &a.prog, &*g, &mut me);
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
        self.const_lines.clear();
        let mut d: Vec<Diagnostic> = errs.iter().map(|e| Diagnostic::parse(e)).collect();
        // (an item moved out of a fn body (`fn main() { fn helper() { .. } }`) is past the source's last line: its borrow
        // and region checks' errors are at the lines it was moved from. Not its other errors: typeck does not know all
        // such an item may name where it was (`Self` of the impl around it), so they are left out as before)
        if d.iter().any(|x| x.line > user_lines && acg::borrow_check_error(&x.msg)) {
            if let Some(moved) = self.moved_lines(src) {
                for x in &mut d {
                    if x.line > user_lines && acg::borrow_check_error(&x.msg) {
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
    for id in 0..p.names.len() {
        let sig = &p.sigs[id];
        // (a trait's default method no impl uses: its `Self` is a placeholder too, bounded by the trait)
        if !sig.is_generic() || !sig.has_body || p.closures.contains_key(&(id as u32)) {
            continue;
        }
        // (a fn of which instances are made: checked with them. Unless every instance gives a `Copy` type for a
        // parameter its bounds do not make `Copy` (`dup(1u32)` of `fn dup<T>(x: T) -> (T, T) { (x, x) }`): those hide
        // what moving a T does, so the body is checked with placeholders too)
        let recheck = match made.get(&(id as u32)) {
            Some(insts) => {
                let hides = |args: &Vec<Ty>| args.iter().zip(&sig.generics).any(|(&a, (_, bs))| !bs.iter().any(|b| b == "Copy") && acg::borrowck::is_copy(p, a, 0));
                if !insts.iter().all(hides) {
                    continue;
                }
                true
            }
            None => false,
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
        let calls_bound = sig.generics.iter().flat_map(|(_, bs)| bs.iter()).any(|b| {
            // (`Fn(..)` / `FnMut(..)` / `FnOnce(..)`: a call of the parameter is checked against the bound itself)
            if matches!(b.as_str(), "Copy" | "Sized" | "?Sized" | "Send" | "Sync" | "Unpin") || crate::ast_summary::parse_fn_bound(b).is_some() {
                return false;
            }
            let tr = b.split('<').next().unwrap_or(b).trim();
            let tail = format!("::{tr}");
            let decls = p.traits.get(tr).or_else(|| p.traits.iter().find(|(k, _)| k.ends_with(&tail)).map(|(_, v)| v));
            // (a trait not known: as calling)
            // (but a method whose declaration names lifetimes (the trait's `'a`, its own): its call says something of
            // the regions)
            decls.is_none_or(|ds| {
                ds.iter().any(|&d| {
                    let called = p.names[d as usize].rsplit("::").next().is_some_and(|m| body.contains(&format!(".{m}(")) || body.contains(&format!("::{m}(")));
                    called && !p.sigs[d as usize].regions.as_deref().is_some_and(|r| !r.lts.is_empty())
                })
            })
        });
        if calls_bound {
            continue;
        }
        let args: Vec<Ty> = sig.generics.iter().enumerate().map(|(k, (n, bs))| Ty::Struct(p.placeholder(id as u32, k as u32, n, bs))).collect();
        if let (Ok(iid), true) = (g.request(p, id as u32, args), recheck) {
            g.recheck.insert(iid);
        }
    }
    std::mem::take(&mut g.pending)
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
    let maybe_unsized = |text: &str, n: &str| {
        text.match_indices("?Sized").any(|(i, _)| {
            let before = &text[..i];
            let b = before.as_bytes();
            let mut j = b.len();
            while j > 0 && (b[j - 1].is_ascii_alphanumeric() || matches!(b[j - 1], b'_' | b' ' | b'\t' | b'\n' | b'\r' | b'+' | b'\'' | b'<' | b'>' | b',')) {
                j -= 1;
            }
            if j == 0 || b[j - 1] != b':' || (j >= 2 && b[j - 2] == b':') {
                return false;
            }
            before[..j - 1].trim_end().rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next() == Some(n)
        })
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
                        // (std's `Clone`, `Copy` and `Default` have `Sized` as a supertrait: `T: ?Sized + Clone` is Sized)
                        bs.iter().any(|b| b == "Sized" || prog.bound_implies(b, &["Sized", "Clone", "Copy", "Default"], false, 0)) || !maybe_unsized(text, n)
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
        let Some(&(_, feat)) = GATED.iter().find(|g| g.0 == name) else { continue };
        // (the source's enabled features: read once, at the first gated attribute)
        let enabled = feats.get_or_insert_with(|| enabled_features(src));
        if !enabled.iter().any(|f| f == feat) {
            let (line, col) = at(k);
            return Some(format!("{line}:{col}: the `{name}` attribute is an experimental feature (E0658)"));
        }
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
                    if !name.is_empty() && b.get(j) != Some(&b':') && (GATED.iter().any(|g| g.0.as_bytes() == name) || name == b"target_feature") {
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
