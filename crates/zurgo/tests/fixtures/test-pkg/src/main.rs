fn greet(n: &str) -> String { format!("hello {n}") }
fn main() { println!("{}", greet("world")); }

#[test]
fn greets() { assert_eq!(greet("x"), "hello x"); }
