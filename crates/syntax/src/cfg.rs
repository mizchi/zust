//! Conditional compilation: blanks out things with `#[cfg(..)]` (items, fields, variants, match arms, statements, expressions)
//! when the predicate is false. `#[cfg_attr(p, a)]` becomes `#[a]` if p is true and is removed if false. `cfg!(p)` becomes `true` / `false`.
//! It also removes `extern crate x;` (std / core / alloc, and workspace dependencies).
//! Byte positions and lines are not changed (painted over with whitespace), so error and panic locations stay as they are.
//!
//! Predicates: `feature = ".."` (a set per crate), `debug_assertions` (true unless -O), `unix` / `target_os = "linux"` and other
//! x86-64 Linux values, `test` / `doc` / `miri` are false, `not` / `all` / `any`. Unknown names are false

/// Settings for evaluating cfg predicates
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
    /// the crate's edition, for what its tokens mean (2021's reserved prefixes: lexer::unknown_prefix). 0: unknown,
    /// and nothing edition-dependent is checked
    pub edition: u16,
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

/// Token: (kind, start, end). Kind 0 = identifier, 1 = one-character symbol, 2 = literal
type Tok = (u8, usize, usize);

/// Evaluates the predicate toks[i..]: (value, next position)
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
        // (`label = "a\"b"`, `label = "\u{6_1}"`: the literal's value, as a build script's `rustc-cfg` gives it)
        let val = str_value(&String::from_utf8_lossy(t(i + 1))).unwrap_or_default();
        return (c.kv(&name, &val), i + 2);
    }
    (c.flag(&name), i)
}

fn is_open(b: &[u8]) -> bool {
    b == b"(" || b == b"[" || b == b"{"
}

fn is_close(b: &[u8]) -> bool {
    b == b")" || b == b"]" || b == b"}"
}

/// The position of the matching closing bracket
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

/// The end of the "thing" after an attribute (the position of its last token). i is the first token of the target
fn thing_end(s: &[u8], toks: &[Tok], mut i: usize) -> usize {
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    // (Skip other attributes)
    while t(i) == b"#" && t(i + 1) == b"[" {
        i = matching(s, toks, i + 1) + 1;
    }
    // (Things that run up to `;`: `const X: T = S { .. };`, `use a::{b, c};`, `let`, `type`, `static`)
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
    // (a `:` that is not half of a `::` was passed: the type of a field or a parameter follows, `a: Map<K, V>,`)
    let mut typed = false;
    let mut k = i;
    while k < toks.len() {
        let b = t(k);
        if toks[k].0 != 1 {
            k += 1;
            continue;
        }
        if is_close(b) {
            // (The end of what encloses it: up to just before that)
            return k - 1;
        }
        if b == b";" || (b == b"," && !keyword_item) {
            return k;
        }
        if b == b":" && !joint(s, toks, k.wrapping_sub(1), b':') && !joint(s, toks, k, b':') {
            typed = true;
        }
        // (generic arguments hold `,`s: a field's or a parameter's type, a turbofish `f::<A, B>()`, and a `<` right after
        //  a name, `Map<K, V>` of a tuple field. Only a `<..>` that closes: `a < b, c` is a comparison)
        if b == b"<" && k > i && (typed || joint(s, toks, k - 1, b'<') && t(k - 1) == b":" || toks[k - 1].0 == 0 && toks[k - 1].2 == toks[k].1) {
            if let Some(e) = angle_close(s, toks, k) {
                k = e + 1;
                continue;
            }
        }
        if is_open(b) {
            let m = matching(s, toks, k);
            // (`#[cfg(x)] if a { .. } else if b { .. } else { .. }`: the whole `if`, also as an arm's body)
            if b == b"{" && !semi_item && t(m + 1) == b"else" {
                k = m + 2;
                continue;
            }
            // (a match arm's struct pattern, `S { .. } => ..`, `S { a } | T { a } => ..`, `S { a } if a > 0 => ..`: the
            //  arm goes on to its body)
            if b == b"{" && !semi_item && !keyword_item && pattern_goes_on(s, toks, m + 1) {
                k = m + 1;
                continue;
            }
            if b == b"{" && !semi_item {
                // (Things that end with a block. A following `,` is removed too: variants, match arms)
                return if t(m + 1) == b"," { m + 1 } else { m };
            }
            k = m + 1;
            continue;
        }
        k += 1;
    }
    toks.len() - 1
}

/// Whether toks[k] is a symbol and the token after it is the symbol `next`, starting where toks[k] ends (`::`, `=>`,
/// `->`)
fn joint(s: &[u8], toks: &[Tok], k: usize, next: u8) -> bool {
    match (toks.get(k), toks.get(k.wrapping_add(1))) {
        (Some(&(1, _, a)), Some(&(1, b, c))) => a == b && c == b + 1 && s[b] == next,
        _ => false,
    }
}

/// Where the `<..>` that opens at toks[k] closes (its `>`), at that depth: groups inside are passed, and the `>` of a
/// `->` closes nothing (`Box<dyn Fn() -> u8>`). None when it does not close before a `;`, a block or the end of what
/// encloses it (`a < b { .. }`), or when toks[k] is half of `<=` / `<-`
fn angle_close(s: &[u8], toks: &[Tok], k: usize) -> Option<usize> {
    if joint(s, toks, k, b'=') || joint(s, toks, k, b'-') {
        return None;
    }
    let mut d = 0u32;
    let mut j = k;
    while j < toks.len() {
        if toks[j].0 == 1 {
            match s[toks[j].1] {
                b'<' => d += 1,
                b'>' if !joint(s, toks, j - 1, b'>') || s[toks[j - 1].1] != b'-' => {
                    d -= 1;
                    if d == 0 {
                        return Some(j);
                    }
                }
                b'(' | b'[' => j = matching(s, toks, j),
                // (a const argument's block, `A<{ N + 1 }>`, `A<T, { N }>`)
                b'{' if matches!(s[toks[j - 1].1], b'<' | b',') => j = matching(s, toks, j),
                b';' | b')' | b']' | b'{' | b'}' => return None,
                _ => {}
            }
        }
        j += 1;
    }
    None
}

/// Whether a match arm's pattern goes on at toks[k], after a `{ .. }` of its (a struct pattern's): `=>`, an
/// or-pattern's `|`, or a guard `if ..` with its `=>` after it. A block statement ends at its `}` (`{ .. } if a { .. }`)
fn pattern_goes_on(s: &[u8], toks: &[Tok], k: usize) -> bool {
    let sym = |k: usize, c: u8| matches!(toks.get(k), Some(&(1, a, _)) if s[a] == c);
    if joint(s, toks, k, b'>') && sym(k, b'=') {
        return true;
    }
    if sym(k, b'|') {
        return !joint(s, toks, k, b'|') && !joint(s, toks, k, b'=');
    }
    if !matches!(toks.get(k), Some(&(0, a, b)) if &s[a..b] == b"if") {
        return false;
    }
    let mut j = k + 1;
    while j < toks.len() {
        if toks[j].0 == 1 {
            match s[toks[j].1] {
                b'=' if joint(s, toks, j, b'>') => return true,
                b'(' | b'[' | b'{' => j = matching(s, toks, j),
                b';' | b',' | b')' | b']' | b'}' => return false,
                _ => {}
            }
        }
        j += 1;
    }
    false
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

/// The byte ranges of the branches of `cfg_if! { if #[cfg(P)] { .. } else if #[cfg(Q)] { .. } else { .. } }` calls
/// that are not taken (the first branch whose predicate holds is; the else branch when none does), and of items
/// inside a call's branches whose own `#[cfg(..)]` does not hold. Only for what is read before expansion: the
/// out-of-line modules a branch declares (srcmap). Not inside `macro_rules!` bodies
pub(crate) fn cfg_if_not_taken(src: &str, c: &CfgSet) -> Vec<(usize, usize)> {
    let mut out = vec![];
    if !src.contains("cfg_if") {
        return out;
    }
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let t = |k: usize| -> &[u8] {
        match toks.get(k) {
            Some(&(_, a, b)) => &s[a..b],
            None => b"",
        }
    };
    // (`#[cfg(P)]` at k: (whether P holds, the token after `]`))
    let cfg_attr = |k: usize| -> Option<(bool, usize)> {
        if t(k) == b"#" && t(k + 1) == b"[" && t(k + 2) == b"cfg" && t(k + 3) == b"(" {
            let (v, j) = eval(s, &toks, k + 4, c);
            let close = matching(s, &toks, k + 1);
            (j <= close).then_some((v, close + 1))
        } else {
            None
        }
    };
    let mut i = 0;
    while i + 2 < toks.len() {
        if t(i) == b"macro_rules" && t(i + 1) == b"!" && toks.get(i + 2).is_some_and(|x| x.0 == 0) && is_open(t(i + 3)) {
            i = matching(s, &toks, i + 3) + 1;
            continue;
        }
        if !(t(i) == b"cfg_if" && t(i + 1) == b"!" && t(i + 2) == b"{") {
            i += 1;
            continue;
        }
        let close = matching(s, &toks, i + 2);
        let mut k = i + 3;
        let mut taken = false;
        while k < close {
            // (`if #[cfg(P)] {` / `else if #[cfg(P)] {` / `else {`)
            let mut holds = true;
            if t(k) == b"else" {
                k += 1;
            }
            if t(k) == b"if" {
                let Some((v, j)) = cfg_attr(k + 1) else { break };
                holds = v;
                k = j;
            }
            if t(k) != b"{" {
                break;
            }
            let end = matching(s, &toks, k);
            if taken || !holds {
                out.push((toks[k].1, toks[end].2));
            } else {
                taken = true;
                // (an item's own `#[cfg(..)]` in the taken branch, `#[cfg(feature = "rng")] mod avx2;`: up to its `;`
                //  or its braces)
                let mut m = k + 1;
                while m < end {
                    if let Some((false, j)) = cfg_attr(m) {
                        let mut e = j;
                        while e < end && t(e) != b";" && t(e) != b"{" {
                            e += 1;
                        }
                        let e = if t(e) == b"{" { matching(s, &toks, e) } else { e };
                        out.push((toks[m].1, toks[e.min(end)].2));
                        m = e + 1;
                        continue;
                    }
                    if is_open(t(m)) && t(m) != b"[" {
                        // (nested braces: a nested `cfg_if!` is taken up by the outer loop, its own items below)
                        if !(m >= 2 && t(m - 1) == b"!" && t(m - 2) == b"cfg_if") {
                            m = matching(s, &toks, m) + 1;
                            continue;
                        }
                    }
                    m += 1;
                }
            }
            k = end + 1;
        }
        i += 3;
    }
    out
}

/// Evaluates the cfgs in src. None if nothing changes
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
    strip_noting(src, c, env, &mut vec![])
}

/// [`strip_noting_env`], and the byte ranges of the `extern crate x;` items it blanks: those of the configured text (not
/// in an item or module a false `#[cfg]` takes away, nor in a macro's input), whose names the duplicate check reads
pub fn strip_noting(src: &str, c: &CfgSet, env: &mut Option<bool>, externs: &mut Vec<(usize, usize)>) -> Option<String> {
    if !src.contains("cfg") && !src.contains("extern crate") {
        return None;
    }
    // (nothing the tokens below rewrite, and no `env` for the question of `env!`: its answer is no)
    if !may_configure(src) && !src.contains("env") {
        *env = Some(false);
        return None;
    }
    // (a `cfg_select!` call the tokens below pass: its chosen arm in place, then that arm's own cfgs)
    let mut select = false;
    let mut seen = false;
    let r = strip_toks(src, c, &mut select, &mut seen, externs);
    *env = Some(seen);
    if select {
        if let Some(e) = expand_select(r.as_deref().unwrap_or(src), c) {
            // (the chosen arm's `extern crate` items too: in place, while the expansion keeps the length)
            let mut more = vec![];
            let same = e.len() == src.len();
            let r2 = strip_toks(&e, c, &mut false, &mut false, &mut more).unwrap_or(e);
            if same {
                externs.extend(more);
                externs.sort_unstable();
            }
            return Some(r2);
        }
    }
    r
}

/// [`strip_noting`] for a caller that does not ask whether the source may call `env!`: nothing to read where
/// may_configure says so (a program whose `cfg` is a variable's name, and the tokens of its whole text read on
/// every edit)
pub fn strip_configured(src: &str, c: &CfgSet, externs: &mut Vec<(usize, usize)>) -> Option<String> {
    if !may_configure(src) {
        return None;
    }
    strip_noting(src, c, &mut None, externs)
}

/// Whether src may have what strip_toks rewrites: a `cfg` that a `(` or `!` follows (`#[cfg(..)]`, `cfg!(..)`, or
/// a comment between them), a word that starts with `cfg_` (`cfg_attr`, `cfg_select`), or an `extern` (`extern
/// crate`). Read in the text, so a comment or a string may say yes; never no where the tokens find one
fn may_configure(src: &str) -> bool {
    if src.contains("extern") {
        return true;
    }
    let b = src.as_bytes();
    let id = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    src.match_indices("cfg").any(|(i, _)| {
        if i > 0 && id(b[i - 1]) {
            return false;
        }
        let rest = &b[i + 3..];
        if rest.first().is_some_and(|&c| id(c)) {
            return rest.first() == Some(&b'_');
        }
        matches!(rest.iter().find(|c| !c.is_ascii_whitespace()), Some(b'(' | b'!' | b'/') | None)
    })
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
    outer_docs_start(s, if j > 0 { toks[j - 1].2 } else { 0 }, toks[j].1)
}

/// Before the byte p (an item or its first attribute), the outer doc comments (`///`, `/** */`) between it and the
/// token before it (which ends at `from`): they are attributes of that item and go with it (rustc: no E0585 for a doc
/// before a cfg'd-out item). The comments there are read forward, as the lexer reads them: they nest, and a `///` or
/// `/**` inside another comment, `/* outer\n/// nested */`, is text. Where the first doc comment starts, or p
pub(crate) fn outer_docs_start(s: &[u8], from: usize, p: usize) -> usize {
    let mut i = from.min(p);
    while i < p {
        if s[i..p].starts_with(b"//") {
            if s[i..].starts_with(b"///") && !s[i..].starts_with(b"////") {
                return i;
            }
            i += s[i..p].iter().position(|&c| c == b'\n').unwrap_or(p - i);
        } else if s[i..p].starts_with(b"/*") {
            if s[i..].starts_with(b"/**") && !s[i..].starts_with(b"/***") && !s[i..].starts_with(b"/**/") {
                return i;
            }
            let mut d = 0usize;
            while i < p {
                if s[i..].starts_with(b"/*") {
                    d += 1;
                    i += 2;
                } else if s[i..].starts_with(b"*/") {
                    d -= 1;
                    i += 2;
                    if d == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    p
}

fn strip_toks(src: &str, c: &CfgSet, select: &mut bool, env: &mut bool, externs: &mut Vec<(usize, usize)>) -> Option<String> {
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
        if t(i) == b"cfg" && t(i + 1) == b"!" && is_open(t(i + 2)) {
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
        // The insides of macro calls and macro_rules! are not looked at (evaluated after expansion: cfg_if!'s `if #[cfg(..)] { .. }`).
        // std's formatting macros (`println!("{}", cfg!(unix))`) are not expanded later, so their insides are evaluated too
        if toks[i].0 == 0 && t(i) != b"cfg" && t(i + 1) == b"!" && !format_like(t(i)) {
            *env |= matches!(t(i), b"env" | b"option_env");
            let g = if t(i + 2) == b"(" || t(i + 2) == b"[" || t(i + 2) == b"{" {
                Some(i + 2)
            // (`macro_rules! name { .. }`; not `if !done { .. }`, whose block is code: aho-corasick's `if !cfg!(..) {
            // debug!(..); }` keeps a `#[cfg(feature = "logging")]` from its expansion until it is stripped)
            } else if t(i) == b"macro_rules" && toks.get(i + 2).is_some_and(|x| x.0 == 0) && t(i + 3) == b"{" {
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
                externs.push((toks[i].1, toks[e].2));
                blank(&mut out, toks[i].1, toks[e].2);
                // (its visibility too: `pub extern crate alloc;`, `pub(crate) extern crate core;`. A path through it
                // (`rt::alloc::vec::Vec`) is the crate's path. `pub(in path)` stays: its path is checked)
                let renamed = e == i + 5 && t(i + 3) == b"as" && t(i + 4) != b"_" && !matches!(t(i + 2), b"std" | b"core" | b"alloc");
                if !renamed && i >= 1 {
                    let vs = if t(i - 1) == b"pub" {
                        Some(i - 1)
                    } else if i >= 4 && t(i - 1) == b")" && matches!(t(i - 2), b"crate" | b"self" | b"super") && t(i - 3) == b"(" && t(i - 4) == b"pub" {
                        Some(i - 4)
                    } else {
                        None
                    };
                    if let Some(vs) = vs {
                        blank(&mut out, toks[vs].1, toks[i].1);
                    }
                }
                // (`extern crate util as u;` names the crate `u` too: `use util as u;` in its place. `use` where `extern`
                // was, and `util as u;` where it was, so a line break in the item stays where it is)
                if renamed {
                    out[toks[i].1..toks[i].1 + 3].copy_from_slice(b"use");
                    let (a, b) = (toks[i + 2].1, toks[e].2);
                    out[a..b].copy_from_slice(&s[a..b]);
                }
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
                // (If `#![cfg(..)]` is false, the whole file. For now only the attribute is removed)
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
    fn outer_docs_are_comments_only() {
        fn start(src: &str) -> &str {
            let p = src.find("#[").unwrap();
            let toks = crate::lexer::tokenize(src.as_bytes());
            let from = toks.iter().take_while(|t| t.1 < p).last().map_or(0, |t| t.2);
            &src[outer_docs_start(src.as_bytes(), from, p)..p]
        }
        assert_eq!(start("fn a() {}\n/**\n * doc\n */\n#[x]\nfn f() {}"), "/**\n * doc\n */\n");
        // (blanks after `*/`, CRLF lines)
        assert_eq!(start("fn a() {}\n/**\n * doc\n */  \n#[x]\nfn f() {}"), "/**\n * doc\n */  \n");
        assert_eq!(start("fn a() {}\r\n/**\r\n * doc\r\n */\r\n/// more\r\n#[x]\r\nfn f() {}"), "/**\r\n * doc\r\n */\r\n/// more\r\n");
        // (a `/**` in a string is no comment's: the `*/` closes the `/*` nearest before it)
        // (comments nest: this `*/ */` closes `/* outer`)
        let nested = "fn a() {}\n/* outer\n/**\n*/ */\n#[cfg(any())]\nfn gone() {}\nfn main() {}\n";
        assert_eq!(start(nested), "");
        let nested1 = "fn a() {}\n/* outer\n/** nested */ */\n#[cfg(any())]\nfn gone() {}\nfn main() {}\n";
        assert_eq!(start(nested1), "");
        let o1 = strip(nested1, &CfgSet::default()).unwrap();
        assert!(o1.contains("/* outer") && o1.contains("/** nested */ */") && o1.contains("fn main() {}") && !o1.contains("gone"), "{o1}");
        // (a `///` in a block comment is text: `/* outer\n/// nested */`)
        let nested2 = "fn a() {}\n/* outer\n/// nested */\n#[cfg(any())]\nfn gone() {}\nfn main() {}\n";
        assert_eq!(start(nested2), "");
        let o2 = strip(nested2, &CfgSet::default()).unwrap();
        assert!(o2.contains("/* outer") && o2.contains("/// nested */") && o2.contains("fn main() {}") && !o2.contains("gone"), "{o2}");
        // (the item's docs with a plain comment after them, and a `///` that ends in `*/`)
        assert_eq!(start("fn a() {}\n/// doc */\n// plain\n#[x]\nfn f() {}"), "/// doc */\n// plain\n");
        // (a one-line doc comment is still the item's)
        assert_eq!(start("fn a() {}\n/** one */\n#[x]\nfn f() {}"), "/** one */\n");
        assert_eq!(start("fn a() {}\n/**/\n#[x]\nfn f() {}"), "");
        let o = strip(nested, &CfgSet::default()).unwrap();
        assert!(o.contains("/* outer") && o.contains("*/ */") && o.contains("fn main() {}") && !o.contains("gone"), "{o}");
        let src = "const K: &str = r#\"\n/**\n\"#;\nfn main() {}\n/* ordinary comment\n*/\n#[cfg(any())]\nfn removed() {}\n";
        assert_eq!(start(src), "");
        let o = strip(src, &CfgSet::default()).unwrap();
        assert!(o.contains("fn main() {}") && o.contains("\"#;") && !o.contains("removed"), "{o}");
    }

    #[test]
    fn items_fields_arms() {
        assert_eq!(run("#[cfg(feature = \"std\")] fn a() {} fn b() {}", &[]), "fn b() {}");
        assert_eq!(run("#[cfg(feature = \"std\")] fn a() {} fn b() {}", &["std"]), "fn a() {} fn b() {}");
        assert_eq!(run("struct S { #[cfg(test)] a: u8, b: u8 }", &[]), "struct S { b: u8 }");
        assert_eq!(run("enum E { #[cfg(not(unix))] A { x: u8 }, B }", &[]), "enum E { B }");
        assert_eq!(run("match x { #[cfg(any(test, doc))] 1 => {} _ => 2 }", &[]), "match x { _ => 2 }");
        // (an arm whose pattern has braces goes on to its body: serde_json, proc-macro2, der)
        assert_eq!(run("match x { #[cfg(test)] S::N { .. } => unreachable!(), _ => 2 }", &[]), "match x { _ => 2 }");
        assert_eq!(run("match x { #[cfg(test)] S { a, .. } => f(*a), _ => 2 }", &[]), "match x { _ => 2 }");
        assert_eq!(run("match x { #[cfg(test)] S { a } => { a } _ => 2 }", &[]), "match x { _ => 2 }");
        assert_eq!(run("match x { #[cfg(test)] S { a } | T { a } => a, _ => 2 }", &[]), "match x { _ => 2 }");
        assert_eq!(run("match x { #[cfg(test)] S { a } if a > 0 => a, _ => 2 }", &[]), "match x { _ => 2 }");
        assert_eq!(run("match x { #[cfg(test)] A => if a { 1 } else { 2 }, _ => 2 }", &[]), "match x { _ => 2 }");
        // (a block statement is not a pattern: the `if` after it stays)
        assert_eq!(run("fn f() { #[cfg(test)] { g(); } if a { h(); } i(); }", &[]), "fn f() { if a { h(); } i(); }");
        // (a type's generic arguments hold `,`s: a field's (anstyle-parse), a parameter's, a turbofish's (zerovec))
        assert_eq!(run("struct S { #[cfg(test)] a: A<u8, N>, b: u8 }", &[]), "struct S { b: u8 }");
        assert_eq!(run("struct S { #[cfg(test)] a: A<B<u8, C>, Box<dyn Fn(u8, u8) -> u8>>, b: u8 }", &[]), "struct S { b: u8 }");
        assert_eq!(run("struct S(#[cfg(test)] Map<K, V>, u8);", &[]), "struct S( u8);");
        assert_eq!(run("fn f(#[cfg(test)] a: Map<K, V>, b: u8) {}", &[]), "fn f( b: u8) {}");
        assert_eq!(run("fn f() { #[cfg(test)] g::<\n  A<u8, B<C, _>>,\n>(&x); h(); }", &[]), "fn f() { h(); }");
        assert_eq!(run("struct S { #[cfg(test)] a: A<{ N + 1 }, 2>, b: u8 }", &[]), "struct S { b: u8 }");
        // (a `<` that does not close is a comparison)
        assert_eq!(run("S { #[cfg(test)] a: b < c, d: 1 }", &[]), "S { d: 1 }");
        assert_eq!(run("[#[cfg(test)] a < b, c]", &[]), "[ c]");
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
        assert_eq!(run("extern crate util as u; extern crate geom as _; fn f() {}", &[]), "use util as u; fn f() {}");
        assert_eq!(run("pub extern crate alloc; pub(crate) extern crate core; pub extern crate util as u; fn f() {}", &[]), "pub use util as u; fn f() {}");
        // (a line break in the item stays on its line)
        let o = strip("extern\ncrate util as u;\n#[cfg(any())] fn f() {}", &CfgSet::default()).unwrap();
        assert_eq!(o.lines().map(str::trim).take(2).collect::<Vec<_>>(), ["use", "util as u;"]);
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

/// A string literal as an attribute takes it: `"a"`, `r"a"`, `r#"a"#` (not `b"a"`)
pub fn str_lit(v: &str) -> bool {
    let b = v.as_bytes();
    match v.strip_prefix('r') {
        // (a raw string ends with its quote and as many `#` as it opens with: `r#"reason"#`; it is the whole value,
        // not `r"a" + "b"`)
        Some(r) => {
            let h = r.len() - r.trim_start_matches('#').len();
            let body = &r[h..];
            body.len() >= 2 + h && body.starts_with('"') && body.ends_with(&format!("\"{}", "#".repeat(h))) && lit_end(b, 0) == Some(b.len())
        }
        // (its closing quote ends the value: `"a\"b"`, not `"a" + "b"` nor `"a\"`)
        None => {
            if b.len() < 2 || b[0] != b'"' {
                return false;
            }
            let mut k = 1;
            while k < b.len() && b[k] != b'"' {
                k += if b[k] == b'\\' { 2 } else { 1 };
            }
            k == b.len() - 1
        }
    }
}

/// Where a string literal, a char literal or a comment that starts at `i` ends (one past it), or None if none
/// starts there: `"a\\"b"`, `r#"a"b"#`, `br"a"`, `'c'`, `'\\''`, `'é'`, `// ..`, `/* /* .. */ */` (not a lifetime,
/// `'a`, nor an identifier ending in r, `for"`)
pub fn lit_end(b: &[u8], i: usize) -> Option<usize> {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    match b[i] {
        b'"' => {
            let mut k = i + 1;
            while k < b.len() && b[k] != b'"' {
                k += if b[k] == b'\\' { 2 } else { 1 };
            }
            Some((k + 1).min(b.len()))
        }
        // (`r".."`, `br".."`, `cr".."`)
        b'r' if !(i > 0 && ident(b[i - 1]) && b[i - 1] != b'b' && b[i - 1] != b'c') => {
            let h = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
            if b.get(i + 1 + h) != Some(&b'"') {
                return None;
            }
            let from = i + 2 + h;
            let mut k = from;
            while k < b.len() {
                if b[k] == b'"' && b.len() >= k + 1 + h && b[k + 1..k + 1 + h].iter().all(|&c| c == b'#') {
                    return Some(k + 1 + h);
                }
                k += 1;
            }
            Some(b.len())
        }
        // (an escape, to its closing quote; one the text ends in, to the end)
        b'\'' if b.get(i + 1) == Some(&b'\\') => {
            let n = b.get(i + 3..).and_then(|r| r.iter().position(|&c| c == b'\'')).map_or(b.len(), |n| i + 3 + n + 1);
            Some(n.min(b.len()))
        }
        b'\'' => {
            // (one char, of 1 to 4 bytes, then the closing quote)
            let c = *b.get(i + 1)?;
            let w = if c < 0x80 { 1 } else if c < 0xE0 { 2 } else if c < 0xF0 { 3 } else { 4 };
            (b.get(i + 1 + w) == Some(&b'\'')).then_some(i + 2 + w)
        }
        b'/' if b.get(i + 1) == Some(&b'/') => Some(b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n)),
        b'/' if b.get(i + 1) == Some(&b'*') => {
            let (mut k, mut d) = (i + 2, 1u32);
            while k < b.len() && d > 0 {
                if b[k..].starts_with(b"/*") {
                    d += 1;
                    k += 2;
                } else if b[k..].starts_with(b"*/") {
                    d -= 1;
                    k += 2;
                } else {
                    k += 1;
                }
            }
            Some(k.min(b.len()))
        }
        _ => None,
    }
}

/// A text with its comments as spaces (comments are whitespace, `all(/* note */)`); literals are kept as written
pub fn blank_comments(v: &str) -> std::borrow::Cow<'_, str> {
    if !v.contains("//") && !v.contains("/*") {
        return std::borrow::Cow::Borrowed(v);
    }
    let b = v.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        match lit_end(b, i) {
            Some(e) if b[i] == b'/' => {
                out.extend(std::iter::repeat_n(b' ', e - i));
                i = e;
            }
            Some(e) => {
                out.extend_from_slice(&b[i..e]);
                i = e;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    std::borrow::Cow::Owned(String::from_utf8(out).unwrap_or_else(|_| v.to_string()))
}

/// The value of a string literal as an attribute takes it (see `str_lit`), with a plain string's escapes decoded:
/// `"console"`, `r"console"` and `r#"console"#` are all `console`
pub fn str_value(v: &str) -> Option<String> {
    let v = v.trim();
    if !str_lit(v) {
        return None;
    }
    if let Some(r) = v.strip_prefix('r') {
        let h = r.len() - r.trim_start_matches('#').len();
        return Some(r[h + 1..r.len() - 1 - h].to_string());
    }
    let body = &v[1..v.len() - 1];
    if !body.contains('\\') {
        return Some(body.to_string());
    }
    let (mut out, mut cs) = (String::new(), body.chars());
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
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            // (`\x61`: two hex digits, up to `\x7F`)
            'x' => {
                let h: String = cs.by_ref().take(2).collect();
                let v = u8::from_str_radix(&h, 16).ok().filter(|v| v.is_ascii() && h.bytes().all(|c| c.is_ascii_hexdigit()))?;
                out.push(char::from(v));
            }
            // (`\u{1F600}`, `\u{1_F600}`: hex digits that may have `_` between them, not before the first, to the
            // closing brace, which must be there: not `\u{_61}` nor `\u{61`)
            'u' => {
                if cs.next()? != '{' {
                    return None;
                }
                let mut hex = String::new();
                loop {
                    match cs.next()? {
                        '}' => break,
                        '_' if !hex.is_empty() => {}
                        d if d.is_ascii_hexdigit() => hex.push(d),
                        _ => return None,
                    }
                }
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
            }
            // (a line continuation: the newline and the next line's leading whitespace)
            '\n' => {
                let rest: String = cs.clone().collect();
                let t = rest.trim_start();
                cs = body[body.len() - t.len()..].chars();
            }
            _ => return None,
        }
    }
    Some(out)
}

/// The entries of an attribute's list: split at the commas outside brackets, literals and comments, trimmed, without
/// the empty one a trailing comma leaves
pub fn attr_entries(v: &str) -> Vec<&str> {
    let (mut out, mut d, mut from, mut i) = (vec![], 0i32, 0, 0);
    let b = v.as_bytes();
    while i < b.len() {
        if let Some(e) = lit_end(b, i) {
            i = e;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => d += 1,
            b')' | b']' | b'}' => d -= 1,
            b',' if d == 0 => {
                out.push(v[from..i].trim());
                from = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(v[from..].trim());
    if out.last().is_some_and(|x| x.is_empty()) {
        out.pop();
    }
    out
}

/// A `cfg` predicate rustc can read: a name (`unix`, `r#true`), `name = "value"`, `all(..)` / `any(..)` of any
/// number, `not(..)` of one; `version(..)` and `target(..)` are left to their feature gates. Not a path, a literal or
/// another `name(..)`
pub fn cfg_pred(p: &str) -> bool {
    cfg_check(p).is_ok()
}

/// What an attribute's `name = value` holds: Ok(true) a string literal, Ok(false) another literal (`1`, `b"a"`,
/// `'c'`, `true`), Err(what rustc says it found) anything else: a macro call, a lone symbol (`` `%` ``), an expression
pub fn attr_value(v: &str) -> Result<bool, String> {
    let v = v.trim();
    if str_lit(v) {
        return Ok(true);
    }
    let lit = v.starts_with("b\"") || v.starts_with("b'") || v.starts_with('\'')
        || v.starts_with(|c: char| c.is_ascii_digit()) && v.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        || v == "true" || v == "false";
    if lit {
        return Ok(false);
    }
    let n = v.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(v.len());
    if n > 0 && v[n..].starts_with('!') && v[n + 1..].trim_start().starts_with(['(', '[', '{']) {
        return Err("macro call".to_string());
    }
    if v.chars().count() == 1 && !v.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return Err(format!("`{v}`"));
    }
    Err("expression".to_string())
}

/// The message rustc gives for an attribute value that is not a literal
pub fn not_a_literal(found: &str) -> String {
    format!("expected a literal (`1u8`, `1.0f32`, `\"string\"`, etc.) here, found {found}")
}

/// The first fn qualifier out of rustc's order `const` `async` `unsafe`/`safe` `extern "abi"` before a `fn` (of an
/// item, an associated fn or a fn pointer type), as rustc's parser reports it at that keyword: `unsafe async fn`
/// "expected one of `extern` or `fn`, found keyword `async`". The parser reads every item, a configured-away one
/// too, so this reads the source as written; not a macro's definition or input, a comment or a literal, and only a
/// run of qualifiers that ends at `fn`
pub fn fn_qualifier_order(src: &str) -> Option<String> {
    if !src.contains("fn") {
        return None;
    }
    let b = src.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let ws = |mut k: usize| {
        while k < b.len() && b[k].is_ascii_whitespace() {
            k += 1;
        }
        k
    };
    let word = |k: usize| &b[k..k + b[k..].iter().take_while(|&&c| ident(c)).count()];
    // (the rank of a qualifier: rustc's order)
    let rank = |w: &[u8]| match w {
        b"const" => Some(0),
        b"async" => Some(1),
        b"unsafe" | b"safe" => Some(2),
        b"extern" => Some(3),
        _ => None,
    };
    // (first a cheap look: a qualifier whose next word is one that may not follow it; the text is read in order only
    // where there is one)
    let next_word = |k: usize| {
        let a = ws(k);
        if a < b.len() && ident(b[a]) { word(a) } else { &b""[..] }
    };
    // (`const` is left out: only another `const` may not follow it, and `const const fn` is read as no item first)
    let suspect = ["async", "unsafe", "extern"].iter().any(|q| {
        src.match_indices(q).any(|(m, _)| {
            let e = m + q.len();
            if m > 0 && ident(b[m - 1]) || b.get(e).is_some_and(|&c| ident(c)) {
                return false;
            }
            let mut k = e;
            if *q == "extern" {
                let a = ws(k);
                if b.get(a) == Some(&b'"') {
                    k = lit_end(b, a).unwrap_or(b.len());
                }
            }
            let n = next_word(k);
            n != b"safe" && rank(n).is_some_and(|r| r <= rank(q.as_bytes()).unwrap_or(0))
        })
    });
    if !suspect {
        return None;
    }
    let mut i = 0;
    while i < b.len() {
        if let Some(e) = lit_end(b, i) {
            i = e;
            continue;
        }
        if !ident(b[i]) || i > 0 && ident(b[i - 1]) {
            i += 1;
            continue;
        }
        let w = word(i);
        let after = ws(i + w.len());
        // (a macro's definition or call: its group is not read, `m! { .. }`, `macro_rules! m { .. }`)
        if b.get(after) == Some(&b'!') && b.get(after + 1) != Some(&b'=') {
            let mut k = ws(after + 1);
            if k < b.len() && ident(b[k]) {
                k = ws(k + word(k).len());
            }
            if matches!(b.get(k), Some(b'{' | b'(' | b'[')) {
                let mut d = 0i32;
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
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
            }
            i = k.max(i + 1);
            continue;
        }
        // (a run of qualifiers from here; `*const` is a pointer's)
        let pointer = src[..i].trim_end().ends_with('*');
        if rank(w).is_none() || pointer {
            i += w.len();
            continue;
        }
        let (mut k, mut last, mut bad): (usize, i32, Option<(usize, &[u8], i32)>) = (i, -1, None);
        let ends_at_fn = loop {
            k = ws(k);
            if k >= b.len() || !ident(b[k]) {
                break false;
            }
            let q = word(k);
            if q == b"fn" {
                break true;
            }
            let Some(r) = rank(q) else { break false };
            if bad.is_none() && r <= last && q != b"safe" {
                bad = Some((k, q, last));
            }
            last = last.max(r);
            k += q.len();
            // (`extern "C"`: its ABI)
            if q == b"extern" {
                let a = ws(k);
                if b.get(a) == Some(&b'"') {
                    k = lit_end(b, a).unwrap_or(b.len());
                }
            }
        };
        if let (true, Some((at, q, prev))) = (ends_at_fn, bad) {
            let expected = match prev {
                0 => "one of `async`, `extern`, `fn`, `safe`, or `unsafe`",
                1 => "one of `extern`, `fn`, `safe`, or `unsafe`",
                2 => "one of `extern` or `fn`",
                _ => "`fn`",
            };
            let line = b[..at].iter().filter(|&&c| c == b'\n').count() + 1;
            let col = at - src[..at].rfind('\n').map_or(0, |p| p + 1) + 1;
            return Some(format!("{line}:{col}: expected {expected}, found keyword `{}`", String::from_utf8_lossy(q)));
        }
        i = k.max(i + 1);
    }
    None
}

/// Whether a predicate holds in a configuration: `unix`, `all(..)`, `not(feature = "x")`
pub fn cfg_holds(p: &str, c: &CfgSet) -> bool {
    let b = p.as_bytes();
    let toks = crate::lexer::tokenize(b);
    eval(b, &toks, 0, c).0
}

/// Whether rustc can read a `cfg` predicate: Ok, or Err with its own message ("" for "malformed `cfg` .. input")
pub fn cfg_check(p: &str) -> Result<(), String> {
    let p = p.trim();
    let raw = p.strip_prefix("r#").unwrap_or(p);
    let n = raw.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(raw.len());
    let (name, rest) = (&raw[..n], raw[n..].trim());
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(String::new());
    }
    if rest.is_empty() {
        return Ok(());
    }
    if let Some(v) = rest.strip_prefix('=') {
        // (a string literal with more after it, `"a" + "b"`: not one value)
        let t = v.trim();
        if t.starts_with(['"', 'r']) && lit_end(t.as_bytes(), 0).is_some_and(|e| e < t.len()) {
            return Err(String::new());
        }
        return match attr_value(v) {
            // (its escapes must decode: not `"\u{61"`)
            Ok(true) if str_value(v).is_some() => Ok(()),
            Ok(true) => Err(String::new()),
            Ok(false) => Err(String::new()),
            Err(found) => Err(not_a_literal(&found)),
        };
    }
    // (`target-os = ..`, `target_os?`: not a predicate's next token)
    if !rest.starts_with(['(', ',']) && !rest.starts_with("::") {
        let c = rest.chars().next().unwrap_or(' ');
        return Err(format!("expected one of `(`, `,`, `::`, or `=`, found `{c}`"));
    }
    let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else { return Err(String::new()) };
    match name {
        "all" | "any" => attr_entries(inner).iter().try_for_each(|x| cfg_check(x)),
        "not" => {
            let e = attr_entries(inner);
            if e.len() == 1 { cfg_check(e[0]) } else { Err(String::new()) }
        }
        "version" | "target" => Ok(()),
        _ => Err(String::new()),
    }
}

/// Past the whitespace and comments at `k` (comments are whitespace: `cfg /* note */ !(unix)`)
fn skip_trivia(b: &[u8], mut k: usize) -> usize {
    while k < b.len() {
        if b[k].is_ascii_whitespace() {
            k += 1;
        } else if b[k] == b'/' && matches!(b.get(k + 1), Some(b'/' | b'*')) {
            k = lit_end(b, k).unwrap_or(b.len());
        } else {
            break;
        }
    }
    k
}

/// A text past its leading whitespace and comments: ` /* safety */ no_mangle` is `no_mangle`
pub fn trim_trivia(v: &str) -> &str {
    &v[skip_trivia(v.as_bytes(), 0)..]
}

/// The first predicate rustc cannot read in a `cfg_attr`'s entries: its own, then, when that holds, one of a `cfg(..)`
/// or a nested `cfg_attr(..)` among the attributes it puts on the item (`cfg_attr(all(), cfg(not()))`), as the
/// attribute's name, rustc's own message ("" for malformed input) and the nested entry (None: the `cfg_attr` itself)
fn cfg_attr_check<'a>(entries: &[&'a str], c: &CfgSet) -> Result<(), (&'static str, String, Option<&'a str>)> {
    let Some(p) = entries.first() else { return Err(("cfg_attr", String::new(), None)) };
    cfg_check(p).map_err(|e| ("cfg_attr", e, None))?;
    if !cfg_holds(p, c) {
        return Ok(());
    }
    for &a in &entries[1..] {
        let (n, rest) = match a.strip_prefix("cfg_attr") {
            Some(r) => ("cfg_attr", r),
            None => match a.strip_prefix("cfg") {
                Some(r) => ("cfg", r),
                None => continue,
            },
        };
        let Some(inner) = rest.trim_start().strip_prefix('(').and_then(|r| r.strip_suffix(')')) else { continue };
        let e = attr_entries(inner);
        match n {
            "cfg_attr" => cfg_attr_check(&e, c).map_err(|(n, m, at)| (n, m, at.or(Some(a))))?,
            _ if e.len() == 1 => cfg_check(e[0]).map_err(|m| ("cfg", m, Some(a)))?,
            _ => return Err(("cfg", String::new(), Some(a))),
        }
    }
    Ok(())
}

/// Where a false `#![cfg(..)]` among a crate's leading inner attributes ends: rustc reads none of the crate after it
/// (`#![cfg(any())]`)
fn crate_cfg_off(src: &str, c: &CfgSet) -> Option<usize> {
    let b = src.as_bytes();
    let mut i = 0;
    // (a shebang line, `#!/usr/bin/env ..`, not an inner attribute)
    if src.starts_with("#!") && !trim_trivia(&src[2..]).starts_with('[') {
        i = src.find('\n').unwrap_or(b.len());
    }
    loop {
        i = skip_trivia(b, i);
        if !b[i..].starts_with(b"#!") {
            return None;
        }
        let o = skip_trivia(b, i + 2);
        if b.get(o) != Some(&b'[') {
            return None;
        }
        // (the matching `]`, past literals and comments)
        let (mut d, mut k, mut close) = (0i32, o, None);
        while k < b.len() {
            if let Some(e) = lit_end(b, k) {
                k = e;
                continue;
            }
            match b[k] {
                b'(' | b'[' | b'{' => d += 1,
                b')' | b']' | b'}' => {
                    d -= 1;
                    if d == 0 {
                        close = Some(k);
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        let close = close?;
        let body = blank_comments(&src[o + 1..close]);
        let pred = body.trim().strip_prefix("cfg").map(str::trim_start).and_then(|r| r.strip_prefix('(')).and_then(|r| r.strip_suffix(')'));
        if pred.is_some_and(|p| cfg_check(p).is_ok() && !cfg_holds(p, c)) {
            return Some(close + 1);
        }
        i = close + 1;
    }
}

/// The names of `cfg` predicates behind a feature gate (rustc 1.100's GATED_CFGS), with the feature
const GATED_CFGS: &[(&str, &str)] = &[
    ("overflow_checks", "cfg_overflow_checks"),
    ("ub_checks", "cfg_ub_checks"),
    ("contract_checks", "cfg_contract_checks"),
    ("target_thread_local", "cfg_target_thread_local"),
    ("target_has_atomic_equal_alignment", "cfg_target_has_atomic_equal_alignment"),
    ("target_has_atomic_load_store", "cfg_target_has_atomic"),
    ("sanitize", "cfg_sanitize"),
    ("version", "cfg_version"),
    ("relocation_model", "cfg_relocation_model"),
    ("sanitizer_cfi_generalize_pointers", "cfg_sanitizer_cfi"),
    ("sanitizer_cfi_normalize_integers", "cfg_sanitizer_cfi"),
    ("fmt_debug", "fmt_debug"),
    ("emscripten_wasm_eh", "cfg_emscripten_wasm_eh"),
    ("target_has_reliable_f16", "cfg_target_has_reliable_f16_f128"),
    ("target_has_reliable_f16_math", "cfg_target_has_reliable_f16_f128"),
    ("target_has_reliable_f128", "cfg_target_has_reliable_f16_f128"),
    ("target_has_reliable_f128_math", "cfg_target_has_reliable_f16_f128"),
    ("target_object_format", "cfg_target_object_format"),
    ("target_has_threads", "cfg_target_has_threads"),
];

/// The first name of a predicate behind a feature gate the crate does not enable (`#![feature(..)]` in `src`): its
/// offset in `p` and rustc's message. `target(..)` is the compact form, `cfg_target_compact`'s
fn gated_cfg(p: &str, src: &str) -> Option<(usize, String)> {
    let b = p.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let tk = |k: usize| &p[toks[k].1..toks[k].2];
    let enabled = |f: &str| {
        src.match_indices("#![feature(").any(|(k, _)| src[k + 11..].split(')').next().is_some_and(|l| l.split(',').any(|x| x.trim() == f)))
    };
    for k in 0..toks.len() {
        if toks[k].0 != 0 || (k > 0 && tk(k - 1) == "=") {
            continue;
        }
        let name = tk(k);
        let call = k + 1 < toks.len() && tk(k + 1) == "(";
        if name == "target" && call && !enabled("cfg_target_compact") {
            return Some((toks[k].1, "compact `cfg(target(..))` is experimental and subject to change".into()));
        }
        if let Some((_, f)) = GATED_CFGS.iter().find(|g| g.0 == name) {
            if !enabled(f) {
                return Some((toks[k].1, format!("`cfg({name})` is experimental and subject to change")));
            }
        }
    }
    None
}

/// The first `#[cfg(..)]`, `#[cfg_attr(..)]` or `cfg!(..)` of a source whose predicate rustc cannot read (E0539 /
/// E0565 / E0805): not one in a comment or a string on its line, nor in a macro's pattern (`$`). `kept` is the text
/// configured by `c` (cfg::strip, in place): rustc reads none of what a false `cfg` takes away, so one in an item,
/// statement or block it removes is not reported, nor an attribute after a false `cfg` of the same item, nor what a
/// false `#![cfg(..)]` of the crate takes away; nor a `cfg!` that `stringify!` only quotes
pub fn malformed_cfg(src: &str, kept: Option<&str>, c: &CfgSet) -> Option<String> {
    if !src.contains("cfg") {
        return None;
    }
    let b = src.as_bytes();
    let mut at = 0;
    let crate_off = if src.contains("#!") { crate_cfg_off(src, c) } else { None };
    // (how far the source is read as code, and the block comments open there: a candidate in a comment, a string or
    // a char literal is skipped, `/* #[cfg(not())] */`)
    let (mut cur, mut depth) = (0usize, 0u32);
    // (the open brackets of the code read so far: the innermost one around a candidate; and whether each opens a
    // `stringify!`, whose tokens are text, `stringify!(cfg!(not()))`, with how many of those are open)
    let mut open: Vec<(usize, bool)> = vec![];
    let mut quoting = 0u32;
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut in_code = |i: usize| -> (bool, Option<usize>, bool) {
        while cur < i {
            if depth > 0 {
                if b[cur..].starts_with(b"/*") {
                    depth += 1;
                    cur += 2;
                } else if b[cur..].starts_with(b"*/") {
                    depth -= 1;
                    cur += 2;
                } else {
                    cur += 1;
                }
                continue;
            }
            match b[cur] {
                b'/' if b.get(cur + 1) == Some(&b'/') => cur += b[cur..].iter().position(|&c| c == b'\n').unwrap_or(b.len() - cur),
                b'/' if b.get(cur + 1) == Some(&b'*') => {
                    depth = 1;
                    cur += 2;
                }
                b'"' => {
                    cur += 1;
                    while cur < b.len() && b[cur] != b'"' {
                        cur += if b[cur] == b'\\' { 2 } else { 1 };
                    }
                    cur += 1;
                }
                // (a raw string, `r#".."#` / `br".."` / `cr".."`, not an identifier ending in r)
                b'r' if !(cur > 0 && ident(b[cur - 1]) && b[cur - 1] != b'b' && b[cur - 1] != b'c') && {
                    let h = b[cur + 1..].iter().take_while(|&&c| c == b'#').count();
                    b.get(cur + 1 + h) == Some(&b'"')
                } => {
                    let h = b[cur + 1..].iter().take_while(|&&c| c == b'#').count();
                    let close = [b"\"".as_slice(), &vec![b'#'; h]].concat();
                    let from = cur + 2 + h;
                    cur = b[from..].windows(close.len()).position(|w| w == close.as_slice()).map_or(b.len(), |n| from + n + close.len());
                }
                // (a char literal, not a lifetime)
                b'\'' if b.get(cur + 1) == Some(&b'\\') => cur += 2 + b[cur + 2..].iter().position(|&c| c == b'\'').map_or(b.len() - cur - 2, |n| n + 1),
                b'\'' if b.get(cur + 2) == Some(&b'\'') => cur += 3,
                b'{' | b'(' | b'[' => {
                    let q = src[..cur].trim_end().strip_suffix('!').map(str::trim_end).and_then(|t| t.strip_suffix("stringify"));
                    let q = q.is_some_and(|t| !t.ends_with(|c: char| c.is_alphanumeric() || c == '_'));
                    quoting += q as u32;
                    open.push((cur, q));
                    cur += 1;
                }
                b'}' | b')' | b']' => {
                    if let Some((_, true)) = open.pop() {
                        quoting -= 1;
                    }
                    cur += 1;
                }
                _ => cur += 1,
            }
        }
        (depth == 0 && cur == i, open.last().map(|o| o.0), quoting > 0)
    };
    // (what the configuring took away: a blank in the configured text where the source has code)
    let kept = kept.filter(|k| k.len() == src.len()).map(str::as_bytes);
    let gone = |i: usize| kept.is_some_and(|k| k[i].is_ascii_whitespace() && !b[i].is_ascii_whitespace());
    // (an attribute after a `cfg` of the same item that does not hold: `#[cfg(any())] #[cfg(not())] fn f() {}`, read
    // with comments as whitespace, `#[cfg(any())] /* note */ #[cfg(not())]`, doc comments too, which are attributes of
    // the same item)
    let mut blanked: Option<std::borrow::Cow<'_, str>> = None;
    let mut after_false = |start: usize| -> bool {
        let src = &**blanked.get_or_insert_with(|| blank_comments(src));
        let b = src.as_bytes();
        let mut p = start;
        loop {
            let q = src[..p].trim_end().len();
            if q == 0 || b[q - 1] != b']' {
                return false;
            }
            let (mut d, mut j, mut o) = (0i32, q - 1, None);
            loop {
                match b[j] {
                    b']' => d += 1,
                    b'[' => {
                        d -= 1;
                        if d == 0 {
                            o = Some(j);
                            break;
                        }
                    }
                    _ => {}
                }
                if j == 0 {
                    break;
                }
                j -= 1;
            }
            let Some(o) = o else { return false };
            // (`#[..]`, or an inner `#![..]` before it)
            let hash = match o.checked_sub(1).map(|h| b[h]) {
                Some(b'#') => o - 1,
                Some(b'!') if o >= 2 && b[o - 2] == b'#' => o - 2,
                _ => return false,
            };
            let body = src[o + 1..q - 1].trim();
            let pred = body.strip_prefix("cfg").map(str::trim_start).and_then(|r| r.strip_prefix('(')).and_then(|r| r.strip_suffix(')'));
            if pred.is_some_and(|p| cfg_check(p).is_ok() && !cfg_holds(p, c)) {
                return true;
            }
            p = hash;
        }
    };
    // (the `macro_rules!` bodies, found once and only when a candidate is in code: each from its delimiter to the
    // matching one, a nested definition inside the one before it)
    let mut macro_defs: Option<Vec<(usize, usize)>> = None;
    let mut in_macro_def = |i: usize| -> bool {
        let defs = macro_defs.get_or_insert_with(|| {
            let mut defs: Vec<(usize, usize)> = vec![];
            // (`macro_rules!` / `macro_rules !` in code: not in a comment or a literal)
            let mut m = 0;
            while m < b.len() {
                if let Some(e) = lit_end(b, m) {
                    m = e;
                    continue;
                }
                let word = b[m..].starts_with(b"macro_rules") && !(m > 0 && ident(b[m - 1])) && !b.get(m + 11).is_some_and(|&c| ident(c));
                let bang = word && b[m + 11..].iter().find(|c| !c.is_ascii_whitespace()) == Some(&b'!');
                if !bang {
                    m += 1;
                    continue;
                }
                // (its delimiter, past the name and comments: `macro_rules! m /* {} */ { .. }`)
                let mut o = m + 11;
                while o < b.len() && !matches!(b[o], b'{' | b'(' | b'[') {
                    o = match lit_end(b, o) {
                        Some(e) if b[o] == b'/' => e,
                        _ => o + 1,
                    };
                }
                if o >= b.len() {
                    break;
                }
                let (mut d, mut k) = (0i32, o);
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
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                defs.push((o, k));
                // (past the body: a definition inside it is inside this one)
                m = k + 1;
            }
            defs
        });
        defs.iter().any(|&(o, e)| o < i && i < e)
    };
    while let Some(f) = src[at..].find("cfg") {
        let i = at + f;
        at = i + 3;
        if i > 0 && ident(b[i - 1]) {
            continue;
        }
        // (`#[cfg(` / `#![cfg_attr(` / `cfg!(`, `cfg ! [`, `cfg!{`: tokens, with whitespace and comments between)
        let name = if src[i..].starts_with("cfg_attr") { "cfg_attr" } else { "cfg" };
        let after = skip_trivia(b, i + name.len());
        let macro_open = (name == "cfg" && b.get(after) == Some(&b'!')).then(|| skip_trivia(b, after + 1));
        let macro_open = macro_open.filter(|&k| matches!(b.get(k), Some(b'(' | b'[' | b'{')));
        let macro_call = macro_open.is_some();
        let attr = !macro_call && (src[..i].ends_with("#[") || src[..i].ends_with("#!["));
        if !attr && !macro_call {
            continue;
        }
        let open = macro_open.unwrap_or(after);
        if !macro_call && b.get(open) != Some(&b'(') {
            continue;
        }
        // (where the attribute or the macro starts)
        let start = if macro_call { i } else { src[..i].rfind('#').unwrap_or(i) };
        // (what a false `#![cfg(..)]` of the crate takes away)
        if crate_off.is_some_and(|e| start >= e) {
            break;
        }
        let (code, around, quoted) = in_code(start);
        if !code || quoted {
            continue;
        }
        // (in what the configuring takes away: an item, a statement or a block of a false `cfg`, or after one)
        if around.is_some_and(gone) || macro_call && gone(start) || !macro_call && kept.is_some() && after_false(start) {
            continue;
        }
        // (not one in a `macro_rules!` body: a pattern's tokens, `(cfg!(not())) => {}`, are checked when expanded)
        if in_macro_def(start) {
            continue;
        }
        // (the matching delimiter, past literals and comments)
        let (mut d, mut k, mut close) = (0i32, open, None);
        while k < b.len() {
            if let Some(e) = lit_end(b, k) {
                k = e;
                continue;
            }
            match b[k] {
                b'(' | b'[' | b'{' => d += 1,
                b')' | b']' | b'}' => {
                    d -= 1;
                    if d == 0 {
                        close = Some(k);
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        let Some(close) = close else { continue };
        // (comments in the predicate are whitespace: `all(/* note */)`)
        let inner = blank_comments(&src[open + 1..close]);
        // (a macro's metavariable, `cfg!($p)`, is checked when expanded: a `$` in the code, not one in a literal or a
        // comment, `feature = "$"`)
        if inner.contains('$') && {
            let ib = inner.as_bytes();
            let mut k = 0;
            loop {
                if k >= ib.len() {
                    break false;
                }
                match lit_end(ib, k) {
                    Some(e) => k = e,
                    None if ib[k] == b'$' => break true,
                    None => k += 1,
                }
            }
        } {
            continue;
        }
        let entries = attr_entries(&inner);
        // (E0658: a predicate of a name behind a feature gate, `cfg(overflow_checks)`, at the name; read where rustc
        // reads the predicate, the first entry: a `cfg_attr`'s attributes are not predicates)
        if let Some(&p) = entries.first().filter(|_| cfg_check(entries[0]).is_ok()) {
            if let Some((off, msg)) = gated_cfg(p, src) {
                let pos = open + 1 + (p.as_ptr() as usize - inner.as_ptr() as usize) + off;
                let line = src[..pos].matches('\n').count() + 1;
                let col = pos - src[..pos].rfind('\n').map_or(0, |n| n + 1) + 1;
                return Some(format!("{line}:{col}: {msg} (E0658)"));
            }
        }
        let checked = match name {
            "cfg_attr" => cfg_attr_check(&entries, c),
            // (one pattern, with or without a trailing comma: `cfg!(unix,)`)
            _ if entries.len() == 1 => cfg_check(entries[0]).map_err(|e| ("cfg", e, None)),
            _ => Err(("cfg", String::new(), None)),
        };
        if let Err((name, own, nested)) = checked {
            // (the line of the `cfg` / `cfg_attr`, or of the nested one at fault)
            let pos = nested.map_or(i, |a| open + 1 + (a.as_ptr() as usize - inner.as_ptr() as usize));
            let line = src[..pos].matches('\n').count() + 1;
            if !own.is_empty() {
                return Some(format!("{line}:1: {own}"));
            }
            let what = match (macro_call, entries.len()) {
                (true, 0) => "macro requires a cfg-pattern as an argument".to_string(),
                (true, 1) => "malformed `cfg` macro input".to_string(),
                (true, _) => "expected 1 cfg-pattern".to_string(),
                _ => format!("malformed `{name}` attribute input"),
            };
            return Some(format!("{line}:1: {what}"));
        }
    }
    None
}
