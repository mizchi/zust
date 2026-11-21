//! Moving a value twice (E0382) would drop it twice: a double free.
//@ expect: reject
fn main() {
    let b = Box::new(5);
    let c = b;
    let d = b; //~ ERROR E0382
    println!("{} {}", c, d);
}
