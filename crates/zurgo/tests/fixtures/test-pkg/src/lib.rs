// (a fn written inside another: hoisted out of it, still the lib's own (its .o has it))
pub fn add(a: i32, b: i32) -> i32 {
    fn sum(x: i32, y: i32) -> i32 { x + y }
    sum(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adds() {
        fn check(x: i32) { assert_eq!(x, 4); }
        check(add(2, 2));
    }
    #[test]
    fn with_dev_dep() { assert_eq!(helper::twice(add(1, 2)), 6); }
    #[test]
    #[should_panic(expected = "not this")]
    fn panics_wrong() { panic!("something else") }
    #[test]
    fn returns_err() -> Result<(), String> { if add(1, 1) == 2 { Err("an Err".into()) } else { Ok(()) } }
    #[test]
    #[should_panic(expected = "overflow")]
    fn overflows() { let x: i32 = i32::MAX; let _ = add(x, std::hint::black_box(1)); }
    #[test]
    #[ignore]
    fn slow() { assert_eq!(add(1, 1), 2); }
    // (never ends: `zurgo test`'s timeout stops it)
    #[test]
    #[ignore]
    fn hangs() {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    // (leaves a process that holds the test's output open: the runner stops it and goes on)
    #[test]
    #[ignore]
    fn leaves_a_child() {
        std::process::Command::new("sh").args(["-c", "sleep 30 &"]).status().unwrap();
    }
    #[test]
    fn reads_file() { assert!(std::fs::read_to_string("Cargo.toml").unwrap().contains("name = \"tp\"")); }
}
