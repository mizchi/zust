// std's Borrow is not in the prelude (#261): with no `use` of it, and no bound naming it, a String has no method
// `borrow` (E0599)
fn main() {
    let s = String::from("a");
    let r: &str = s.borrow();
    println!("{}", r);
}
