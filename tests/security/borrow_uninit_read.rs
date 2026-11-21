//! Reading a possibly uninitialized variable (E0381).
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ todo: rejected, but reported at the function's first line instead of the read
//@ expect: reject
fn main() {
    let x: i32;
    if std::env::args().count() > 5 {
        x = 1;
    }
    println!("{x}"); //~ ERROR E0381
}
