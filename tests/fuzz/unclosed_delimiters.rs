// Delimiters left open or closed twice, in items, bodies, macro definitions and attributes.
macro_rules! m { ($x:expr) => { ($x }; }
fn f() { let v = [1, 2; }
#[cfg(any(unix, ]
struct S { a: u8,
fn g() -> u8 { m!(1)) }
