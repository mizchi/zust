use pm::{docs, make_fn, sum};
use re::Describe;

#[re::scaled(3)]
fn seven() -> u32 {
    7
}

/// The point
/// of it all
#[docs]
#[derive(Clone, Describe)]
struct Point {
    x: i32,
    #[describe(skip)]
    y: i32,
    z: i32,
}

#[derive(Describe)]
struct Empty {}

make_fn!(forty_two, 42);

fn main() {
    println!("{} {}", seven(), seven_scaled());
    let p = Point { x: 1, y: 2, z: 3 };
    let q = p.clone();
    println!("{} {} {}", Point::describe(), Empty::describe(), q.x + q.y + q.z);
    println!("docs {}", DOCS_POINT);
    let s = sum!(1, 2, 3) * 2;
    println!("{s} {}", forty_two());
    println!("{}", pm::pkg!());
}
