//! Returning a reference that borrows from an argument not named in the signature (E0621 / lifetime
//! may not live long enough): the caller trusts the signature, so the reference would dangle.
//@ expect: reject
fn pick<'a, 'b>(x: &'a str, y: &'b str) -> &'a str {
    if x.len() > 100 { x } else { y } //~ ERROR lifetime may not live long enough
}
fn main() {
    let a = String::from("a");
    let r;
    {
        let b = String::from("b");
        r = pick(&a, &b);
    }
    println!("{r}");
}
