// `S2::Line(3)` / `K2::C` / `<K2 as Tr>::T`: variants and associated consts through a type alias
enum Shape {
    Dot,
    Line(u8),
    Rect { w: u8, h: u8 },
}
type S2 = Shape;
struct K;
impl K {
    const C: u8 = 9;
}
type K2 = K;
trait Tr {
    const T: u8;
}
impl Tr for K {
    const T: u8 = 4;
}
fn area(s: &S2) -> u8 {
    match s {
        S2::Dot => 0,
        S2::Line(n) => *n,
        S2::Rect { w, h } => w * h,
    }
}
fn main() {
    let v = [S2::Dot, S2::Line(3), S2::Rect { w: 2, h: 5 }];
    let total: u8 = v.iter().map(area).sum();
    println!("{} {} {}", total, K2::C, <K2 as Tr>::T);
}
