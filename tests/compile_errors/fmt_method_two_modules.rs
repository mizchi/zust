// Two modules, one importing Debug, the other Display (#256, #280): zustc's fmt imports are the program's, not each
// module's, so it does not know which trait `s.fmt(f)` in the Debug module sees. It finds no method rather than pick
// one, which could be the wrong one. (rustc builds this and prints `"a" 1`; the per-module scope is a follow-up)
mod dbg {
    use std::fmt::{self, Debug};
    pub fn show(s: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        s.fmt(f)
    }
}

mod disp {
    use std::fmt::Display;
    pub fn text<T: Display>(t: T) -> String {
        format!("{t}")
    }
}

struct W;

impl std::fmt::Display for W {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        dbg::show("a", f)
    }
}

fn main() {
    println!("{} {}", W, disp::text(1));
}
