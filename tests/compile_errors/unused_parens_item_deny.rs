#[deny(unused_parens)]
fn f(x: i32) -> i32 {
    if (x > 3) { return (x); }
    let y = (x + 1);
    match (y) { _ => {} }
    while (false) {}
    y
}
fn main() { println!("{}", f(1)); }
