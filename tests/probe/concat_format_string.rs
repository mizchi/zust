// chacha20 0.10 (rand's StdRng): a format string made by concat! / stringify! in a macro, and concat! with a
// nested stringify!
use std::fmt;
struct R;
macro_rules! imp { ($R:ident) => { impl fmt::Debug for $R { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, concat!(stringify!($R), " {{ ... }}")) } } }; }
imp!(R);
fn main() {
    println!("{:?}", R);
    println!(concat!("a", "-{}"), 1);
    let s = format!(concat!(stringify!(xy), "={}"), 2);
    println!("{s}");
}
