mod cli;
use geom::{P, Shape};
use geom::shape::Rect;
fn helper() -> u32 { 1 }
struct Sq(i64);
impl Shape for Sq { fn area(&self) -> i64 { self.0 * self.0 } fn name(&self) -> String { "sq".to_string() } }
fn main() {
    let p = P::new(3, -4).scaled();
    println!("{} {:?}", p, p);
    println!("{} {} {}", helper(), geom::helper(), util::pick(&[10, 20, 30], 1));
    let v: Vec<Box<dyn Shape>> = vec![Box::new(Rect { w: 2, h: 3 }), Box::new(Sq(4))];
    println!("{} {}", geom::shape::total(&v), v[1].name());
    // (`::util::pick`: a path from the extern crate)
    println!("{}", ::util::pick(&[7, 8], 0));
    println!("calls {}", util::calls());
    // (5 arguments: an out-of-range index inside a generic fn of a dependency crate (the location is in that crate's file))
    if std::env::args().count() == 6 {
        println!("{}", util::pick(&[1u8, 2, 3], 9));
    }
    cli::run(std::env::args().count() as i64 - 1);
}
