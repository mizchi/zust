#[derive(Debug)]
struct NoClone;
fn main() {
    let values = [NoClone];
    let c = move || println!("{values:?}");
    let r = &c;
    let cloned = r.clone();
    let _ = cloned;
}
