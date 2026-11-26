//! Rewrites tuple variants / tuple structs used as values (`.map(Some)` / `.map(Ty::Struct)` / `.map(Wrapper)`)
//! into closures (`|a0| Some(a0)`). As closures, type inference and lifting treat them the same as hand-written ones.
//! The rewriting is done within the same line and does not change the line numbers after it.
//! It depends on no external crates (only the lexer's tokenizing), so zustc itself can compile it too.

use crate::lexer::tokenize;

/// Element counts of tuple variants / tuple structs read from definitions: (enum name (empty for a struct), name, element count)
fn arities(s: &[u8], toks: &[(u8, usize, usize)], defs: &mut Vec<(usize, usize)>) -> Vec<(String, String, usize)> {
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let name = |k: usize| String::from_utf8_lossy(tok(k)).into_owned();
    // The number of top-level comma-separated elements inside the brackets (k is the opening one) and the position of the closing bracket
    let count = |k: usize| -> (usize, usize) {
        let mut d = 0i32;
        let mut n = 0usize;
        let mut any = false;
        let mut j = k;
        while j < toks.len() {
            if is_sym(j, b'(') || is_sym(j, b'[') || is_sym(j, b'{') || is_sym(j, b'<') {
                d += 1;
                if d > 1 {
                    any = true;
                }
            } else if is_sym(j, b')') || is_sym(j, b']') || is_sym(j, b'}') || (is_sym(j, b'>') && !(j >= 1 && is_sym(j - 1, b'-'))) {
                d -= 1;
                if d == 0 {
                    return (if any { n + 1 } else { n }, j);
                }
            } else if d == 1 && is_sym(j, b',') {
                if any {
                    n += 1;
                }
                any = false;
            } else if d >= 1 {
                any = true;
            }
            j += 1;
        }
        (n, j)
    };
    let mut out = vec![];
    let mut k = 0;
    while k + 1 < toks.len() {
        let kw = tok(k);
        if toks[k].0 == 0 && (kw == b"enum".as_slice() || kw == b"struct".as_slice()) && toks[k + 1].0 == 0 {
            let ty = name(k + 1);
            let mut j = k + 2;
            if is_sym(j, b'<') {
                j = count(j).1 + 1;
            }
            if kw == b"struct".as_slice() {
                if is_sym(j, b'(') {
                    let (n, close) = count(j);
                    out.push((String::new(), ty, n));
                    defs.push((j, close));
                }
                k = j;
                continue;
            }
            while j < toks.len() && !is_sym(j, b'{') {
                j += 1;
            }
            if j >= toks.len() {
                break;
            }
            let end = count(j).1;
            defs.push((j, end));
            // `V ( .. )` directly inside the braces
            let mut m = j + 1;
            let mut d = 0i32;
            while m < end {
                if is_sym(m, b'(') || is_sym(m, b'[') || is_sym(m, b'{') {
                    if d == 0 && is_sym(m, b'(') && m >= 1 && toks[m - 1].0 == 0 {
                        let (n, close) = count(m);
                        out.push((ty.clone(), name(m - 1), n));
                        m = close + 1;
                        continue;
                    }
                    d += 1;
                } else if is_sym(m, b')') || is_sym(m, b']') || is_sym(m, b'}') {
                    d -= 1;
                }
                m += 1;
            }
            k = end;
            continue;
        }
        k += 1;
    }
    out
}

/// The rewritten source (None if there is nothing to rewrite)
pub fn rewrite_ctor_values(src: &str) -> Option<String> {
    let s = src.as_bytes();
    // (Skip fast: do nothing if no candidate shape like `(Some)` / `::X)` is present)
    let quick = src.contains("(Some)") || src.contains("(Ok)") || src.contains("(Err)") || src.contains("::") || src.contains("struct");
    if !quick {
        return None;
    }
    let toks = tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| k < toks.len() && toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let mut defs = vec![];
    let ar = arities(s, &toks, &mut defs);
    // (whether the `=` at p is a type alias's or an associated type's, `type A where W: Sized = W;`: a `type` from
    // the statement's start)
    let alias_value = |p: usize| {
        let mut q = p;
        while q > 0 && !(is_sym(q - 1, b';') || is_sym(q - 1, b'{') || is_sym(q - 1, b'}')) {
            q -= 1;
            if toks[q].0 == 0 && tok(q) == b"type" {
                return true;
            }
        }
        false
    };
    // Inside use statements and struct / enum definitions is not rewritten
    let mut in_use = vec![false; toks.len()];
    for &(a, b) in &defs {
        for x in &mut in_use[a..b.min(toks.len())] {
            *x = true;
        }
    }
    let mut k = 0;
    while k < toks.len() {
        if toks[k].0 == 0 && tok(k) == b"use".as_slice() {
            let mut j = k;
            while j < toks.len() && !is_sym(j, b';') {
                in_use[j] = true;
                j += 1;
            }
            k = j;
        }
        k += 1;
    }
    let mut repl: Vec<(usize, usize, String)> = vec![];
    let mut impls: Option<Vec<(usize, usize, String)>> = None;
    let mut k = 1;
    while k < toks.len() {
        if toks[k].0 != 0 || in_use[k] {
            k += 1;
            continue;
        }
        // Before: `(` / `,` / `=` (not `==` etc.)
        let p = k - 1;
        let eq = is_sym(p, b'=') && !(p >= 1 && matches!(s[toks[p].1 - 1], b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'|' | b'&' | b'^' | b'%')) && !is_sym(k, b'=');
        // (not a type alias's right side: `type Ok = Ok;` names a type (a parameter `Ok`), not the variant)
        let alias = eq && p >= 2 && toks[p - 1].0 == 0 && toks[p - 2].0 == 0 && tok(p - 2) == b"type";
        let prev_ok = (is_sym(p, b'(') || is_sym(p, b',') || eq) && !alias;
        if !prev_ok {
            k += 1;
            continue;
        }
        // `A::B` or `B`
        let (ty, name, last) = if k + 3 < toks.len() && is_sym(k + 1, b':') && is_sym(k + 2, b':') && toks[k + 3].0 == 0 {
            (String::from_utf8_lossy(tok(k)).into_owned(), String::from_utf8_lossy(tok(k + 3)).into_owned(), k + 3)
        } else {
            (String::new(), String::from_utf8_lossy(tok(k)).into_owned(), k)
        };
        // (a turbofish after it: `P::<u16>` / `Some::<u32>`, kept on the call)
        let mut last = last;
        if is_sym(last + 1, b':') && is_sym(last + 2, b':') && is_sym(last + 3, b'<') {
            let mut d = 0i32;
            let mut j = last + 3;
            while j < toks.len() {
                if is_sym(j, b'<') {
                    d += 1;
                } else if is_sym(j, b'>') && !is_sym(j - 1, b'-') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                } else if is_sym(j, b';') || is_sym(j, b'{') || is_sym(j, b'}') {
                    break;
                }
                j += 1;
            }
            if d == 0 && j < toks.len() {
                last = j;
            }
        }
        // After: `)` / `,` / `;` (not a continuation of `::`)
        let next_ok = last + 1 < toks.len() && (is_sym(last + 1, b')') || is_sym(last + 1, b',') || is_sym(last + 1, b';')) && !(is_sym(last + 1, b':'));
        if !next_ok || !name.starts_with(|c: char| c.is_ascii_uppercase()) {
            k += 1;
            continue;
        }
        // (`opt.map(Self)` in an impl of a tuple struct: the constructor of the impl's type, as `Self(..)` is. Only
        // as a call's argument: not `type Output = Self;`. The impls' extents are found once, when one is needed)
        let self_ty;
        let name = if name == "Self" && ty.is_empty() {
            let arg = is_sym(p, b'(') && p >= 1 && toks[p - 1].0 == 0 && !matches!(tok(p - 1), b"fn" | b"Fn" | b"FnMut" | b"FnOnce");
            let impls = match (arg, &impls) {
                (false, _) => None,
                (true, Some(v)) => Some(v),
                (true, None) => {
                    impls = Some(impl_extents(s, &toks));
                    impls.as_ref()
                }
            };
            // (through a type alias, `type T = ST6; impl T { .. }`)
            self_ty = impls
                .and_then(|v| v.iter().rev().find(|x| x.0 < k && k < x.1).map(|x| x.2.clone()))
                .map(|n| if ar.iter().any(|a| a.0.is_empty() && a.1 == n) { n.clone() } else { alias_of(s, &toks, &n).unwrap_or(n) })
                .unwrap_or_default();
            if self_ty.is_empty() {
                k += 1;
                continue;
            }
            self_ty.as_str()
        } else {
            name.as_str()
        };
        let n = match (ty.as_str(), name) {
            ("" | "Option", "Some") | ("" | "Result", "Ok") | ("" | "Result", "Err") => Some(1),
            // Tuple structs only in call argument position (to tell them from type positions like `(u32, Wrapper)`)
            ("", _) => ar
                .iter()
                .find(|a| a.0.is_empty() && a.1 == *name)
                // (or the value of a `let` / `const` / assignment: `let f: fn(isize) -> Foo = Foo;`, not `type A = Foo;`)
                // (not the argument of a fn pointer's type or an `Fn(..)` bound: `fn(Foo)`, `dyn Fn(Foo)` name the type)
                .filter(|_| (is_sym(p, b'(') && p >= 1 && toks[p - 1].0 == 0 && !matches!(tok(p - 1), b"fn" | b"Fn" | b"FnMut" | b"FnOnce" | b"AsyncFn" | b"AsyncFnMut" | b"AsyncFnOnce"))
                    || (eq && is_sym(last + 1, b';') && !alias_value(p) && !(p >= 1 && is_sym(p - 1, b'>'))))
                .map(|a| a.2),
            (t, _) => ar.iter().find(|a| a.0 == t && a.1 == *name).map(|a| a.2),
        };
        // (Self::V is not rewritten, since its type is not known on the spot)
        let Some(n) = n.filter(|&n| n > 0 && ty != "Self") else {
            k += 1;
            continue;
        };
        let xs: Vec<String> = (0..n).map(|i| format!("__c{i}")).collect();
        let path = String::from_utf8_lossy(&s[toks[k].1..toks[last].2]).into_owned();
        repl.push((toks[k].1, toks[last].2, format!("|{}| {path}({})", xs.join(", "), xs.join(", "))));
        k = last + 1;
    }
    if repl.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(src.len() + repl.len() * 16);
    let mut at = 0;
    for (a, b, t) in &repl {
        out.push_str(&src[at..*a]);
        out.push_str(t);
        at = *b;
    }
    out.push_str(&src[at..]);
    Some(out)
}

/// The impls' bodies (from their `{` to their `}`, by token) and the names of their types (`impl<T> Tr for W<T>`:
/// `W`), in the order they start: an inner one after the one around it
fn impl_extents(s: &[u8], toks: &[(u8, usize, usize)]) -> Vec<(usize, usize, String)> {
    let tok = |j: usize| -> &[u8] { &s[toks[j].1..toks[j].2] };
    let sym = |j: usize, c: u8| toks[j].0 == 1 && toks[j].2 == toks[j].1 + 1 && s[toks[j].1] == c;
    let mut out = vec![];
    let mut j = 0;
    while j < toks.len() {
        if toks[j].0 == 0 && tok(j) == b"impl".as_slice() {
            // (the type's name: after `for` when there is one, else the first name past the generics)
            let (mut name, mut depth, mut m) = (None, 0i32, j + 1);
            while m < toks.len() && !(depth == 0 && sym(m, b'{')) && !sym(m, b';') {
                if sym(m, b'<') {
                    depth += 1;
                } else if sym(m, b'>') && !sym(m - 1, b'-') {
                    depth -= 1;
                } else if depth == 0 && toks[m].0 == 0 {
                    match tok(m) {
                        b"for" => name = None,
                        b"where" => break,
                        t if name.is_none() && t.first().is_some_and(u8::is_ascii_uppercase) => name = Some(m),
                        _ => {}
                    }
                }
                m += 1;
            }
            while m < toks.len() && !sym(m, b'{') {
                m += 1;
            }
            // (its body's end)
            let (mut d, mut e) = (0i32, m);
            while e < toks.len() {
                if sym(e, b'{') {
                    d += 1;
                } else if sym(e, b'}') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                e += 1;
            }
            if let Some(n) = name {
                out.push((m, e, String::from_utf8_lossy(tok(n)).into_owned()));
            }
            j = m;
        }
        j += 1;
    }
    out
}

/// The type a `type name = Other..;` alias names, by its first name (`Other`)
fn alias_of(s: &[u8], toks: &[(u8, usize, usize)], name: &str) -> Option<String> {
    let tok = |j: usize| -> &[u8] { &s[toks[j].1..toks[j].2] };
    let sym = |j: usize, c: u8| toks[j].0 == 1 && toks[j].2 == toks[j].1 + 1 && s[toks[j].1] == c;
    (0..toks.len().saturating_sub(3))
        .find(|&j| toks[j].0 == 0 && tok(j) == b"type".as_slice() && toks[j + 1].0 == 0 && tok(j + 1) == name.as_bytes() && sym(j + 2, b'=') && toks[j + 3].0 == 0)
        .map(|j| String::from_utf8_lossy(tok(j + 3)).into_owned())
}
