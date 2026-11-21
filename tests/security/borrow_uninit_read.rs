//! Reading a possibly uninitialized variable (E0381).
//@ expect: reject
fn main() {
    let x: i32;
    if std::env::args().count() > 5 {
        x = 1;
    }
    println!("{x}"); //~ ERROR E0381
}
