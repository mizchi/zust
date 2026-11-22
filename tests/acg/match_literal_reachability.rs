// literals told apart in match usefulness: distinct literals reach, `0x2a` is `42`
#![deny(unreachable_patterns)]
fn f(s: &str, n: u8) -> u8 {
    let a = match s {
        "hello" => 1,
        "world" => 2,
        _ => 3,
    };
    let b = match n {
        0x2a => 1,
        7 => 2,
        _ => 3,
    };
    let c = match (n, s) {
        (1, "a") => 1,
        (1, "b") => 2,
        (_, "a") => 3,
        _ => 4,
    };
    a + b + c
}
fn main() {
    println!("{} {}", f("world", 42), f("zz", 1));
}
