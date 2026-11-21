//! A reference to a temporary kept past the statement (E0716).
//@ expect: reject
fn main() {
    let r: &String;
    r = &String::from("temp"); //~ ERROR E0716
    println!("{r}");
}
