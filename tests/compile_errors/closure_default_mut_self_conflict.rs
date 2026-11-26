// (a closure that calls a trait's default `&mut self` method on what it captures borrows it mutably while it lives)
trait Bump {
    fn inc(&mut self);
    fn bump(&mut self) {
        self.inc();
    }
}
struct C(Vec<i32>);
impl Bump for C {
    fn inc(&mut self) { self.0.push(1) }
}
fn main() {
    let mut c = C(vec![]);
    let mut f = || c.bump();
    println!("{}", c.0.len());
    f();
}
