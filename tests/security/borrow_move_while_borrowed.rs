//! Moving out of a value while it is borrowed (E0505).
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn main() {
    let s = String::from("x");
    let r = &s;
    let t = s; //~ ERROR E0505
    println!("{r} {t}");
}
