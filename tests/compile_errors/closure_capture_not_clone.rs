// A closure moving in a capture that is not Clone is not Clone
#[derive(Debug)]
struct NoClone;
fn main() {
    let z = NoClone;
    let k = move || println!("{z:?}");
    let k2 = k.clone();
    k2();
}
