// An argument's `#[inline]` / lint attributes are taken (itertools), but `#[track_caller]` on a closure is not (rustc:
// E0658, unstable)
fn apply(x: u8, f: impl Fn(u8) -> u8) -> u8 {
    f(x)
}

fn main() {
    let a = apply(1, #[inline(always)] |v| v + 1);
    let b = [#[allow(unused)] 2, 3];
    let c = apply(
        a,
        #[allow(unused)]
        #[track_caller]
        |v| v,
    );
    println!("{} {:?} {}", a, b, c);
}
