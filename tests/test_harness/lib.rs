pub fn add(a: i32, b: i32) -> i32 { a + b }

fn main() { println!("real main"); }

#[cfg(not(test))]
fn only_not_test() {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adds() { assert_eq!(add(2, 2), 4); }
    #[test]
    fn fails() { assert_eq!(add(2, 2), 5); }
    #[test]
    #[should_panic(expected = "bo om")]
    fn panics() { panic!("bo om {}", 1); }
    #[test]
    #[should_panic(expected = "nope")]
    fn panics_wrong() { panic!("other"); }
    #[test]
    #[ignore]
    fn slow() {}
    #[test]
    fn result() -> Result<(), String> { Err("bad".into()) }
    // (the panic's message is what `expected` matches, not what the test printed before it)
    #[test]
    #[should_panic(expected = "wanted")]
    fn printed_then_panicked() { eprintln!("wanted"); panic!("different"); }
    #[test]
    #[should_panic(expected = r"a\b")]
    fn raw_expected() { panic!(r"x a\b y"); }
    #[test]
    #[should_panic(expected = "\u{41}B")]
    fn escaped_expected() { panic!("xABy"); }
    // (the crate's own `main`, which the harness renamed)
    #[test]
    fn calls_main() { main(); super::main(); }
    mod inner {
        #[test]
        fn deep() { println!("deep out"); }
    }
}
