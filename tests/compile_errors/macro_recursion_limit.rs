// A macro that calls itself without end: the error names it, at the call its chain started from
macro_rules! count {
    () => { 0 };
    ($x:tt $($rest:tt)*) => { 1 + count!($x $($rest)*) };
}

macro_rules! outer {
    ($($t:tt)*) => { count!($($t)*) };
}

fn main() {
    let n = 2;
    let m = outer!(a b c);
    println!("{n} {m}");
}
