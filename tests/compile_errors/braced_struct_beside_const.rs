// (a struct with braces is a type only: a const of its name is no clash)
#[allow(non_upper_case_globals)]
const Point: i32 = 3;
struct Point {
    x: i32,
}
fn main() {
    let p = Point { x: Point };
    let b: bool = p.x;
}
