//! Whether the incremental split (front::split_inc) is the same as splitting the whole every time.
//! To real sources (crates/{syntax,sema,core}/src/*.rs and examples/*.rs), keeps applying random edits (inserting / deleting characters, opening quotes or comments,
//! adding / removing fns), comparing at each step

use zustc::front::split_inc;
use zustc::lexer::{self, SrcItem};

fn same(a: &[SrcItem], b: &[SrcItem]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.start == y.start && x.end == y.end && x.line == y.line && x.hash == y.hash && x.idents == y.idents && x.kind == y.kind
        })
}

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
    let at = char_floor(s, r.below(s.len() + 1));
    let snippets = [
        "x", " ", "\n", "}", "{", "\"", "'", "/*", "*/", "//", "#", ";", "1 + 2", "fn added() {}\n", "\nfn f2(a: u32) -> u32 { a + 1 }\n",
        "impl S { fn m(&self) {} }\n", "r#\"", "b'x'", "'a", "\n#[derive(Clone)]\nstruct Q;\n",
    ];
    match r.below(3) {
        // Insertion
        0 | 1 => {
            let t = snippets[r.below(snippets.len())];
            format!("{}{}{}", &s[..at], t, &s[at..])
        }
        // Deletion
        _ => {
            let end = char_floor(s, (at + 1 + r.below(40)).min(s.len()));
            let end = end.max(at);
            format!("{}{}", &s[..at], &s[end..])
        }
    }
}

#[test]
fn split_inc_matches_full_split() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut files: Vec<String> = vec![];
    for dir in ["crates/syntax/src", "crates/sema/src", "crates/core/src", "examples"] {
        let mut ps: Vec<_> = std::fs::read_dir(format!("{root}/{dir}")).unwrap().map(|e| e.unwrap().path()).collect();
        ps.sort();
        for p in ps {
            if p.extension().is_some_and(|e| e == "rs") {
                files.push(std::fs::read_to_string(&p).unwrap());
            }
        }
    }
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut checked, mut errs) = (0, 0);
    for src in files.iter().take(40) {
        let mut memo = None;
        let mut cur = src.clone();
        let mut prev = src.clone();
        for step in 0..60 {
            let full = lexer::split(&cur);
            let memo_txt = memo.as_ref().map(|m: &(String, Vec<SrcItem>)| m.0.clone()).unwrap_or_default();
            let inc = split_inc(&cur, &mut memo);
            match (full, inc) {
                (Ok(f), Ok(i)) => {
                    if !same(&f, &i) {
                        let d = env!("CARGO_TARGET_TMPDIR");
                        let _ = std::fs::write(format!("{d}/split_inc_prev.rs"), &memo_txt);
                        let _ = std::fs::write(format!("{d}/split_inc_cur.rs"), &cur);
                    }
                    assert!(same(&f, &i), "differs after {step} edits");
                    checked += 1;
                }
                (Err(_), Err(_)) => {
                    // (an edit that cannot be split is undone: try many edits to valid sources)
                    errs += 1;
                    cur = prev.clone();
                }
                (f, i) => panic!("ok / err differ after {step} edits: full {:?} inc {:?}", f.map(|v| v.len()), i.map(|v| v.len())),
            }
            // (on a failure, memo stays at the previous success: the next edit is a diff from there)
            prev = cur.clone();
            cur = edit(&cur, &mut r);
        }
    }
    assert!(checked > 1000, "checked {checked}, errors {errs}");
}
