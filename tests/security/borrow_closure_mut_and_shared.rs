//! A closure holding `&mut` while another use reads the same value (E0502).
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn main() {
    let mut v = vec![1];
    let mut push = || v.push(2);
    println!("{}", v.len()); //~ ERROR E0502
    push();
}
