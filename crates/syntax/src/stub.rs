//! Turns a dependency crate's source into the form passed to the crates that use it (the interface).
//!
//! In a workspace, a user crate also analyzes the source of its dependency crates together with its own (it needs the types, signatures and bodies of generic fns).
//! The bodies of non-generic fns are compiled by the dependency crate itself into its .o, so the user does not need them: they become `{ loop {} }`
//! (the newlines are kept to preserve the line correspondence). Macro expansion, flattening of modules, lexing, summaries and Program get lighter by the dependencies' bodies.
//!
//! What keeps its body: generic fns (`fn f<T>` / `impl Trait` in parameters / `where`), generic impls (`impl<T> ..`) and the inside of traits
//! (default methods are monomorphized with the user's types), `const fn`, those returning `impl Trait` (the type is decided from the body), `async fn`

type Tok = (u8, usize, usize);

/// Bracket matching (open → close). Unmatched ones are usize::MAX
fn matching(s: &[u8], toks: &[Tok]) -> Vec<usize> {
    let mut close = vec![usize::MAX; toks.len()];
    let mut st: Vec<usize> = vec![];
    for (k, t) in toks.iter().enumerate() {
        if t.0 != 1 {
            continue;
        }
        match s[t.1] {
            b'(' | b'[' | b'{' => st.push(k),
            b')' | b']' | b'}' => {
                if let Some(o) = st.pop() {
                    close[o] = k;
                }
            }
            _ => {}
        }
    }
    close
}

/// The source with bodies made `{ loop {} }`. Unchanged if there is nothing to change
pub fn interface_text(src: &str) -> String {
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let close = matching(s, &toks);
    let n = toks.len();
    let t = |k: usize| -> &[u8] {
        if k < n {
            &s[toks[k].1..toks[k].2]
        } else {
            b""
        }
    };
    let id = |k: usize, w: &[u8]| k < n && toks[k].0 == 0 && t(k) == w;
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && s[toks[k].1] == c;
    // (the ranges of bodies to remove: from after the `{` to before the `}`)
    let mut cut: Vec<(usize, usize)> = vec![];
    // The context of each open brace: 0 = module (top level), 1 = non-generic impl, 2 = other (the inside is not looked at)
    let mut ctx: Vec<(usize, u8)> = vec![];
    let mut pending: u8 = 2;
    let mut k = 0;
    while k < n {
        // (pop the closed contexts)
        while let Some(&(e, _)) = ctx.last() {
            if k > e {
                ctx.pop();
            } else {
                break;
            }
        }
        let cur = ctx.last().map(|c| c.1).unwrap_or(0);
        if sym(k, b'{') {
            let e = close[k];
            if e == usize::MAX {
                break;
            }
            let c = pending;
            pending = 2;
            if c == 2 {
                // (the inside is not looked at: struct bodies, generic impls, traits, fn bodies, etc.)
                k = e + 1;
                continue;
            }
            ctx.push((e, c));
            k += 1;
            continue;
        }
        if cur == 2 {
            k += 1;
            continue;
        }
        if id(k, b"mod") && k + 2 < n && toks[k + 1].0 == 0 && sym(k + 2, b'{') {
            pending = 0;
            k += 2;
            continue;
        }
        if id(k, b"impl") && cur == 0 {
            // (`impl<..>` is generic: the inside is not looked at)
            pending = if sym(k + 1, b'<') { 2 } else { 1 };
            k += 1;
            continue;
        }
        if id(k, b"fn") && toks.get(k + 1).is_some_and(|x| x.0 == 0) {
            // (the preceding qualifiers: const fn / async fn are kept)
            let mut j = k;
            let mut keep = false;
            while j > 0 {
                j -= 1;
                let w = t(j);
                if toks[j].0 == 0 && (w == b"const" || w == b"async") {
                    keep = true;
                    break;
                }
                if !(toks[j].0 == 0 && (w == b"pub" || w == b"unsafe" || w == b"extern" || w == b"crate" || w == b"default")) && toks[j].0 != 2 && !sym(j, b'(') && !sym(j, b')') {
                    break;
                }
            }
            let mut i = k + 2;
            if sym(i, b'<') {
                keep = true;
            }
            // (the parameter list)
            while i < n && !sym(i, b'(') && !sym(i, b'{') && !sym(i, b';') {
                i += 1;
            }
            if !sym(i, b'(') || close[i] == usize::MAX {
                k = i;
                continue;
            }
            let pe = close[i];
            if (i..pe).any(|q| id(q, b"impl")) {
                keep = true;
            }
            // (the return type and where: up to the body's `{` or the declaration's `;`)
            let mut q = pe + 1;
            while q < n && !sym(q, b'{') && !sym(q, b';') {
                if id(q, b"impl") || id(q, b"where") {
                    keep = true;
                }
                if sym(q, b'(') || sym(q, b'[') {
                    if close[q] == usize::MAX {
                        break;
                    }
                    q = close[q];
                }
                q += 1;
            }
            if !sym(q, b'{') || close[q] == usize::MAX {
                k = q + 1;
                continue;
            }
            let be = close[q];
            if !keep && be > q + 1 {
                cut.push((toks[q].2, toks[be].1));
            }
            k = be + 1;
            continue;
        }
        k += 1;
    }
    if cut.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for (a, z) in cut {
        out.push_str(&src[last..a]);
        out.push_str(" loop {} ");
        for &c in &s[a..z] {
            if c == b'\n' {
                out.push('\n');
            }
        }
        last = z;
    }
    out.push_str(&src[last..]);
    out
}

/// An item in a module (so that users can follow a dependency crate's interface by name)
pub struct StubItem {
    /// The names defined (for an impl, all the names in its header: if the type or the trait is reached, the impl is reached too)
    pub names: Vec<String>,
    /// The hash of the item's text
    pub hash: crate::key::Key,
    /// All the identifiers inside
    pub idents: Vec<String>,
    /// The range of the text (bytes)
    pub start: usize,
    pub end: usize,
    /// `use` (needed for name resolution, so not removed even when unreached)
    pub is_use: bool,
}

/// The items of src (a crate's text). Inside `mod m { .. }` too (the mod itself is not made an item)
pub fn items(src: &str) -> Vec<StubItem> {
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let close = matching(s, &toks);
    let n = toks.len();
    let t = |k: usize| -> &[u8] {
        if k < n {
            &s[toks[k].1..toks[k].2]
        } else {
            b""
        }
    };
    let id = |k: usize, w: &[u8]| k < n && toks[k].0 == 0 && t(k) == w;
    let sym = |k: usize, c: u8| k < n && toks[k].0 == 1 && s[toks[k].1] == c;
    let mut out: Vec<StubItem> = vec![];
    let mut k = 0;
    while k < n {
        // (a module's closing brace)
        if sym(k, b'}') || sym(k, b';') {
            k += 1;
            continue;
        }
        // `[pub] mod m {`: look inside
        let mut j = k;
        while sym(j, b'#') && sym(j + 1, b'[') && close[j + 1] != usize::MAX {
            j = close[j + 1] + 1;
        }
        if id(j, b"pub") {
            j += 1;
            if sym(j, b'(') && close[j] != usize::MAX {
                j = close[j] + 1;
            }
        }
        if id(j, b"mod") && toks.get(j + 1).is_some_and(|x| x.0 == 0) && sym(j + 2, b'{') {
            k = j + 3;
            continue;
        }
        // The end of an item: `;` or the body's `}` (const / static / use / type go up to the `;`)
        // (`const fn` / `const unsafe fn` / `const async fn` / `const extern "C" fn` are fns)
        let const_fn = |q: usize| id(q, b"const") && (id(q + 1, b"fn") || id(q + 1, b"unsafe") || id(q + 1, b"async") || id(q + 1, b"extern"));
        let semi_kind = (j..n.min(j + 4)).any(|q| id(q, b"const") && !const_fn(q) || id(q, b"static") || id(q, b"use") || id(q, b"type"));
        // (`#![..]` (an inner attribute) is one by itself)
        if sym(k, b'#') && sym(k + 1, b'!') && sym(k + 2, b'[') && close[k + 2] != usize::MAX {
            k = close[k + 2] + 1;
            continue;
        }
        let mut e = j;
        while e < n {
            if sym(e, b';') {
                break;
            }
            // (the closing brace of the enclosing module: end just before it)
            if sym(e, b'}') {
                e = if e > k { e - 1 } else { k };
                break;
            }
            if (sym(e, b'(') || sym(e, b'[') || sym(e, b'{')) && close[e] != usize::MAX {
                let c = close[e];
                if sym(e, b'{') && !semi_kind {
                    e = c;
                    if sym(e + 1, b';') {
                        e += 1;
                    }
                    break;
                }
                e = c + 1;
                continue;
            }
            e += 1;
        }
        let e = e.min(n - 1);
        let mut idents: Vec<String> = vec![];
        let mut q = k;
        while q <= e {
            if toks[q].0 == 0 {
                let w = String::from_utf8_lossy(t(q)).into_owned();
                if !idents.contains(&w) {
                    idents.push(w);
                }
            }
            q += 1;
        }
        let mut names: Vec<String> = vec![];
        // (an `extern` block declares many: each fn's and static's name names the block, `libc::getpid()` reaches it)
        let open = (j..e).find(|&q| sym(q, b'{'));
        if let Some(o) = open.filter(|&o| (j..o).any(|q| id(q, b"extern")) && !(j..o).any(|q| id(q, b"fn") || id(q, b"impl") || id(q, b"mod"))) {
            let mut q = o;
            while q + 1 < e {
                if (id(q, b"fn") || id(q, b"static")) && toks[q + 1].0 == 0 {
                    let nm = if id(q, b"static") && id(q + 1, b"mut") { q + 2 } else { q + 1 };
                    names.push(String::from_utf8_lossy(t(nm)).into_owned());
                }
                q += 1;
            }
        }
        let mut q = if names.is_empty() { j } else { e };
        while q < e {
            let w = t(q);
            if toks[q].0 == 0 && matches!(w, b"fn" | b"struct" | b"enum" | b"union" | b"trait" | b"type" | b"const" | b"static") && toks.get(q + 1).is_some_and(|x| x.0 == 0) {
                let nm = if w == b"static" && id(q + 1, b"mut") { q + 2 } else { q + 1 };
                if !(w == b"const" && const_fn(q)) {
                    names.push(String::from_utf8_lossy(t(nm)).into_owned());
                    break;
                }
            }
            if toks[q].0 == 0 && w == b"use" {
                // (a use: all the names in it)
                let mut r = q + 1;
                while r <= e {
                    if toks[r].0 == 0 {
                        names.push(String::from_utf8_lossy(t(r)).into_owned());
                    }
                    r += 1;
                }
                break;
            }
            if toks[q].0 == 0 && w == b"impl" {
                // (an impl: the names of the trait and type in its header. Inside `impl<..>`, from `where` on, primitive types and keywords are
                //  not counted: so that `impl FromHex for [u8; 4]` is not reached from every crate that writes `u8`)
                let mut r = q + 1;
                if sym(r, b'<') {
                    let mut d = 0i32;
                    while r <= e {
                        if sym(r, b'<') {
                            d += 1;
                        } else if sym(r, b'>') && !(r > 0 && sym(r - 1, b'-')) {
                            d -= 1;
                            if d == 0 {
                                r += 1;
                                break;
                            }
                        }
                        r += 1;
                    }
                }
                // (for a trait impl whose target is a std type (`Vec<u8>` / `[u8; 4]` etc.), only the trait's name)
                let has_for = (r..e).take_while(|&x| !sym(x, b'{')).any(|x| id(x, b"for"));
                let mut after_for = false;
                let mut tr_names: Vec<String> = vec![];
                let mut ty_names: Vec<String> = vec![];
                while r <= e && !sym(r, b'{') && !id(r, b"where") {
                    if id(r, b"for") {
                        after_for = true;
                    } else if toks[r].0 == 0 {
                        let x = t(r);
                        let std_ty = matches!(
                            x,
                            b"Vec" | b"String" | b"Option" | b"Result" | b"Box" | b"HashMap" | b"HashSet" | b"BTreeMap" | b"BTreeSet" | b"VecDeque" | b"Rc" | b"Arc" | b"Cell" | b"RefCell"
                        );
                        let skip = crate::types::Ty::prim(&String::from_utf8_lossy(x)).is_some()
                            || matches!(x, b"str" | b"dyn" | b"mut" | b"const" | b"unsafe" | b"crate" | b"self" | b"super" | b"Self")
                            || (after_for && std_ty);
                        if !skip {
                            let w = String::from_utf8_lossy(x).into_owned();
                            if has_for && !after_for {
                                tr_names.push(w);
                            } else {
                                ty_names.push(w);
                            }
                        }
                    }
                    r += 1;
                }
                names.extend(tr_names);
                names.extend(ty_names);
                if names.is_empty() {
                    names.push("impl".to_string());
                }
                break;
            }
            q += 1;
        }
        if names.is_empty() {
            names = idents.clone();
        }
        let mut h = crate::key::KeyHasher::new();
        h.add(&src[toks[k].1..toks[e].2]);
        let is_use = (j..e.min(j + 3)).any(|q| id(q, b"use"));
        out.push(StubItem { names, hash: h.finish(), idents, start: toks[k].1, end: toks[e].2, is_use });
        k = e + 1;
    }
    out
}

/// Blanks out the items of src whose keep[k] is false (and that are not `use`) (newlines are kept)
pub fn prune(src: &str, its: &[StubItem], keep: &[bool]) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut last = 0;
    for (k, it) in its.iter().enumerate() {
        if keep.get(k).copied().unwrap_or(true) || it.is_use || it.start < last {
            continue;
        }
        out.push_str(&src[last..it.start]);
        for &c in &b[it.start..it.end] {
            if c == b'\n' {
                out.push('\n');
            }
        }
        last = it.end;
    }
    out.push_str(&src[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `const unsafe fn` (also `const async fn`, `const extern "C" fn`) is a fn: it ends at its body and is named
    /// by its fn name, not taken for a constant named `unsafe` that runs to the next `;` (libc 0.2)
    /// An `extern` block is one item named by every fn and static it declares: a use of the second, `libc::getpid()`,
    /// reaches it (libc 0.2's unix declares many in one block)
    #[test]
    fn extern_block_is_named_by_all_its_fns() {
        let src = "extern \"C\" {\n    pub fn abs(i: i32) -> i32;\n    pub fn getpid() -> i32;\n    pub static mut environ: *const u8;\n}\n";
        let its = super::items(src);
        assert_eq!(its.len(), 1);
        for n in ["abs", "getpid", "environ"] {
            assert!(its[0].names.iter().any(|x| x == n), "{:?}", its[0].names);
        }
    }

    #[test]
    fn const_unsafe_fn_is_one_fn_item() {
        let src = "pub const unsafe fn f() -> u8 { 1 }\npub const extern \"C\" fn e() {}\npub fn g() {}\npub const N: u8 = 1;\n";
        let names: Vec<String> = items(src).iter().map(|i| i.names.join("+")).collect();
        assert_eq!(names, ["f", "e", "g", "N"]);
    }

    #[test]
    fn stubs_concrete_bodies() {
        let src = "pub mod m {\n pub fn a(x: u32) -> u32 {\n x + 1\n }\n pub fn g<T: Copy>(x: T) -> T { x }\n pub fn h(x: impl Fn()) { x() }\n pub const fn c() -> u8 { 3 }\n pub fn o() -> impl Iterator<Item = u8> { 0..3 }\n struct S;\n impl S { fn m(&self) -> u8 { 1 } }\n impl<T> W<T> { fn n(&self) { body() } }\n trait Tr { fn d(&self) -> u8 { 2 } }\n fn w<T>(t: T) where T: Copy { t; }\n mod inner { fn z() { q() } }\n}\n";
        let o = interface_text(src);
        assert_eq!(o.lines().count(), src.lines().count());
        let its = items(&o);
        let names: Vec<String> = its.iter().map(|i| i.names.join("+")).collect();
        assert_eq!(names, ["a", "g", "h", "c", "o", "S", "S", "W+T", "Tr", "w", "z"]);
        assert_eq!(
            norm(&o),
            norm("pub mod m { pub fn a(x: u32) -> u32 { loop {} } pub fn g<T: Copy>(x: T) -> T { x } pub fn h(x: impl Fn()) { x() } pub const fn c() -> u8 { 3 } pub fn o() -> impl Iterator<Item = u8> { 0..3 } struct S; impl S { fn m(&self) -> u8 { loop {} } } impl<T> W<T> { fn n(&self) { body() } } trait Tr { fn d(&self) -> u8 { 2 } } fn w<T>(t: T) where T: Copy { t; } mod inner { fn z() { loop {} } } }")
        );
    }
}
