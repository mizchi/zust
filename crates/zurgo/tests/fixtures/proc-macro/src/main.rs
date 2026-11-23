use pm::{docs, make_fn, sum};
use pm::sum as total;
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

/**
 * A doc comment
 * over lines
 */
#[docs]
struct Multi;

const NINE: u32 = pm::add!(1, 2) * 3;

fn answer() -> u32 {
    return ::pm::add!(40, 2);
}

// (a macro_rules! expansion that calls a proc macro under another name)
macro_rules! via_alias {
    ($($e:expr),*) => {
        total!($($e),*)
    };
}

fn main() {
    println!("{} {}", seven(), seven_scaled());
    let p = Point { x: 1, y: 2, z: 3 };
    let q = p.clone();
    println!("{} {} {}", Point::describe(), Empty::describe(), q.x + q.y + q.z);
    println!("docs {}", DOCS_POINT);
    let s = sum!(1, 2, 3) * 2;
    println!("{s} {}", forty_two());
    println!("{}", pm::pkg!());
    println!("{} {} {} {}", DOCS_MULTI, NINE, answer(), via_alias!(1, 2, 3));
    let _ = Multi;
    println!("{}", pm::shared!());
}
