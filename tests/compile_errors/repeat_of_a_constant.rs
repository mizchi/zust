// (a constant is evaluated for each element; one copy needs no Copy)
const EMPTY: Option<String> = None;
fn main() {
    let a = [EMPTY; 3];
    let b = [String::new(); 1];
    let c: u8 = a.len();
    let _ = b;
}
