#![deny(unreachable_patterns)]
fn main() {
    // (`0x2a` is 42 again)
    match 5u8 {
        42 => {}
        0x2a => {}
        _ => {}
    }
}
