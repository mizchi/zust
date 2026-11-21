//! Writing a short-lived reference through `&mut &'static str` (lifetime may not live long enough):
//! `&mut T` is invariant in `T`, otherwise the caller keeps a dangling `&'static str`.
//@ expect: reject
fn evil<'a>(x: &mut &'static str, y: &'a str) {
    *x = y; //~ ERROR lifetime may not live long enough
}
fn main() {
    let mut s: &'static str = "static";
    {
        let owned = String::from("temporary");
        evil(&mut s, &owned);
    }
    println!("{s}");
}
