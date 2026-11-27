// Which fmt traits a program imports, for `x.fmt(f)` (#256, #280's review): a glob of another module (`ops::*`) in
// the same tree as `fmt::{Debug}` imports no Display, and `io::Error as Display` is io::Error, not the trait. So
// `s.fmt(f)` of a str is Debug's
use std::io::Error as Display;
use std::{fmt::{self, Debug}, ops::*};

fn show(s: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    s.fmt(f)
}

struct W;

impl fmt::Display for W {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        show("text", f)
    }
}

fn main() {
    let code = Display::from_raw_os_error(1).raw_os_error();
    let r: Range<u8> = 0..2;
    println!("{} {:?} {:?}", W, r, code);
}
