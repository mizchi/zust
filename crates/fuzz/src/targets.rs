//! What a fuzz input is run through. Every target must return (an answer or an error) for any input: a panic, a
//! hang or a crash is a finding. `expand-diff` also compares the fast `macro_rules!` expander with the reference one.

use zust_core::session::{Options, Session};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// the parser's tokenizer, the item splitter and the lexer's token scan
    Lex,
    /// the parser (`parse::parse_file`)
    Parse,
    /// the `macro_rules!` expander
    Expand,
    /// the fast expander against the reference one (`tests/macros_expand.rs`'s comparison)
    ExpandDiff,
    /// the preprocessing (cfg, macros, module flattening, items moved out of bodies)
    Prep,
    /// `zustc check`: everything up to the type, borrow and region checks, in process
    Check,
}

pub const ALL: [Target; 6] = [Target::Lex, Target::Parse, Target::Expand, Target::ExpandDiff, Target::Prep, Target::Check];

impl Target {
    pub fn parse(s: &str) -> Option<Target> {
        ALL.iter().copied().find(|t| t.name() == s)
    }

    pub fn name(self) -> &'static str {
        match self {
            Target::Lex => "lex",
            Target::Parse => "parse",
            Target::Expand => "expand",
            Target::ExpandDiff => "expand-diff",
            Target::Prep => "prep",
            Target::Check => "check",
        }
    }
}

/// The result of one run that returned: a short feature of the answer (the start of the first error), which the
/// fuzzer uses to keep inputs that reach something new, or a mismatch between two implementations.
pub enum Outcome {
    Done(String),
    Mismatch(String),
}

/// The leading words of a message, without positions and names: "12:5: expected `;`, found `x`" -> "expected".
fn feature(msg: &str) -> String {
    let m = msg.trim_start_matches(|c: char| c.is_ascii_digit() || c == ':' || c == ' ');
    let m = m.split('`').next().unwrap_or(m);
    m.split_whitespace().take(4).collect::<Vec<_>>().join(" ")
}

pub fn run(t: Target, src: &str, edition: u16) -> Outcome {
    match t {
        Target::Lex => {
            let a = zust_core::parse::tokenize(src).err().map(|e| feature(&e));
            let b = zust_core::lexer::split(src).err().map(|e| feature(&e));
            let n = zust_core::lexer::tokenize(src.as_bytes()).len();
            Outcome::Done(format!("{a:?} {b:?} {}", n.min(1)))
        }
        Target::Parse => Outcome::Done(match zust_core::parse::parse_file(src) {
            Ok(f) => format!("ok {}", f.items.len().min(1)),
            Err(e) => feature(&e),
        }),
        Target::Expand => Outcome::Done(match zust_core::macros::expand(src) {
            Ok(Some(_)) => "expanded".into(),
            Ok(None) => "none".into(),
            Err(e) => feature(&e),
        }),
        Target::ExpandDiff => {
            if !src.contains("macro_rules!") {
                return Outcome::Done("no macro".into());
            }
            let a = zust_core::macros::expand(src);
            let b = zust_core::macros::expand_reference(src);
            match (&a, &b) {
                // (the reference reads the whole source with proc_macro2 first, so it cannot answer for a source that
                // does not tokenize (an unbalanced delimiter, an unterminated literal); the fast one may)
                (_, Err(y)) if y.contains("cannot parse string into token stream") => Outcome::Done("not tokens".into()),
                (Ok(x), Ok(y)) if x == y => Outcome::Done("same".into()),
                (Err(x), Err(y)) if x.split(':').take(2).eq(y.split(':').take(2)) => Outcome::Done(feature(x)),
                _ => Outcome::Mismatch(format!(
                    "fast {:?} vs reference {:?}",
                    a.as_ref().map(|o| o.as_ref().map(|s| s.len())),
                    b.as_ref().map(|o| o.as_ref().map(|s| s.len()))
                )),
            }
        }
        Target::Prep => Outcome::Done(match zust_core::prep::prep_full(src, &[], &mut vec![], None) {
            Ok(_) => "ok".into(),
            Err(e) => feature(&e),
        }),
        Target::Check => {
            let mut s = Session::new(Options { jobs: 1, no_cache: true, edition, ..Options::default() });
            let d = s.check(src);
            Outcome::Done(d.first().map_or_else(|| "clean".to_string(), |d| feature(&d.msg)))
        }
    }
}
