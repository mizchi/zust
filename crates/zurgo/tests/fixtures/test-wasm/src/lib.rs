pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adds() {
        assert_eq!(add(2, 2), 4);
    }
    #[test]
    fn fails() {
        assert_eq!(add(2, 2), 5, "on purpose");
    }
    // (an explicit panic: in a wasm32 module, an arithmetic check traps without a message)
    #[test]
    #[should_panic(expected = "too big")]
    fn panics_right() {
        if add(2, 2) > 3 {
            panic!("too big: {}", add(2, 2));
        }
    }
    #[test]
    #[should_panic(expected = "not this")]
    fn panics_wrong() {
        panic!("something else");
    }
    #[test]
    fn returns_err() -> Result<(), String> {
        Err("an Err".into())
    }
    #[test]
    #[ignore]
    fn ignored() {}
}
