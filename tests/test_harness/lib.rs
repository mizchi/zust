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
    mod inner {
        #[test]
        fn deep() { println!("deep out"); }
    }
}
