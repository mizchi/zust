// `x.fmt (f)` with a space before its arguments, the program's only `fmt` call (#280's review: the prelude's helper
// for it was selected only where the method's name had its parenthesis right after it), with the formatter's flags.
// (No comment here may write the name and the parenthesis together: the selection reads comments too)
use std::fmt;

struct W(u8);

impl fmt::Display for W {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt (f)
    }
}

fn main() {
    println!("[{:>4}] [{}]", W(7), W(42));
}
