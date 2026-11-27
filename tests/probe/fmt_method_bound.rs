// `x.fmt(f)` of a type parameter's value takes the trait of its bound (#256, #280's review): with only Display
// imported, `T: fmt::Debug` writes `1.0` and `"s"`, and `T: Display` writes `1` and `s`
use std::fmt::{self, Display};

fn show<T: fmt::Debug>(x: T, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    x.fmt(f)
}

fn text<T: Display>(x: &T, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    x.fmt(f)
}

struct W(f64, &'static str);

impl fmt::Display for W {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        show(self.0, f)?;
        f.write_str(" ")?;
        show(self.1, f)?;
        f.write_str(" ")?;
        text(&self.0, f)?;
        f.write_str(" ")?;
        text(&self.1, f)
    }
}

fn main() {
    println!("{}", W(1.0, "s"));
}
