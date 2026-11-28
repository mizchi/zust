// std's `fmt::Result` imported by its own name after a `use` that a newline or a tab follows, not a space (#260):
// still fmt's alias, which has no parameters
use
    core::fmt::Result;
use	std::fmt::{Display, Formatter};

struct P(u8);

impl Display for P {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "P{}", self.0)
    }
}

fn check(ok: bool) -> Result {
    if ok { Ok(()) } else { Err(core::fmt::Error::default()) }
}

fn main() {
    println!("{} {:?} {:?}", P(1), check(true), check(false));
}
