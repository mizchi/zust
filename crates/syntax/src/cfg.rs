//! 条件付きコンパイル: `#[cfg(..)]` の付いたもの (アイテム・フィールド・バリアント・match の腕・文・式) を
//! 述語が偽なら空白にする。`#[cfg_attr(p, a)]` は p が真なら `#[a]`、偽なら消す。`cfg!(p)` は `true` / `false` にする。
//! ついでに `extern crate x;` (std / core / alloc と、ワークスペースの依存) も消す。
//! バイト位置と行は変えない (空白で塗る) ので、エラーと panic の場所はそのまま。
//!
//! 述語: `feature = ".."` (クレートごとの集合)・`debug_assertions` (-O でなければ真)・`unix` / `target_os = "linux"` など
//! x86-64 Linux の値・`test` / `doc` / `miri` は偽・`not` / `all` / `any`。知らない名前は偽

/// cfg の述語を評価するための設定
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CfgSet {
    pub features: Vec<String>,
    pub debug_assertions: bool,
    /// the target is wasm32-unknown-unknown (`target_arch = "wasm32"`, `target_family = "wasm"`, no `unix`).
    /// `target_pointer_width` stays "64": zustc's wasm32 output keeps 64-bit pointers and usize
    pub wasm: bool,
    /// more cfgs of this crate, as a build script's `rustc-cfg` gives them: `name`, or `name = "value"`
    pub cfgs: Vec<(String, Option<String>)>,
    /// the crate's compile-time environment for `env!` / `option_env!` (cargo's `CARGO_PKG_*`, `OUT_DIR`, a build
    /// script's `rustc-env`). A name that is not here is looked up in zustc's own environment, as rustc does
    pub env: Vec<(String, String)>,
}

impl CfgSet {
    /// the value `env!(name)` / `option_env!(name)` gives in this crate
    pub fn env_var(&self, name: &str) -> Option<String> {
        match self.env.iter().rev().find(|(k, _)| k == name) {
            Some((_, v)) => Some(v.clone()),
            None => std::env::var(name).ok(),
        }
    }

    fn flag(&self, name: &str) -> bool {
        if self.cfgs.iter().any(|(k, v)| k == name && v.is_none()) {
            return true;
        }
        match name {
            "unix" => !self.wasm,
            "debug_assertions" => self.debug_assertions,
            "overflow_checks" => self.debug_assertions,
            _ => false,
        }
    }

    fn kv(&self, key: &str, val: &str) -> bool {
        if self.cfgs.iter().any(|(k, v)| k == key && v.as_deref() == Some(val)) {
            return true;
        }
        match key {
            "feature" => self.features.iter().any(|f| f == val),
            "target_os" if self.wasm => val == "unknown",
            "target_family" if self.wasm => val == "wasm",
            "target_arch" if self.wasm => val == "wasm32",
            "target_env" if self.wasm => val.is_empty(),
            "panic" if self.wasm => val == "abort",
            "target_feature" if self.wasm => val == "bulk-memory" || val == "sign-ext" || val == "nontrapping-fptoint" || val == "multivalue" || val == "simd128" || val == "relaxed-simd",
            "target_os" => val == "linux",
            "target_family" => val == "unix",
            "target_arch" => val == "x86_64",
            "target_pointer_width" => val == "64",
            "target_endian" => val == "little",
            "target_env" => val == "gnu",
            "target_vendor" => val == "unknown",
            "panic" => val == "unwind",
            "target_has_atomic" => val == "8" || val == "16" || val == "32" || val == "64" || val == "ptr",
            "target_feature" => val == "sse" || val == "sse2" || val == "fxsr",
            _ => false,
        }
    }
}

/// 字句: (種類, 開始, 終了)。種類 0 = 識別子, 1 = 記号 1 文字, 2 = リテラル
type Tok = (u8, usize, usize);

/// 述語 toks[i..] を評価し、(値, 次の位置)
fn eval(s: &[u8], toks: &[Tok], mut i: usize, c: &CfgSet) -> (bool, usize) {
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    let name: &[u8] = t(i);
    i += 1;
    // (`not((any(..)))`: a predicate in parentheses, as a macro's `$e:expr` puts it)
    if name == b"(" {
        let (v, j) = eval(s, toks, i, c);
        return (v, if t(j) == b")" { j + 1 } else { j });
    }
    if t(i) == b"(" && (name == b"not" || name == b"all" || name == b"any") {
        i += 1;
        let mut vals: Vec<bool> = vec![];
        while i < toks.len() && t(i) != b")" {
            let (v, j) = eval(s, toks, i, c);
            vals.push(v);
            i = j;
            if t(i) == b"," {
                i += 1;
            }
        }
        i += 1;
        let v = match name {
            b"not" => !vals.first().copied().unwrap_or(false),
            b"all" => vals.iter().all(|&v| v),
            _ => vals.iter().any(|&v| v),
        };
        return (v, i);
    }
    // (`cfg(true)` / `cfg(false)`: boolean literals as predicates)
    if name == b"true" || name == b"false" {
        return (name == b"true", i);
    }
    let name = String::from_utf8_lossy(name).into_owned();
    if t(i) == b"=" && toks.get(i + 1).is_some_and(|x| x.0 == 2) {
        let lit = t(i + 1);
        let mut val = String::from_utf8_lossy(&lit[1..lit.len() - 1]).into_owned();
        // (`label = "a\"b"`: the literal's value, as a build script's `rustc-cfg` gives it)
        if val.contains('\\') {
            val = unescape(&val);
        }
        return (c.kv(&name, &val), i + 2);
    }
    (c.flag(&name), i)
}

/// The value of a string literal's text between its quotes (`\"`, `\\`, `\n`, `\t`, `\r`, `\0`, `\xNN`, `\u{..}`)
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(ch) = it.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('x') => {
                let hex: String = it.by_ref().take(2).collect();
                if let Some(c) = u8::from_str_radix(&hex, 16).ok().filter(|b| b.is_ascii()) {
                    out.push(c as char);
                }
            }
            Some('u') => {
                let hex: String = it.by_ref().skip_while(|&c| c == '{').take_while(|&c| c != '}').collect();
                if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(c);
                }
            }
            Some(x) => out.push(x),
            None => {}
        }
    }
    out
}

fn is_open(b: &[u8]) -> bool {
    b == b"(" || b == b"[" || b == b"{"
}

fn is_close(b: &[u8]) -> bool {
    b == b")" || b == b"]" || b == b"}"
}

/// 対応する閉じ括弧の位置
fn matching(s: &[u8], toks: &[Tok], i: usize) -> usize {
    let mut d = 0i32;
    let mut k = i;
    while k < toks.len() {
        let b = &s[toks[k].1..toks[k].2];
        if toks[k].0 == 1 && is_open(b) {
            d += 1;
        } else if toks[k].0 == 1 && is_close(b) {
            d -= 1;
            if d == 0 {
                return k;
            }
        }
        k += 1;
    }
    toks.len() - 1
}

fn blank(out: &mut [u8], a: usize, b: usize) {
    for k in a..b {
        if out[k] != b'\n' {
            out[k] = b' ';
        }
    }
}

/// 属性の後ろの「もの」の終わり (その最後の字句の位置)。i は対象の最初の字句
fn thing_end(s: &[u8], toks: &[Tok], mut i: usize) -> usize {
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    // (ほかの属性を飛ばす)
    while t(i) == b"#" && t(i + 1) == b"[" {
        i = matching(s, toks, i + 1) + 1;
    }
    // (`;` まで続くもの: `const X: T = S { .. };`・`use a::{b, c};`・`let`・`type`・`static`)
    let mut j = i;
    while matches!(t(j), b"pub" | b"unsafe" | b"async" | b"extern") || t(j).first() == Some(&b'"') {
        if t(j) == b"pub" && t(j + 1) == b"(" {
            j = matching(s, toks, j + 1);
        }
        j += 1;
    }
    let semi_item = matches!(t(j), b"const" | b"static" | b"use" | b"let" | b"type") && t(j + 1) != b"fn";
    // (an item with a keyword ends at its `;` or its block, not at a `,` of `<T, F>` or of its where clause)
    let keyword_item = matches!(t(j), b"fn" | b"struct" | b"enum" | b"union" | b"impl" | b"trait" | b"mod" | b"const" | b"static" | b"type" | b"use");
    let mut k = i;
    while k < toks.len() {
        let b = t(k);
        if toks[k].0 == 1 && is_close(b) {
            // (包んでいるものの終わり: その手前まで)
            return k - 1;
        }
        if toks[k].0 == 1 && (b == b";" || (b == b"," && !keyword_item)) {
            return k;
        }
        if toks[k].0 == 1 && is_open(b) {
            let m = matching(s, toks, k);
            // (`#[cfg(x)] if a { .. } else if b { .. } else { .. }`: the whole `if`)
            if b == b"{" && !semi_item && t(j) == b"if" && t(m + 1) == b"else" {
                k = m + 2;
                continue;
            }
            if b == b"{" && !semi_item {
                // (ブロックで終わるもの。後ろの `,` も消す: バリアント・match の腕)
                return if t(m + 1) == b"," { m + 1 } else { m };
            }
            k = m + 1;
            continue;
        }
        k += 1;
    }
    toks.len() - 1
}

/// std's macros whose arguments are expressions zustc reads as they are (no macro_rules expansion follows): their
/// `cfg!` is evaluated with the rest of the code
fn format_like(name: &[u8]) -> bool {
    matches!(
        name,
        b"println" | b"print" | b"eprintln" | b"eprint" | b"format" | b"format_args" | b"write" | b"writeln" | b"panic" | b"assert"
            | b"assert_eq" | b"assert_ne" | b"debug_assert" | b"debug_assert_eq" | b"debug_assert_ne" | b"unreachable"
            | b"unimplemented" | b"todo" | b"vec" | b"dbg"
    )
}

/// src の cfg を評価する。変わらなければ None
/// `cfg_select! { pred => { .. } _ => { .. } }`: the tokens of the first arm whose predicate holds, in place of the
/// whole call (the rest blanked, newlines kept, so lines and columns stay). Nested calls in the chosen arm are
/// expanded too; not inside `macro_rules!` (a predicate there may be a metavariable, expanded first)
pub fn expand_select(src: &str, c: &CfgSet) -> Option<String> {
    if !src.contains("cfg_select") {
        return None;
    }
    let mut out = src.as_bytes().to_vec();
    let mut changed = false;
    for _ in 0..16 {
        let s = out.clone();
        let toks = crate::lexer::tokenize(&s);
        let t = |k: usize| -> &[u8] {
            match toks.get(k) {
                Some(&(_, a, b)) => &s[a..b],
                None => b"",
            }
        };
        let mut found = false;
        let mut i = 0;
        while i + 2 < toks.len() {
            // (a `macro_rules!` body is skipped)
            if t(i) == b"macro_rules" && t(i + 1) == b"!" && toks.get(i + 2).is_some_and(|x| x.0 == 0) && is_open(t(i + 3)) {
                i = matching(&s, &toks, i + 3) + 1;
                continue;
            }
            // (another macro's arguments are its own: `stringify!(cfg_select! { .. })` keeps them; a macro_rules! call's
            // are expanded with it. The std macros that take expressions, `println!(cfg_select! { .. })`, are looked in)
            if toks[i].0 == 0 && t(i) != b"cfg_select" && t(i + 1) == b"!" && is_open(t(i + 2)) && !expr_macro(t(i)) {
                i = matching(&s, &toks, i + 2) + 1;
                continue;
            }
            if !(toks[i].0 == 0 && t(i) == b"cfg_select" && t(i + 1) == b"!" && is_open(t(i + 2))) {
                i += 1;
                continue;
            }
            let close = matching(&s, &toks, i + 2);
            // (the call's path: `std::cfg_select!` / `::core::cfg_select!` from its first segment)
            let mut st = i;
            while st >= 2 && t(st - 1) == b":" && t(st - 2) == b":" {
                if st >= 3 && toks[st - 3].0 == 0 {
                    st -= 3;
                } else {
                    st -= 2;
                    break;
                }
            }
            // (in an expression, not as statements or items: after an operator, `=`, `(`, `,`, `=>`, `return` ..)
            let in_expr = st > 0 && !matches!(t(st - 1), b"{" | b"}" | b";" | b"]");
            // the arms: `pred => { .. }` / `_ => { .. }`, an optional `,` after each
            let mut k = i + 3;
            let mut chosen: Option<(usize, usize)> = None;
            let mut ok = true;
            while k < close {
                let (v, j) = if t(k) == b"_" { (true, k + 1) } else { eval(&s, &toks, k, c) };
                if !(t(j) == b"=" && t(j + 1) == b">" && is_open(t(j + 2))) {
                    ok = false;
                    break;
                }
                let bc = matching(&s, &toks, j + 2);
                if v && chosen.is_none() {
                    chosen = Some((j + 2, bc));
                }
                k = bc + 1;
                if t(k) == b"," {
                    k += 1;
                }
            }
            if !ok || close >= toks.len() {
                i += 1;
                continue;
            }
            match chosen {
                // (a `{ .. }` arm gives its contents as statements or items; a `( .. )` / `[ .. ]` one, and a `{ .. }`
                // one of more than one token in an expression, keeps its delimiters: `2 * cfg_select! { _ => { 1 + 2 } }`
                // is 6, a block. One token, `println!(cfg_select! { _ => { "x" } })`, is itself)
                Some((bo, bc)) if t(bo) == b"{" && (!in_expr || bc == bo + 2) => {
                    blank(&mut out, toks[st].1, toks[bo].2);
                    blank(&mut out, toks[bc].1, toks[close].2);
                }
                Some((bo, bc)) => {
                    blank(&mut out, toks[st].1, toks[bo].1);
                    blank(&mut out, toks[bc].2, toks[close].2);
                }
                // (no arm holds: an error at the call, as rustc's "none of the predicates in this `cfg_select`
                // evaluated to true")
                None => {
                    let (a, b) = (toks[st].1, toks[close].2);
                    blank(&mut out, a, b);
                    let w: &[u8] = b"compile_error!(\"none of the predicates in this `cfg_select` evaluated to true\")";
                    if b - a >= w.len() {
                        out[a..a + w.len()].copy_from_slice(w);
                    } else {
                        out.splice(a..a, w.iter().copied());
                    }
                }
            }
            found = true;
            changed = true;
            i = close + 1;
        }
        if !found {
            break;
        }
    }
    if changed { String::from_utf8(out).ok() } else { None }
}

pub fn strip(src: &str, c: &CfgSet) -> Option<String> {
    strip_noting_env(src, c, &mut None)
}

/// strip, and whether the source may call `env!` / `option_env!` (macros::may_use_env), found on the way by the
/// same tokens: `env` is left None when the source was not read by tokens (nothing to evaluate in it)
pub fn strip_noting_env(src: &str, c: &CfgSet, env: &mut Option<bool>) -> Option<String> {
    if !src.contains("cfg") && !src.contains("extern crate") {
        return None;
    }
    // (a `cfg_select!` call the tokens below pass: its chosen arm in place, then that arm's own cfgs)
    let mut select = false;
    let mut seen = false;
    let r = strip_toks(src, c, &mut select, &mut seen);
    *env = Some(seen);
    if select {
        if let Some(e) = expand_select(r.as_deref().unwrap_or(src), c) {
            return Some(strip_toks(&e, c, &mut false, &mut false).unwrap_or(e));
        }
    }
    r
}

/// The `extern crate x;` items of `src` that [`strip`] blanked in `stripped` as such (its own pass, not one that went
/// with a configured-away item or module: the token before it is kept, is such an item's, or ends its own true
/// `#[cfg(..)]`), and not in a comment: (byte offset, the item's text). One tokenizing of the text, only when it has an
/// `extern crate`
pub fn blanked_externs(src: &str, stripped: &str, c: &CfgSet) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = vec![];
    if src.len() != stripped.len() || !src.contains("extern crate") {
        return out;
    }
    let (s, st) = (src.as_bytes(), stripped.as_bytes());
    let toks = crate::lexer::tokenize(s);
    let t = |k: usize| -> &[u8] { toks.get(k).map_or(&b""[..], |&(_, a, b)| &s[a..b]) };
    // (where the last item taken ends)
    let mut last_end = usize::MAX;
    for i in 0..toks.len() {
        if t(i) != b"extern" || t(i + 1) != b"crate" || !toks.get(i + 2).is_some_and(|x| x.0 == 0) {
            continue;
        }
        let Some(e) = (i + 3..(i + 6).min(toks.len())).find(|&e| t(e) == b";") else { continue };
        let (a, b) = (toks[i].1, toks[e].2);
        let blank = st[a..b].iter().all(u8::is_ascii_whitespace);
        // (its own attributes `#[cfg(p)]`, which went with the item when p is false: back to where they start)
        let mut j = i;
        let mut cfg_true = true;
        while j >= 3 && t(j - 1) == b"]" {
            let Some(o) = (0..j - 1).rev().find(|&o| t(o) == b"[" && o >= 1 && t(o - 1) == b"#") else { break };
            if t(o + 1) == b"cfg" && t(o + 2) == b"(" {
                cfg_true &= eval(s, &toks, o + 3, c).0;
            }
            j = o - 1;
        }
        let before_kept = j == 0 || {
            let (_, pa, pb) = toks[j - 1];
            st[pa..pb] == s[pa..pb] || pb == last_end
        };
        let before_kept = before_kept && cfg_true;
        if blank && before_kept {
            out.push((a, src[a..b].to_string()));
            last_end = b;
        }
    }
    out
}

/// The first outer attribute of the attributes just before the one at i (`#[a] #[b] #[cfg(x)]`: `#[a]`'s byte offset)
fn attrs_start(s: &[u8], toks: &[Tok], i: usize) -> usize {
    let t = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let mut j = i;
    while j >= 3 && toks[j - 1].0 == 1 && t(j - 1) == b"]" {
        // (back to its `[`)
        let mut depth = 0usize;
        let mut o = j - 1;
        loop {
            if toks[o].0 == 1 {
                match t(o) {
                    b")" | b"]" | b"}" => depth += 1,
                    b"(" | b"[" | b"{" => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            if depth == 0 || o == 0 {
                break;
            }
            o -= 1;
        }
        if o == 0 || t(o) != b"[" || t(o - 1) != b"#" {
            break;
        }
        j = o - 1;
    }
    toks[j].1
}

fn strip_toks(src: &str, c: &CfgSet, select: &mut bool, env: &mut bool) -> Option<String> {
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let mut out = s.to_vec();
    let mut changed = false;
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    let mut i = 0;
    while i < toks.len() {
        // `cfg!(p)` → `true` / `false`
        if t(i) == b"cfg" && t(i + 1) == b"!" && t(i + 2) == b"(" {
            let close = matching(s, &toks, i + 2);
            // (`cfg!(version(..))` is unstable: left for acg, which says so (E0658))
            if (i + 3..close).any(|k| t(k) == b"version" && t(k + 1) == b"(") {
                i = close + 1;
                continue;
            }
            let (v, _) = eval(s, &toks, i + 3, c);
            let (a, b) = (toks[i].1, toks[close].2);
            blank(&mut out, a, b);
            let w: &[u8] = if v { b"true" } else { b"false" };
            if b - a >= w.len() {
                out[a..a + w.len()].copy_from_slice(w);
            }
            changed = true;
            i = close + 1;
            continue;
        }
        // マクロの呼び出しと macro_rules! の中は見ない (展開してから評価する: cfg_if! の `if #[cfg(..)] { .. }`)。
        // std の書式のマクロ (`println!("{}", cfg!(unix))`) は後で展開されないので、中も評価する
        if toks[i].0 == 0 && t(i) != b"cfg" && t(i + 1) == b"!" && !format_like(t(i)) {
            *env |= matches!(t(i), b"env" | b"option_env");
            let g = if t(i + 2) == b"(" || t(i + 2) == b"[" || t(i + 2) == b"{" {
                Some(i + 2)
            } else if toks.get(i + 2).is_some_and(|x| x.0 == 0) && t(i + 3) == b"{" {
                Some(i + 3)
            } else {
                None
            };
            if let Some(g) = g {
                let end = matching(s, &toks, g);
                // (`include!(concat!(env!("OUT_DIR"), ..))`, `env!($k)` in a macro_rules! body)
                *env = *env || (end < toks.len() && crate::macros::may_use_env(&src[toks[g].1..toks[end].2]));
                // (also in the arguments of a std macro that takes expressions: `println!(cfg_select! { .. })`)
                *select |= t(i) == b"cfg_select"
                    || (expr_macro(t(i)) && end < toks.len() && src[toks[g].1..toks[end].2].contains("cfg_select"));
                i = end + 1;
                continue;
            }
        }
        // `extern crate x [as y];`
        if t(i) == b"extern" && t(i + 1) == b"crate" && toks.get(i + 2).is_some_and(|x| x.0 == 0) {
            let mut e = i + 3;
            while e < toks.len() && t(e) != b";" && e < i + 6 {
                e += 1;
            }
            if t(e) == b";" {
                blank(&mut out, toks[i].1, toks[e].2);
                changed = true;
                i = e + 1;
                continue;
            }
        }
        if t(i) != b"#" {
            i += 1;
            continue;
        }
        let inner = t(i + 1) == b"!";
        let lb = if inner { i + 2 } else { i + 1 };
        if t(lb) != b"[" {
            i += 1;
            continue;
        }
        let rb = matching(s, &toks, lb);
        let name = t(lb + 1);
        if (name == b"cfg" || name == b"cfg_attr") && t(lb + 2) == b"(" {
            let pclose = matching(s, &toks, lb + 2);
            let (mut v, after) = eval(s, &toks, lb + 3, c);
            changed = true;
            // (`#[cfg_attr(p, cfg(q))]` with p true is `#[cfg(q)]`)
            let mut name = name;
            if name == b"cfg_attr" && v && t(after) == b"," && t(after + 1) == b"cfg" && t(after + 2) == b"(" && matching(s, &toks, after + 2) + 1 == pclose {
                v = eval(s, &toks, after + 3, c).0;
                name = b"cfg";
            }
            if name == b"cfg_attr" {
                if v && t(after) == b"," {
                    // `#[cfg_attr(p, a, b)]` → `#[         a, b ]`
                    blank(&mut out, toks[lb + 1].1, toks[after].2);
                    blank(&mut out, toks[pclose].1, toks[pclose].2);
                    i = after + 1;
                    continue;
                }
                blank(&mut out, toks[i].1, toks[rb].2);
                i = rb + 1;
                continue;
            }
            if inner {
                // (`#![cfg(..)]` が偽ならファイル全体。今は属性だけ消す)
                blank(&mut out, toks[i].1, toks[rb].2);
                if !v {
                    blank(&mut out, 0, s.len());
                    return String::from_utf8(out).ok();
                }
                i = rb + 1;
                continue;
            }
            if v {
                blank(&mut out, toks[i].1, toks[rb].2);
                i = rb + 1;
            } else {
                // (with the attributes before it: `#[derive(Debug)] #[cfg(x)] struct A;` would leave the derive on the
                // next item)
                let e = thing_end(s, &toks, rb + 1);
                blank(&mut out, attrs_start(s, &toks, i), toks[e.max(rb)].2);
                i = e.max(rb) + 1;
            }
            continue;
        }
        i += 1;
    }
    if changed { String::from_utf8(out).ok() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, feats: &[&str]) -> String {
        let c = CfgSet { features: feats.iter().map(|s| s.to_string()).collect(), debug_assertions: true, ..Default::default() };
        let o = strip(src, &c).unwrap_or_else(|| src.to_string());
        assert_eq!(o.len(), src.len());
        o.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn items_fields_arms() {
        assert_eq!(run("#[cfg(feature = \"std\")] fn a() {} fn b() {}", &[]), "fn b() {}");
        assert_eq!(run("#[cfg(feature = \"std\")] fn a() {} fn b() {}", &["std"]), "fn a() {} fn b() {}");
        assert_eq!(run("struct S { #[cfg(test)] a: u8, b: u8 }", &[]), "struct S { b: u8 }");
        assert_eq!(run("enum E { #[cfg(not(unix))] A { x: u8 }, B }", &[]), "enum E { B }");
        assert_eq!(run("match x { #[cfg(any(test, doc))] 1 => {} _ => 2 }", &[]), "match x { _ => 2 }");
        assert_eq!(run("#[cfg(test)] const X: S = S { a: 1 }; fn f() {}", &[]), "fn f() {}");
        assert_eq!(run("#[cfg(test)] use a::{b, c}; fn f() {}", &[]), "fn f() {}");
        // (a value with an escape: compared as the literal's value)
        let q = CfgSet { cfgs: vec![("label".into(), Some("a\"b".into()))], ..Default::default() };
        assert!(strip("#[cfg(label = \"a\\\"b\")] fn f() {}", &q).unwrap().contains("fn f()"));
        assert!(!strip("#[cfg(label = \"ab\")] fn f() {}", &q).unwrap().contains("fn f()"));
        let a = CfgSet { cfgs: vec![("label".into(), Some("ab".into()))], ..Default::default() };
        assert!(strip("#[cfg(label = \"\\x61\\u{62}\")] fn f() {}", &a).unwrap().contains("fn f()"));
        // (whether the source may call `env!`, seen by the same tokens: also inside a macro's arguments)
        let env = |src: &str| {
            let mut e = None;
            strip_noting_env(src, &CfgSet::default(), &mut e);
            e
        };
        assert_eq!(env("#[cfg(x)] fn f() {} fn g() -> &'static str { env ! (\"A\") }"), Some(true));
        assert_eq!(env("#[cfg(x)] fn f() {} include!(concat!(env!(\"OUT_DIR\"), \"/a.rs\"));"), Some(true));
        assert_eq!(env("#[cfg(x)] fn f() {} const S: &str = \"env!(A)\"; use std::env;"), Some(false));
        assert_eq!(env("fn f() {}"), None);
        // (the attributes before a false `#[cfg]` go with its item, not on to the next one)
        assert_eq!(run("#[derive(Debug)] #[cfg(test)] struct A; struct B;", &[]), "struct B;");
        assert_eq!(run("#[inline] fn a() {} #[pm::traced]\n#[cfg(any())] fn h() {} fn b() {}", &[]), "#[inline] fn a() {} fn b() {}");
        assert_eq!(run("struct S { #[serde(x)] #[cfg(test)] a: u8, b: u8 }", &[]), "struct S { b: u8 }");
        assert_eq!(run("#[derive(Debug)] #[cfg_attr(test, inline)] #[cfg(test)] struct A;", &[]), "");
        assert_eq!(run("#[cfg_attr(feature = \"std\", derive(Debug))] struct S;", &["std"]), "#[ derive(Debug) ] struct S;");
        assert_eq!(run("#![cfg_attr(not(feature = \"std\"), no_std)] fn f() {}", &["std"]), "fn f() {}");
        assert_eq!(run("let a = cfg!(all(unix, target_pointer_width = \"64\"));", &[]), "let a = true ;");
        // (in the arguments of std's format macros too; a user macro's are left for after its expansion)
        assert_eq!(run("println!(\"{}\", cfg!(unix));", &[]), "println!(\"{}\", true );");
        assert_eq!(run("m!(cfg!(unix));", &[]), "m!(cfg!(unix));");
        assert_eq!(run("fn f() { #[cfg(test)] g(); h(); }", &[]), "fn f() { h(); }");
        assert_eq!(run("[1, #[cfg(test)] 2, 3]", &[]), "[1, 3]");
        assert_eq!(run("m! { if #[cfg(test)] { a } } macro_rules! n { () => { #[cfg(test)] fn f() {} } }", &[]), "m! { if #[cfg(test)] { a } } macro_rules! n { () => { #[cfg(test)] fn f() {} } }");
        assert_eq!(run("#[cfg(feature = \"alloc\")] extern crate alloc; extern crate std as s; fn f() {}", &["alloc"]), "fn f() {}");
    }
}

/// std macros whose arguments are expressions (a `cfg_select!` in them is expanded in place)
fn expr_macro(n: &[u8]) -> bool {
    matches!(
        n,
        b"println" | b"print" | b"eprintln" | b"eprint" | b"format" | b"write" | b"writeln" | b"panic" | b"assert" | b"assert_eq"
            | b"assert_ne" | b"debug_assert" | b"debug_assert_eq" | b"debug_assert_ne" | b"vec" | b"dbg" | b"todo" | b"unreachable"
            | b"unimplemented" | b"format_args"
    )
}
