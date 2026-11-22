#![deny(unreachable_patterns)]
fn main() {
    match "x" {
        "hello" => {}
        "hello" => {}
        _ => {}
    }
}
