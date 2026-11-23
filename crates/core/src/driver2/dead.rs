//! `#![deny(dead_code)]` below the item level, in a binary: the first field of a struct (union) of the program's that
//! something constructs but nothing reads, the first variant of an enum nothing constructs, the first const or fn of
//! an inherent impl nothing names.
//!
//! Read from the tokens, conservatively: a struct a pattern names (or a macro other than std's expression ones does,
//! or an update `..base` fills), one with a derive other than Debug / Clone / Copy, a `#[repr]`, in a module, or a
//! `pub` field, is skipped; so is an enum a derive other than the comparisons, Debug, Clone, Copy and Default is on.

use crate::fxhash::{FxMap, FxSet};
use crate::lexer::{ItemKind, SrcItem};

/// The first dead part of the program's items, as `(line, message)`
pub(super) fn first_dead_part(src: &str, items: &[SrcItem]) -> Option<(usize, String)> {
    let (Some(first), Some(last)) = (items.first(), items.last()) else { return None };
    let base = first.start;
    let region = src.get(base..last.end)?;
    let b = region.as_bytes();
    let toks = crate::lexer::tokenize(b);
    let n = toks.len();
    let word = |k: usize| -> &str { if k < n && toks[k].0 == 0 { &region[toks[k].1..toks[k].2] } else { "" } };
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && b[toks[k].1] == c;
    // (each token's innermost enclosing open bracket, each open bracket's close)
    let none = usize::MAX;
    let mut encl = vec![none; n];
    let mut close = vec![none; n];
    let mut open_of = vec![none; n];
    let mut stack: Vec<usize> = vec![];
    for k in 0..n {
        encl[k] = stack.last().copied().unwrap_or(none);
        if toks[k].0 == 1 {
            match b[toks[k].1] {
                b'(' | b'[' | b'{' => stack.push(k),
                b')' | b']' | b'}' => {
                    if let Some(o) = stack.pop() {
                        close[o] = k;
                        open_of[k] = o;
                        encl[k] = stack.last().copied().unwrap_or(none);
                    }
                }
                _ => {}
            }
        }
    }
    // (the item each token is in)
    let mut item_of = vec![none; n];
    let mut first_tok = vec![none; items.len()];
    let mut i = 0;
    for k in 0..n {
        let at = base + toks[k].1;
        while i < items.len() && items[i].end <= at {
            i += 1;
        }
        if i < items.len() && items[i].start <= at {
            item_of[k] = i;
            if first_tok[i] == none {
                first_tok[i] = k;
            }
        }
    }
    let line_of = |k: usize| -> usize {
        let it = &items[item_of[k]];
        it.line + src[it.start..base + toks[k].1].matches('\n').count()
    };
    // (an item's keyword and name, after its attributes and visibility: the token index of the keyword)
    let head = |ii: usize| -> Option<usize> {
        let it = &items[ii];
        let mut k = first_tok[ii];
        if k == none {
            return None;
        }
        while k < n && item_of[k] == ii {
            if sym(k, b'#') {
                // (an attribute: past its brackets)
                let o = if sym(k + 1, b'!') { k + 2 } else { k + 1 };
                if !sym(o, b'[') || close[o] == none {
                    return None;
                }
                k = close[o] + 1;
                continue;
            }
            match word(k) {
                "pub" => {
                    k += 1;
                    if sym(k, b'(') && close[k] != none {
                        k = close[k] + 1;
                    }
                }
                "unsafe" | "async" | "extern" | "default" => k += 1,
                "const" if word(k + 1) == "fn" || word(k + 1) == "unsafe" || word(k + 1) == "async" => k += 1,
                _ if toks[k].0 == 2 => k += 1,
                _ => break,
            }
        }
        (k < n && item_of[k] == ii && base + toks[k].1 < it.end).then_some(k)
    };
    let attrs_of = |ii: usize| -> &str {
        let it = &items[ii];
        match head(ii) {
            Some(k) => &src[it.start..base + toks[k].1],
            None => &src[it.start..it.end],
        }
    };
    // (a lint level of its own (allow / expect / warn, spaced or not), a cfg, a repr, an export: left alone)
    let skip_attr = |a: &str| {
        let a: String = a.chars().filter(|c| !c.is_whitespace()).collect();
        ["allow(", "expect(", "warn(", "#[cfg", "repr(", "no_mangle", "export_name", "#[test]"].iter().any(|x| a.contains(x))
    };
    let derives = |a: &str| -> Vec<String> {
        let mut out = vec![];
        let mut rest = a;
        while let Some(p) = rest.find("derive(") {
            rest = &rest[p + 7..];
            let e = rest.find(')').unwrap_or(rest.len());
            out.extend(rest[..e].split(',').map(|d| d.trim().rsplit("::").next().unwrap_or("").to_string()).filter(|d| !d.is_empty()));
        }
        out
    };
    // (std's macros that take expressions: an occurrence in one of them is what it is outside; in any other, a
    // pattern, maybe)
    let expr_macro = |m: &str| {
        matches!(
            m,
            "vec" | "println" | "print" | "eprintln" | "eprint" | "format" | "write" | "writeln" | "panic" | "assert" | "assert_eq" | "assert_ne" | "debug_assert" | "debug_assert_eq" | "debug_assert_ne" | "dbg" | "format_args"
        )
    };
    let keyword = |w: &str| matches!(w, "else" | "loop" | "unsafe" | "move" | "async" | "const" | "match" | "if" | "while" | "in" | "return" | "break" | "try");
    // (whether the occurrence that starts at token ps (its path) and ends at e (its brackets) is, or may be, in a
    // pattern: after `let` / `for` / `|` / `@`, in a macro other than std's expression ones, or followed (out through
    // the brackets around it) by `=` / `=>` / `|` / `:` / `in` / `if` before anything that ends an expression)
    let pattern_like = |ps: usize, e: usize| -> bool {
        let mut o = encl[ps];
        while o != none {
            if o >= 2 && sym(o - 1, b'!') && !expr_macro(word(o - 2)) {
                return true;
            }
            o = encl[o];
        }
        let p = ps.wrapping_sub(1);
        if matches!(word(p), "let" | "for") || sym(p, b'|') || sym(p, b'@') {
            return true;
        }
        // (after `=>` (an arm's body), a single `=` (a value), `return` / `break`: an expression)
        let arrow = p != none && p > 0 && sym(p, b'>') && sym(p - 1, b'=');
        let assign = p != none && sym(p, b'=') && !(p > 0 && toks[p - 1].0 == 1 && toks[p - 1].2 == toks[p].1 && b"=<>!+-*/%^&|".contains(&b[toks[p - 1].1]));
        if arrow || assign || matches!(word(p), "return" | "break") {
            return false;
        }
        let mut d = 0i32;
        let mut j = e + 1;
        while j < n {
            if toks[j].0 == 0 {
                match word(j) {
                    "fn" | "struct" | "enum" | "union" | "impl" | "trait" | "mod" => return false,
                    "in" | "if" if d == 0 => return true,
                    "let" | "return" | "use" | "static" | "type" if d == 0 => return false,
                    _ => {}
                }
            } else if toks[j].0 == 1 {
                match b[toks[j].1] {
                    b'(' | b'[' => d += 1,
                    // (a struct's braces go on; a block's start ends the expression)
                    // (right after it, with its own brackets in e: a block, `match E::A {` / `if x == S {`)
                    b'{' if d == 0 && j == e + 1 => return false,
                    b'{' if d == 0 => {
                        let prev = word(j - 1);
                        if prev.is_empty() || keyword(prev) {
                            return false;
                        }
                        d += 1;
                    }
                    b'{' => d += 1,
                    // (the end of a block around it (an arm's `{ .. }`, a fn's body): the expression ended; the end
                    // of a struct's braces (`Outer { s: S { f } }`): go on)
                    b'}' if d == 0 => {
                        let o = open_of[j];
                        if o == none || o == 0 || word(o - 1).is_empty() || keyword(word(o - 1)) {
                            return false;
                        }
                    }
                    b')' | b']' | b'}' => d = (d - 1).max(0),
                    b';' if d == 0 => return false,
                    b':' if d == 0 && !sym(j + 1, b':') && !sym(j - 1, b':') => return true,
                    b'|' if d == 0 => return true,
                    b'=' if d == 0 => {
                        if sym(j + 1, b'>') {
                            return true;
                        }
                        if sym(j + 1, b'=') {
                            j += 2;
                            continue;
                        }
                        let glued = toks[j - 1].0 == 1 && toks[j - 1].2 == toks[j].1 && b"=<>!+-*/%^&|".contains(&b[toks[j - 1].1]);
                        if !glued {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        true
    };
    // (the path an occurrence at k is the last segment of: back over `a::`)
    let path_start = |k: usize| -> usize {
        let mut ps = k;
        while ps >= 3 && sym(ps - 1, b':') && sym(ps - 2, b':') && toks[ps - 3].0 == 0 {
            ps -= 3;
        }
        ps
    };
    // (each inherent / trait impl's self type, by its header's index)
    let impl_self = |h: usize| -> Option<&str> {
        let it = items.get(h)?;
        let t = crate::lexer::tokenize(src.get(it.start..it.end)?.as_bytes());
        let text = &src[it.start..it.end];
        let tw = |k: usize| -> &str { if k < t.len() && t[k].0 == 0 { &text[t[k].1..t[k].2] } else { "" } };
        let ts = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && text.as_bytes()[t[k].1] == c;
        let mut k = t.iter().position(|x| x.0 == 0 && &text[x.1..x.2] == "impl")? + 1;
        // (past the impl's generics)
        let mut angle = 0;
        let mut after = None;
        while k < t.len() {
            if ts(k, b'<') {
                angle += 1;
            } else if ts(k, b'>') && !ts(k.wrapping_sub(1), b'-') {
                angle -= 1;
            } else if angle == 0 && tw(k) == "for" {
                after = Some(k + 1);
            } else if angle == 0 && (ts(k, b'{') || tw(k) == "where") {
                break;
            }
            k += 1;
        }
        let end = k;
        let mut k = after.unwrap_or_else(|| {
            let mut k = t.iter().position(|x| x.0 == 0 && &text[x.1..x.2] == "impl").unwrap_or(0) + 1;
            if ts(k, b'<') {
                let mut d = 0;
                while k < t.len() {
                    if ts(k, b'<') {
                        d += 1;
                    } else if ts(k, b'>') && !ts(k - 1, b'-') {
                        d -= 1;
                        if d == 0 {
                            k += 1;
                            break;
                        }
                    }
                    k += 1;
                }
            }
            k
        });
        // (the last segment of the self type's path, before its generics)
        let mut name = None;
        while k < end {
            if t[k].0 == 0 && !matches!(tw(k), "mut" | "dyn" | "const") {
                name = Some(tw(k));
            } else if ts(k, b'<') || ts(k, b'{') {
                break;
            }
            k += 1;
        }
        name
    };
    let mut self_of: FxMap<usize, Option<&str>> = Default::default();
    // (the struct or enum each occurrence names: its own name, or `Self` in its impl)
    let mut names_at = |k: usize, name: &str| -> bool {
        let w = word(k);
        if w == name {
            return true;
        }
        if w != "Self" || item_of[k] == none {
            return false;
        }
        let ItemKind::ImplMember(h) = items[item_of[k]].kind else { return false };
        *self_of.entry(h as usize).or_insert_with(|| impl_self(h as usize)) == Some(name)
    };
    // (the program's structs and unions: what `let x = S(..)` can name, not a fn `let x = make(..)`)
    let struct_names: FxSet<&str> = (0..items.len())
        .filter(|&ii| matches!(items[ii].kind, ItemKind::Normal))
        .filter_map(|ii| head(ii).filter(|&k| matches!(word(k), "struct" | "union")).map(|k| word(k + 1)))
        .collect();
    // (the struct a variable at k is bound to: the item's only `let` of that name, `let [mut] x = S(` / `S {`)
    let bound_to = |k: usize| -> Option<&str> {
        let ii = item_of[k];
        if ii == none {
            return None;
        }
        let v = word(k);
        let mut found = None;
        let mut q = first_tok[ii];
        while q < n && item_of[q] == ii {
            if word(q) == "let" {
                let x = if word(q + 1) == "mut" { q + 2 } else { q + 1 };
                if word(x) == v {
                    if found.is_some() {
                        return None;
                    }
                    let s = x + 2;
                    found = Some((sym(x + 1, b'=') && toks.get(s).is_some_and(|t| t.0 == 0) && (sym(s + 1, b'(') || sym(s + 1, b'{')) && struct_names.contains(word(s))).then(|| word(s)));
                }
            }
            q += 1;
        }
        found.flatten()
    };
    // (the field names and numbers something reads: after a `.`; `self.f` in an impl reads its self type's)
    let mut read: FxSet<String> = Default::default();
    let mut read_by: FxSet<(String, String)> = Default::default();
    for k in 0..n {
        if !sym(k, b'.') {
            continue;
        }
        let f = if toks.get(k + 1).is_some_and(|t| t.0 == 0) {
            word(k + 1).to_string()
        } else if toks.get(k + 1).is_some_and(|t| t.0 == 1 && b[t.1].is_ascii_digit() && t.1 == toks[k].2) {
            let mut j = k + 1;
            let mut s = String::new();
            while j < n && toks[j].0 == 1 && b[toks[j].1].is_ascii_digit() && toks[j].1 == toks[j - 1].2 {
                s.push(b[toks[j].1] as char);
                j += 1;
            }
            s
        } else {
            continue;
        };
        let owner = (k > 0 && word(k - 1) == "self" && !sym(k.wrapping_sub(2), b'.') && item_of[k] != none)
            .then(|| match items[item_of[k]].kind {
                ItemKind::ImplMember(h) => impl_self(h as usize),
                _ => None,
            })
            .flatten()
            // (`x.f` of an `x` the same item binds once, to a struct's value: `let x = S(..)` / `let x = S { .. }`)
            .or_else(|| (k > 0 && toks[k - 1].0 == 0 && !sym(k.wrapping_sub(2), b'.')).then(|| bound_to(k - 1)).flatten());
        match owner {
            Some(o) => {
                read_by.insert((o.to_string(), f));
            }
            None => {
                read.insert(f);
            }
        }
    }
    let is_read = |owner: &str, f: &str| read.contains(f) || read_by.contains(&(owner.to_string(), f.to_string()));
    // (uses of each name: every occurrence but a definition's)
    let mut uses: FxMap<&str, usize> = Default::default();
    for k in 0..n {
        if toks[k].0 == 0 && !(k > 0 && matches!(word(k - 1), "fn" | "struct" | "enum" | "const" | "static" | "trait" | "union" | "type") && !sym(k.wrapping_sub(2), b'*')) {
            *uses.entry(word(k)).or_default() += 1;
        }
    }
    // (a `use`, or a type's definition: nothing in it constructs or matches)
    let use_items: Vec<bool> = (0..items.len())
        .map(|ii| matches!(items[ii].kind, ItemKind::Normal) && head(ii).is_some_and(|k| matches!(word(k), "use" | "struct" | "enum" | "union" | "type")))
        .collect();
    let is_use_item = |ii: usize| use_items[ii];
    let list = |names: &[String], what: &str, verb: &str| -> String {
        let q: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
        match q.len() {
            1 => format!("{what} {} is never {verb}", q[0]),
            2 => format!("{what}s {} and {} are never {verb}", q[0], q[1]),
            _ => format!("{what}s {}, and {} are never {verb}", q[..q.len() - 1].join(", "), q[q.len() - 1]),
        }
    };
    let unit_structs: FxSet<&str> = (0..items.len())
        .filter(|&ii| matches!(items[ii].kind, ItemKind::Normal))
        .filter_map(|ii| head(ii).filter(|&k| word(k) == "struct" && (sym(k + 2, b';') || (sym(k + 2, b'{') && sym(k + 3, b'}')))).map(|k| word(k + 1)))
        .collect();
    // (an inherent impl's const or fn nothing names: as `(line, message)`)
    let dead_member = |ii: usize, h: usize| -> Option<(usize, String)> {
        let hd = items.get(h)?;
        let ht = src.get(hd.start..hd.end)?;
        if skip_attr(ht) || ht.contains("#[automatically_derived") {
            return None;
        }
        // (a trait's impl: `for` not of a `for<'a>`)
        let t = crate::lexer::tokenize(ht.as_bytes());
        if t.iter().enumerate().any(|(q, x)| x.0 == 0 && &ht[x.1..x.2] == "for" && !t.get(q + 1).is_some_and(|y| y.0 == 1 && ht.as_bytes()[y.1] == b'<')) {
            return None;
        }
        let sn = impl_self(h)?;
        if sn.contains("__") {
            return None;
        }
        // (the self type's dead_code level reaches its impls)
        let allowed = (0..items.len()).any(|jj| {
            matches!(items[jj].kind, ItemKind::Normal) && head(jj).is_some_and(|k| matches!(word(k), "struct" | "enum" | "union") && word(k + 1) == sn) && skip_attr(attrs_of(jj))
        });
        if allowed {
            return None;
        }
        let k = head(ii)?;
        let kw = word(k);
        let name = word(k + 1);
        if !matches!(kw, "fn" | "const") || name.is_empty() || name.starts_with('_') || skip_attr(attrs_of(ii)) {
            return None;
        }
        if (first_tok[ii]..k).any(|j| word(j) == "pub") || uses.get(name).copied().unwrap_or(0) > 0 {
            return None;
        }
        // (the parameters: past the generics)
        let mut p = k + 2;
        if sym(p, b'<') {
            let mut d = 0;
            while p < n {
                if sym(p, b'<') {
                    d += 1;
                } else if sym(p, b'>') && !sym(p - 1, b'-') {
                    d -= 1;
                    if d == 0 {
                        p += 1;
                        break;
                    }
                }
                p += 1;
            }
        }
        let what = if kw == "const" {
            "associated constant"
        } else if sym(p, b'(') && (1..=4).map(|d| word(p + d)).take_while(|w| matches!(*w, "" | "mut" | "self") || toks[p + 1].0 == 1).any(|w| w == "self") && !sym(p + 1, b')') {
            "method"
        } else {
            "associated function"
        };
        Some((line_of(k), format!("{what} `{name}` is never used")))
    };
    // (each enum of the program's and its variants' names: the identifiers directly in its braces before `(` / `{` /
    // `,` / `=` / its end)
    let mut enum_variants: FxMap<&str, FxSet<&str>> = Default::default();
    for ii in 0..items.len() {
        if !matches!(items[ii].kind, ItemKind::Normal) {
            continue;
        }
        let Some(k) = head(ii).filter(|&k| word(k) == "enum") else { continue };
        let Some(open) = (k + 2..n).take_while(|&q| item_of[q] == ii).find(|&q| sym(q, b'{')) else { continue };
        if close[open] == none {
            continue;
        }
        let vs = enum_variants.entry(word(k + 1)).or_default();
        for q in open + 1..close[open] {
            if toks[q].0 == 0 && encl[q] == open && (sym(q + 1, b'(') || sym(q + 1, b'{') || sym(q + 1, b',') || sym(q + 1, b'=') || q + 1 == close[open]) {
                vs.insert(word(q));
            }
        }
    }
    for ii in 0..items.len() {
        let it = &items[ii];
        match it.kind {
            ItemKind::Normal => {}
            ItemKind::ImplMember(h) => {
                if let Some(e) = dead_member(ii, h as usize) {
                    return Some(e);
                }
                continue;
            }
            _ => continue,
        }
        let Some(k) = head(ii) else { continue };
        let kw = word(k);
        if !matches!(kw, "struct" | "union" | "enum") {
            continue;
        }
        let name = word(k + 1);
        let a = attrs_of(ii);
        if name.is_empty() || name.contains("__") || name.starts_with('_') || skip_attr(a) || (0..k).any(|j| item_of[j] == ii && word(j) == "pub") {
            continue;
        }
        let ds = derives(a);
        // (the body: past the generics and a where clause)
        let mut j = k + 2;
        if sym(j, b'<') {
            let mut d = 0;
            while j < n {
                if sym(j, b'<') {
                    d += 1;
                } else if sym(j, b'>') && !sym(j - 1, b'-') {
                    d -= 1;
                    if d == 0 {
                        j += 1;
                        break;
                    }
                }
                j += 1;
            }
        }
        if kw != "enum" && sym(j, b'(') {
            // (a tuple struct)
        } else {
            while j < n && item_of[j] == ii && !sym(j, b'{') && !sym(j, b';') {
                j += 1;
            }
        }
        if !(sym(j, b'{') || sym(j, b'(')) || close[j] == none {
            continue;
        }
        let (open, end) = (j, close[j]);
        let tuple = sym(open, b'(');
        // (the parts: split at the commas between the brackets; each one's attributes, visibility, name)
        let mut parts: Vec<(usize, usize)> = vec![];
        let mut s = open + 1;
        // (not at a comma of a type's arguments: `Result<u8, u8>`)
        let mut angle = 0i32;
        for q in open + 1..=end {
            if q < end && encl[q] == open {
                if sym(q, b'<') {
                    angle += 1;
                } else if sym(q, b'>') && !sym(q - 1, b'-') {
                    angle -= 1;
                }
            }
            if q == end || (sym(q, b',') && encl[q] == open && angle <= 0) {
                if s < q {
                    parts.push((s, q));
                }
                s = q + 1;
            }
        }
        // (the occurrences of the name: whether anything constructs it, whether a pattern (or anything unclear) names
        // it, whether an update fills it)
        let mut built = false;
        let mut unclear = false;
        let mut written: FxSet<&str> = Default::default();
        for q in 0..n {
            if item_of[q] == ii || item_of[q] == none || is_use_item(item_of[q]) || !names_at(q, name) {
                continue;
            }
            if matches!(items[item_of[q]].kind, ItemKind::ImplHeader) {
                continue;
            }
            if kw == "enum" {
                continue;
            }
            if sym(q + 1, b':') && sym(q + 2, b':') && sym(q + 3, b'<') {
                unclear = true;
                continue;
            }
            let ps = path_start(q);
            // (a type: a return type, `impl .. for`)
            if sym(ps.wrapping_sub(1), b'>') && sym(ps.wrapping_sub(2), b'-') {
                continue;
            }
            let g = q + 1;
            if !(if tuple { sym(g, b'(') } else { sym(g, b'{') }) || close[g] == none {
                continue;
            }
            if pattern_like(ps, close[g]) {
                unclear = true;
            } else {
                built = true;
                // (a union's field is used when something writes it)
                if kw == "union" {
                    for x in g + 1..close[g] {
                        if encl[x] == g && toks[x].0 == 0 && (sym(x + 1, b':') || sym(x + 1, b',') || x + 1 == close[g]) && (x == g + 1 || sym(x - 1, b',')) {
                            written.insert(word(x));
                        }
                    }
                }
                if (g + 1..close[g]).any(|x| sym(x, b'.') && sym(x + 1, b'.') && encl[x] == g) {
                    unclear = true;
                }
            }
        }
        if kw == "enum" {
            if ds.iter().any(|d| !matches!(d.as_str(), "Clone" | "Copy" | "Debug" | "PartialEq" | "Eq" | "Hash" | "PartialOrd" | "Ord" | "Default")) {
                continue;
            }
            if uses.get(name).copied().unwrap_or(0) == 0 {
                continue;
            }
            let mut dead: Vec<(String, usize)> = vec![];
            for &(s, e) in &parts {
                let mut v = s;
                let mut default = false;
                while sym(v, b'#') && sym(v + 1, b'[') && close[v + 1] != none {
                    let at = &region[toks[v].1..toks[close[v + 1]].2];
                    if skip_attr(at) || at.contains("cfg") {
                        v = e;
                        break;
                    }
                    default |= at.replace(' ', "") == "#[default]";
                    v = close[v + 1] + 1;
                }
                if v >= e || toks[v].0 != 0 {
                    continue;
                }
                let vn = word(v);
                if vn.starts_with('_') || default {
                    continue;
                }
                // (its brackets, as its definition has them: `V(..)`, `V { .. }`, none)
                let shape = if sym(v + 1, b'(') { b'(' } else if sym(v + 1, b'{') { b'{' } else { 0 };
                // (constructed: an occurrence outside the enum that is not (or may not be) in a pattern)
                let made = (0..n).any(|q| {
                    if word(q) != vn || item_of[q] == ii || item_of[q] == none || is_use_item(item_of[q]) {
                        return false;
                    }
                    // (`F::B` of another enum of the program's that has a `B` too: not this one's)
                    if q >= 3 && sym(q - 1, b':') && sym(q - 2, b':') && word(q - 3) != name && word(q - 3) != "Self" && enum_variants.get(word(q - 3)).is_some_and(|vs| vs.contains(vn)) {
                        return false;
                    }
                    let g = q + 1;
                    let e = if shape != 0 && sym(g, shape) && close[g] != none { close[g] } else { q };
                    !pattern_like(path_start(q), e)
                });
                if !made {
                    dead.push((vn.to_string(), v));
                }
            }
            if let Some(&(_, v)) = dead.first() {
                let names: Vec<String> = dead.into_iter().map(|d| d.0).collect();
                return Some((line_of(v), list(&names, "variant", "constructed")));
            }
            continue;
        }
        if !built || unclear || ds.iter().any(|d| !matches!(d.as_str(), "Clone" | "Copy" | "Debug")) {
            continue;
        }
        let mut dead: Vec<(String, usize)> = vec![];
        for (idx, &(s, e)) in parts.iter().enumerate() {
            let mut f = s;
            let mut skip = false;
            while sym(f, b'#') && sym(f + 1, b'[') && close[f + 1] != none {
                let at = &region[toks[f].1..toks[close[f + 1]].2];
                skip |= skip_attr(at) || at.contains("cfg");
                f = close[f + 1] + 1;
            }
            if skip || word(f) == "pub" || f >= e {
                continue;
            }
            if tuple {
                // (a field of a 1-ZST: `()`, PhantomData, a unit struct of the program's)
                let ty = region[toks[f].1..toks[e - 1].2].replace(' ', "");
                let ty = ty.rsplit("::").next().unwrap_or("");
                if ty == "()" || ty.starts_with("PhantomData") || unit_structs.contains(ty) || ty.ends_with(";0]") {
                    continue;
                }
                let num = idx.to_string();
                if !is_read(name, &num) {
                    dead.push((num, f));
                }
            } else {
                let fname = word(f);
                if fname.is_empty() || fname.starts_with('_') || !sym(f + 1, b':') {
                    continue;
                }
                if !is_read(name, fname) && !written.contains(fname) {
                    dead.push((fname.to_string(), f));
                }
            }
        }
        if let Some(&(_, f)) = dead.first() {
            let names: Vec<String> = dead.into_iter().map(|d| d.0).collect();
            return Some((line_of(f), list(&names, "field", "read")));
        }
    }
    None
}
