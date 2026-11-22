#![deny(unreachable_patterns)]
fn main() {
    match "x" {
        "hello" => {}
        "hello" => {}
        _ => {}
    }
    match 5u8 {
        42 => {}
        0x2a => {}
        _ => {}
    }
}
