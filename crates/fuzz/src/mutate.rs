//! Mutations of Rust source, on a rough token split (identifiers and numbers, whitespace runs, single punctuation),
//! so most mutants are still close to Rust and get past the lexer into the parser, the expander and the checker.

use crate::rng::Rng;

/// Tokens and fragments that reach the corners of zustc's front end: delimiters, literals and their prefixes,
/// comments, attributes, macro matchers, generics, lifetimes, keywords.
const DICT: &[&str] = &[
    "{", "}", "(", ")", "[", "]", "<", ">", "::", "->", "=>", "..", "..=", "...", "<<=", ">>=", "&&", "||", "?", "!", "#",
    "$", "@", "~", ";", ",", ":", "'", "\"", "r#\"", "\"#", "b'", "b\"", "c\"", "r#", "'a", "'static", "'_", "/*", "*/", "//",
    "///", "//!", "/** */", "#[", "#![", "#[cfg(", "#[derive(", "#[test]", "#[cfg(false)]", "cfg!(", "macro_rules!", "$x:expr",
    "$t:tt", "$($t:tt)*", "$($x:ident),*", "$crate", "fn", "let", "mut", "ref", "const", "static", "struct", "enum", "union",
    "trait", "impl", "impl<T>", "for", "where", "dyn", "?Sized", "Self", "self", "super", "crate", "mod", "use", "pub",
    "pub(crate)", "extern", "extern \"C\"", "unsafe", "async", "await", "move", "match", "if", "else", "loop", "while",
    "break", "continue", "return", "yield", "type", "as", "in", "box", "0", "1", "-1", "0x", "0b2", "1e309", "1_000u8",
    "255u8", "i128::MAX", "'\\u{110000}'", "'\\n'", "\"\\\"", "1.0f32", "usize", "u8", "&", "&mut", "*const", "*mut",
    "Box<dyn Fn()>", "Vec<_>", "Option<T>", "T: Clone", "<T as Tr>::A", "|x| x", "move ||", "..Default::default()",
    "println!(\"{}\", ", "format!(\"{:?}\", ", "vec![", "assert!(", "panic!(", "unreachable!()", "todo!()", "\n", "\t",
];

/// The tokens of a source: runs of identifier characters, runs of whitespace, and every other character alone.
pub fn tokens(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut kind = 0u8;
    for (i, c) in s.char_indices() {
        let k = if c.is_alphanumeric() || c == '_' { 1 } else if c.is_whitespace() { 2 } else { 3 };
        if i > start && (k != kind || k == 3) {
            out.push(&s[start..i]);
            start = i;
        }
        kind = k;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// One mutant of `src`: 1 to 4 mutations, some of which take a piece of `other` (another corpus file).
pub fn mutate(rng: &mut Rng, src: &str, other: &str, max_len: usize) -> String {
    let mut toks: Vec<String> = tokens(src).into_iter().map(str::to_string).collect();
    let donor = tokens(other);
    for _ in 0..1 + rng.below(4) {
        let n = toks.len();
        let at = if n == 0 { 0 } else { rng.below(n + 1) };
        let span = |rng: &mut Rng, len: usize| 1 + rng.below(len.clamp(1, 12));
        match rng.below(10) {
            // (delete a range)
            0 | 1 if n > 0 => {
                let a = at.min(n - 1);
                let b = (a + span(rng, n - a)).min(n);
                toks.drain(a..b);
            }
            // (duplicate a range)
            2 if n > 0 => {
                let a = at.min(n - 1);
                let b = (a + span(rng, n - a)).min(n);
                let piece: Vec<String> = toks[a..b].to_vec();
                let to = rng.below(toks.len() + 1);
                toks.splice(to..to, piece);
            }
            // (insert, or replace with, a token of the dictionary)
            3 | 4 => toks.insert(at, rng.pick(DICT).to_string()),
            5 if n > 0 => toks[at.min(n - 1)] = rng.pick(DICT).to_string(),
            // (swap two tokens)
            6 if n > 1 => {
                let (a, b) = (rng.below(n), rng.below(n));
                toks.swap(a, b);
            }
            // (splice in a range of another file)
            7 | 8 if !donor.is_empty() => {
                let a = rng.below(donor.len());
                let b = (a + 1 + rng.below(64)).min(donor.len());
                toks.splice(at..at, donor[a..b].iter().map(|t| t.to_string()));
            }
            // (wrap a range in a delimiter pair, or leave one half of it)
            _ if n > 0 => {
                let (o, c) = *rng.pick(&[("{", "}"), ("(", ")"), ("[", "]"), ("<", ">"), ("/*", "*/"), ("\"", "\"")]);
                let a = at.min(n - 1);
                let b = (a + span(rng, n - a)).min(n);
                if !rng.one_in(3) {
                    toks.insert(b, c.to_string());
                }
                toks.insert(a, o.to_string());
            }
            _ => toks.insert(at, rng.pick(DICT).to_string()),
        }
    }
    let mut out: String = toks.concat();
    if out.len() > max_len {
        let mut cut = max_len;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// The smallest source found by removing token ranges (and then lines) while `still` holds: delta debugging, at
/// most `budget` calls of `still`.
pub fn minimize(src: &str, budget: usize, still: &mut dyn FnMut(&str) -> bool) -> String {
    let mut cur: Vec<String> = tokens(src).into_iter().map(str::to_string).collect();
    let mut calls = 0;
    let mut chunk = (cur.len() / 2).max(1);
    while chunk >= 1 && calls < budget {
        let mut i = 0;
        let mut removed = false;
        while i < cur.len() && calls < budget {
            let end = (i + chunk).min(cur.len());
            let cand: String = cur[..i].concat() + &cur[end..].concat();
            calls += 1;
            if still(&cand) {
                cur.drain(i..end);
                removed = true;
            } else {
                i += chunk;
            }
        }
        if !removed {
            if chunk == 1 {
                break;
            }
            chunk /= 2;
        }
    }
    cur.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_cover_the_source() {
        let s = "fn main() { let x_1 = 2; }\n";
        let t = tokens(s);
        assert_eq!(t.concat(), s);
        assert_eq!(t[..4], ["fn", " ", "main", "("]);
    }

    #[test]
    fn mutants_are_deterministic_and_bounded() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..200 {
            let x = mutate(&mut a, "fn f() -> u8 { 1 }", "struct S;", 40);
            assert_eq!(x, mutate(&mut b, "fn f() -> u8 { 1 }", "struct S;", 40));
            assert!(x.len() <= 40);
        }
    }

    #[test]
    fn minimize_keeps_what_the_predicate_needs() {
        let m = minimize("fn a() {} fn bad() {} fn c() {}", 500, &mut |s| s.contains("bad"));
        assert_eq!(m, "bad");
    }
}
