// A result `impl Display + 'a` holds the borrow of the argument of lifetime 'a (edition 2021: the lifetimes its bounds
// name; 2024 also the elided ones, tests/ui impl-trait/precise-capturing/migration-note.rs)
use std::fmt::Display;
fn dl<'a>(x: &'a Vec<u8>) -> impl Display + 'a {
    x.len()
}
fn main() {
    let mut x: Vec<u8> = vec![];
    let a = dl(&x);
    x.push(1);
    println!("{a}");
}
