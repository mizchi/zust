// (a closure that calls a trait's default `&mut self` method on what it captures is FnMut: calling it needs `mut f`)
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
    let f = || c.bump();
    f();
    println!("{}", c.0.len());
}
