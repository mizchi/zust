// A macro call in a fn whose signature writes `impl Trait` (return or argument position) is an expression: it keeps
// its own value in `m!() * 3`, and so is one in a braced const argument of an impl's head (`impl C<{ m!() * 3 }>`).
// In an impl block's body (plain, `unsafe impl`, with an attribute) a call writes items
macro_rules! sum {
    () => {
        1 + 2
    };
}
macro_rules! getter {
    ($name:ident, $v:expr) => {
        fn $name(&self) -> u32 {
            $v
        }
    };
}
fn ret() -> impl std::fmt::Display {
    sum!() * 3
}
fn arg(x: impl std::fmt::Display) -> String {
    format!("{x} {}", sum!() * 3)
}
struct S;
impl S {
    getter!(one, 1);
}
#[allow(dead_code)]
impl S {
    getter!(two, sum!() * 3);
}
unsafe trait Marker {
    fn mark(&self) -> u32;
}
unsafe impl Marker for S {
    getter!(mark, 4);
}
struct C<const N: usize>;
impl C<{ sum!() * 3 }> {
    fn nine(&self) -> usize {
        9
    }
}
fn size<const N: usize>(_: &C<N>) -> usize {
    N
}
fn main() {
    let c: C<{ sum!() * 3 }> = C;
    println!("{} {}", size(&c), c.nine());
    println!("{} {}", ret(), arg("a"));
    println!("{} {} {}", S.one(), S.two(), S.mark());
}
