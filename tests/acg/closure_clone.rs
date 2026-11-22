// A closure is Clone when its captures are, whatever its signature: one taking and returning a type that is not Clone,
// one borrowing a capture that is not Clone, one moving a Copy capture
struct NoClone(u8);
#[derive(Debug)]
struct NoCloneUpvar(u8);
fn main() {
    let f = |x: NoClone| -> NoClone { NoClone(x.0 + 1) };
    let g = f.clone();
    println!("{} {}", f(NoClone(1)).0, g(NoClone(2)).0);
    let x = NoCloneUpvar(7);
    let h = || println!("{x:?}");
    let h2 = h.clone();
    h();
    h2();
    let n = 5u8;
    let m = move || n + 1;
    let m2 = m.clone();
    println!("{} {}", m(), m2());
}
