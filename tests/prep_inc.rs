//! Whether incremental preprocessing (prep::prep) is the same as preprocessing the whole every time (prep::prep_full).
//! To real sources (examples/*.rs, and the files of crates/syntax/src bundled with `mod x { .. }`), keeps applying random edits,
//! comparing the text, the column shifts and success / failure at each step. The edits are both inside fn bodies (the skeleton does not change: the incremental path) and anywhere (the skeleton
//! changes: the full path)

use zustc::prep::{PrepMemo, marked, prep, prep_full};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn char_floor(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn edit(s: &str, r: &mut Rng) -> String {
    // (half right after a `let` line (inside a fn body), half anywhere)
    let at = if r.below(2) == 0 {
        let lets: Vec<usize> = s.match_indices("let ").map(|(i, _)| i).collect();
        if lets.is_empty() { char_floor(s, r.below(s.len() + 1)) } else { lets[r.below(lets.len())] }
    } else {
        char_floor(s, r.below(s.len() + 1))
    };
    let snippets = [
        "x", " ", "\n", "let q = 1;\n", "let v = Some(1).map(Some);\n", "{ fn inner() {} }\n", "\"", "'", "/*", "*/", "//", ";", "1 + 2",
        "fn added() {}\n", "\nmod mm { pub fn f() {} }\n", "struct T(u8);\n", "use std::path::Path;\n", "m!();\n", "thread_local! { static TT: u8 = 1; }\n",
        "\nmacro_rules! m { () => { 1 }; }\n",
    ];
    match r.below(3) {
        0 | 1 => {
            let t = snippets[r.below(snippets.len())];
            format!("{}{}{}", &s[..at], t, &s[at..])
        }
        _ => {
            let end = char_floor(s, (at + 1 + r.below(30)).min(s.len())).max(at);
            format!("{}{}", &s[..at], &s[end..])
        }
    }
}

fn sources() -> Vec<String> {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut v = vec![];
    let mut ps: Vec<_> = std::fs::read_dir(format!("{root}/examples")).unwrap().map(|e| e.unwrap().path()).collect();
    ps.sort();
    for p in ps.iter().filter(|p| p.extension().is_some_and(|e| e == "rs")).take(25) {
        v.push(std::fs::read_to_string(p).unwrap());
    }
    // (the files bundled into modules: rewriting of name collisions, use, nested items, macros)
    let mut bundle = String::from("fn main() {}\n");
    for m in ["ctors", "cfg", "stub", "macros"] {
        let t = std::fs::read_to_string(format!("{root}/crates/syntax/src/{m}.rs")).unwrap();
        let t: String = t.lines().filter(|l| !l.starts_with("//!") && !l.starts_with("#![")).map(|l| format!("{l}\n")).collect();
        bundle.push_str(&format!("mod {m} {{\n{t}}}\n"));
    }
    v.push(bundle);
    v
}

#[test]
fn prep_inc_matches_full() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    let (mut same, mut errs) = (0, 0);
    for src in sources() {
        let mut memo = PrepMemo::default();
        let mut cur = src.clone();
        let mut prev = src.clone();
        for step in 0..40 {
            let mut f1 = vec![];
            let mut f2 = vec![];
            let full = prep_full(&cur, &[], &mut f1, None).map(marked);
            let inc = prep(&cur, &[], &mut f2, None, &mut memo);
            match (full, inc) {
                (Ok(a), Ok(b)) => {
                    if a != b || f1 != f2 {
                        let d = env!("CARGO_TARGET_TMPDIR");
                        let _ = std::fs::write(format!("{d}/prep_inc_src.rs"), &cur);
                        let _ = std::fs::write(format!("{d}/prep_inc_full.rs"), &a);
                        let _ = std::fs::write(format!("{d}/prep_inc_inc.rs"), &b);
                        panic!("differs after {step} edits (fix same: {})", f1 == f2);
                    }
                    same += 1;
                }
                (Err(_), Err(_)) => {
                    errs += 1;
                    cur = prev.clone();
                }
                (a, b) => panic!("ok / err differ after {step} edits: full {:?} inc {:?}", a.err(), b.err()),
            }
            prev = cur.clone();
            cur = edit(&cur, &mut r);
        }
    }
    assert!(same > 500, "same {same}, errors {errs}");
}

/// An item after an attribute with a `]` in a string (`#[doc = "]"] fn inner`): the incremental preparation falls
/// back to the full one, and both agree after an edit elsewhere
#[test]
fn prep_inc_attr_with_bracket_string() {
    // (another fn's indented item makes the whole source hoist; the edit is in `outer`'s chunk)
    let base = "fn other() {\n    fn nested() {}\n    nested()\n}\nfn outer() -> u8 {\n    #[doc = \"]\"] fn inner() -> u8 { 7 } inner()\n}\nfn main() { other(); let _ = outer(); }\n";
    let edited = base.replace("inner()\n}", "let _q = 1; inner()\n}");
    assert_ne!(base, edited);
    let mut memo = PrepMemo::default();
    for src in [base.to_string(), edited] {
        let (mut f1, mut f2) = (vec![], vec![]);
        let full = prep_full(&src, &[], &mut f1, None).map(marked);
        let inc = prep(&src, &[], &mut f2, None, &mut memo);
        match (full, inc) {
            (Ok(a), Ok(b)) => assert!(a == b && f1 == f2, "{src}"),
            (Err(_), Err(_)) => {}
            (a, b) => panic!("ok / err differ: full {:?} inc {:?}", a.err(), b.err()),
        }
    }
}

/// Each source in turn through one memo: the incremental result is the full one's, and whether each call after the
/// first reused the chunks it kept (the first call preprocesses the whole and keeps nothing; the second keeps the
/// chunks; the third can reuse them)
fn through_memo(srcs: &[String]) -> Vec<bool> {
    through(PrepMemo::default(), srcs)
}

fn through(mut memo: PrepMemo, srcs: &[String]) -> Vec<bool> {
    let mut reused = vec![];
    for (i, src) in srcs.iter().enumerate() {
        let (mut f1, mut f2) = (vec![], vec![]);
        let full = prep_full(src, &[], &mut f1, None).map(marked);
        let inc = prep(src, &[], &mut f2, None, &mut memo);
        match (full, inc) {
            (Ok(a), Ok(b)) => assert!(a == b && f1 == f2, "source {i} differs:\n{src}"),
            (Err(_), Err(_)) => {}
            (a, b) => panic!("source {i}: ok / err differ: full {:?} inc {:?}", a.err(), b.err()),
        }
        if i > 0 {
            reused.push(memo.was_incremental());
        }
    }
    reused
}

/// What only looks like an item after a brace on its line (in a comment, a string, a one-line impl) does not stop the
/// incremental path, and an item that is one (in a fn body on its `{`'s line) is moved as the full preparation moves it
/// (#188: on zustc's own sources, every edit used to preprocess the whole)
#[test]
fn prep_inc_same_line_items() {
    let base = "\
// (a comment: `extern \"C\" { fn f(); }`)
struct T(u8);
impl T { fn get(&self) -> u8 { self.0 } }
fn label() -> &'static str { \"{ type X = u8; }\" }
fn helper() -> u8 {
    let t = T(1);
    t.get()
}
fn outer() -> u8 { fn inner() -> u8 { 7 } inner() + helper() }
fn main() { let _ = (outer(), label()); }
";
    let in_helper = base.replace("    t.get()\n", "    let _q = 2;\n    t.get()\n");
    let in_outer = in_helper.replace("inner() + helper()", "inner() + helper() + 1");
    let in_label = in_outer.replace("type X = u8;", "type X = u16;");
    let srcs: Vec<String> = [base, &in_helper, &in_outer, &in_label].iter().map(|s| s.to_string()).collect();
    for w in srcs.windows(2) {
        assert_ne!(w[0], w[1]);
    }
    assert_eq!(through_memo(&srcs), vec![false, true, true]);
}

/// A body whose `fn` type comes before a block (`let f: fn() -> u8 = || { 1 };`) and that holds an item: the item
/// stays in its skeleton (it once went with the closure's block, and with it the name another body's item collides
/// with: that one was not renamed after an edit elsewhere)
#[test]
fn prep_inc_fn_type_before_a_block() {
    let base = "\
fn a() -> usize {
    let f: fn() -> u8 = || { 1 };
    const K: usize = 1;
    K + f() as usize
}
fn b() -> usize {
    const K: usize = 2;
    K
}
fn main() { let _ = a() + b(); }
";
    let edited = base.replace("    K\n}", "    let _q = 1;\n    K\n}");
    let again = edited.replace("let _q = 1;", "let _q = 2;");
    assert!(base != edited && edited != again);
    let srcs = vec![base.to_string(), edited, again];
    assert_eq!(through_memo(&srcs), vec![false, true]);
}

/// keep_from_first (zust-analyzer's documents): the first call keeps the chunks, so the second reuses them
#[test]
fn prep_inc_keeps_from_the_first_call() {
    let base = "fn a() -> u8 {\n    1\n}\nfn b() -> u8 {\n    2\n}\nfn main() {\n    let _ = a() + b();\n}\n";
    let edited = base.replace("    2\n", "    let _q = 1;\n    2\n");
    let srcs = vec![base.to_string(), edited.clone()];
    let mut memo = PrepMemo::default();
    memo.keep_from_first();
    assert_eq!(through(memo, &srcs), vec![true]);
    // (without it, as a build: the first call keeps nothing)
    assert_eq!(through_memo(&srcs), vec![false]);
}

/// An item in a const's initializer (`const F: fn() -> u8 = { fn inner() .. inner };`) is moved out as a fn body's,
/// and its name counts with the other moved items' (`inner__h0`, `inner__h1`): the skeleton keeps such an
/// initializer, so an edit in another body (`other`) leaves the names as the full preparation gives them (once two
/// `fn inner__h0`)
#[test]
fn prep_inc_item_in_a_const_initializer() {
    let base = "fn inner() -> u8 { 0 }\nconst F: fn() -> u8 = { fn inner() -> u8 { 1 } inner };\nfn other() -> u8 { fn inner() -> u8 { 2 } inner() }\nfn main() { let _ = (F(), other()); }\n";
    let edited = base.replace("inner() }\nfn main", "inner() + 1 }\nfn main");
    let again = edited.replace("inner() + 1", "inner() + 2");
    assert!(base != edited && edited != again);
    let srcs = vec![base.to_string(), edited, again];
    assert_eq!(through_memo(&srcs), vec![false, true]);
}

/// A multi-line impl block: its chunks start inside it (each method a chunk), so a chunk's delimiters do not match
/// alone, the impl's `{` in one and its `}` in another. An edit in a method still reuses the other chunks
#[test]
fn prep_inc_reuses_chunks_in_an_impl_block() {
    let base = "struct T(u8);\nimpl T {\n    fn a(&self) -> u8 {\n        self.0\n    }\n    fn b(&self) -> u8 {\n        2\n    }\n}\nmod m {\n    pub fn c() -> u8 {\n        3\n    }\n}\nfn main() {\n    let _ = T(1).a() + T(1).b() + m::c();\n}\n";
    let edited = base.replace("        2\n", "        let q = 2;\n        q\n");
    let again = edited.replace("let q = 2;", "let q = 4;");
    assert!(base != edited && edited != again);
    let srcs = vec![base.to_string(), edited, again];
    assert_eq!(through_memo(&srcs), vec![false, true]);
}

/// An item after an attribute in a const's initializer (`{ #[inline] fn inner() .. }`) keeps the initializer in the
/// skeleton too
#[test]
fn prep_inc_attributed_item_in_a_const_initializer() {
    let base = "fn inner() -> u8 { 0 }\nconst F: fn() -> u8 = { #[inline] fn inner() -> u8 { 1 } inner };\nfn other() -> u8 { fn inner() -> u8 { 2 } inner() + 1 }\nfn main() { let _ = (F(), other()); }\n";
    let edited = base.replace("inner() + 1", "inner() + 3");
    let again = edited.replace("inner() + 3", "inner() + 4");
    let srcs = vec![base.to_string(), edited, again];
    assert_eq!(through_memo(&srcs), vec![false, true]);
}

/// A `macro_rules!` outside the fn bodies whose rules hold a fn with a body (`fn inner() -> i32 { 7 }`): the skeleton
/// keeps it as written, so a call of it in the changed chunk expands to that body (it used to expand to `{}`)
#[test]
fn prep_inc_macro_rules_with_a_fn_body() {
    let base = "macro_rules! val {\n    () => {{ fn inner(q: u8) -> i32 { 7 } inner(1) }};\n}\nfn other(a: i32) -> i32 {\n    a\n}\nfn main() {\n    let v = val!();\n}\n";
    let edited = base.replace("    let v = val!();\n", "    let z = 2;\n    let v = val!();\n");
    let again = edited.replace("let z = 2;", "let z = 3;");
    assert!(base != edited && edited != again);
    let srcs = vec![base.to_string(), edited, again];
    assert_eq!(through_memo(&srcs), vec![false, true]);
}

/// The skeletons blank the fn signatures outside the bodies (`fn name() {`): an edit in a body is still preprocessed as
/// the full preparation does it, beside generics with a `->` (`<F: Fn(u8) -> u8>`), a where clause, a struct pattern, a
/// parameter's attribute, a trait's declarations, a macro call that makes a fn, and a fn whose body keeps an item
/// (its signature stays). An edit of a signature changes the skeleton, as before (the whole is preprocessed)
#[test]
fn prep_inc_signatures_in_skeletons() {
    let base = "\
macro_rules! mk { ($n:ident) => { fn $n(a: u8) -> u8 { a + 1 } }; }
mk!(made);
struct P { x: u8, y: u8 }
trait Tr { fn m(&self) -> u8; fn d(&self) -> u8 { 1 } }
impl Tr for P { fn m(&self) -> u8 { self.x } }
fn apply<F: Fn(u8) -> u8>(f: F, v: u8) -> Vec<u8> where F: Copy { vec![f(v)] }
fn pat(P { x, y }: P) -> u8 { x + y }
fn attr(#[cfg(any())] q: u8, r: u8) -> u8 { r }
fn outer<T: Copy>(t: T) -> T { fn inner<U>(u: U) -> U { u } inner(t) }
fn edited() -> u8 {
    let a = apply(made, 1);
    a[0] + pat(P { x: 1, y: 2 }) + attr(3) + outer(4u8)
}
fn main() { let _ = edited(); }
";
    let in_body = base.replace("    let a = apply", "    let _q = 1;\n    let a = apply");
    let again = in_body.replace("let _q = 1;", "let _q = 2;");
    let in_sig = again.replace("r: u8) -> u8 { r }", "r: u8) -> u16 { r as u16 }");
    assert!(base != in_body && in_body != again && again != in_sig);
    assert_eq!(through_memo(&[base.to_string(), in_body, again, in_sig]), vec![false, true, false]);
}

/// A call in a body of a macro that makes no item (`sum!` calling itself) is blanked in the skeletons; one that may
/// make an item is kept: a macro with an item's keyword in its rules (`mk!`), one that calls such a macro
/// (`via!`), and a call of an item-free macro whose input has an item (`wrap!({ fn inner() .. })`, moved out of the
/// body after the expansion, and numbered with the others). An edit in another body is still preprocessed as the full
/// preparation does it, and an edit of a blanked call reuses the other chunks
#[test]
fn prep_inc_item_free_macro_calls() {
    let base = "\
macro_rules! sum { () => { 0u64 }; ($x:expr $(, $r:expr)*) => { $x + sum!($($r),*) }; }
macro_rules! mk { ($n:ident) => {{ fn $n() -> u64 { 1 } $n() }}; }
macro_rules! via { ($n:ident) => { mk!($n) }; }
macro_rules! wrap { ($e:expr) => { $e }; }
fn a() -> u64 {
    sum!(1, 2, 3, 4) + mk!(made) + via!(also)
}
fn b() -> u64 {
    wrap!({ fn inner() -> u64 { 2 } inner() })
}
fn c() -> u64 {
    let q = sum!(5, 6);
    q
}
fn main() { let _ = a() + b() + c(); }
";
    let in_c = base.replace("    q\n}", "    let _z = 1;\n    q\n}");
    let again = in_c.replace("let _z = 1;", "let _z = 2;");
    let in_a = again.replace("sum!(1, 2, 3, 4)", "sum!(1, 2, 3, 4, 5)");
    assert!(base != in_c && in_c != again && again != in_a);
    assert_eq!(through_memo(&[base.to_string(), in_c, again, in_a]), vec![false, true, true]);
}
