// (a std path to a prelude type the program shadows is the renamed name padded to its length: the columns after it
// on its line stay where rustc puts them)
pub struct Duration(i64);
fn main() {
    let d = Duration(1);
    let s = std::time::Duration::from_secs(1).as_secs() + missing;
    println!("{} {}", d.0, s);
}
