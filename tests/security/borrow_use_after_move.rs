//! Use after move (E0382): the moved-from String's buffer was freed by its new owner.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn consume(s: String) -> usize {
    s.len()
}
fn main() {
    let s = String::from("hello");
    let n = consume(s);
    println!("{} {}", n, s); //~ ERROR E0382
}
