// `x.fmt(f)` takes the fmt trait in scope at the call (#256, review of #280): with only `use std::fmt::Debug;`
// imported, a str's, a number's and an Option's fmt outside an impl of Display / Debug is Debug's (`"text"` quoted),
// not Display's. (With both traits in scope rustc rejects the call, E0034: no program to compare.) The import's `;` in
// a comment is not its end
use std::fmt::{self, /* ; */ Debug};

fn show(s: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    s.fmt(f)
}

fn num(n: u32, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    n.fmt(f)
}

struct W(&'static str, u32);

impl W {
    fn parts(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        show(self.0, f)?;
        f.write_str(" ")?;
        num(self.1, f)?;
        f.write_str(" ")?;
        Some(self.1).fmt(f)
    }
}

impl fmt::Display for W {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.parts(f)
    }
}

impl fmt::Debug for W {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

fn main() {
    let w = W("text", 7);
    println!("{w}");
    println!("[{w:?}] [{}]", W("ab", 1));
}
