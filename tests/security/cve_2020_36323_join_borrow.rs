//! CVE-2020-36323 (std, fixed in 1.52.0): `[S]::join` for `S: Borrow<str>` measured the parts, then borrowed them
//! again to copy; a Borrow impl that answered a longer string the second time made join expose uninitialized bytes.
//! Here each borrow answers a different string: join must give a valid String of what it copied (or panic).
//! Not applicable yet: zustc has no `join` over a type of the program's that is `Borrow<str>`, so the build is rejected; when it builds, this case fails
//! and must become a test of the behavior above.
//@ cve: CVE-2020-36323
//@ expect: unsupported
use std::borrow::Borrow;
use std::cell::Cell;
struct Flip(Cell<u32>);
impl Borrow<str> for Flip {
    fn borrow(&self) -> &str {
        self.0.set(self.0.get() + 1);
        if self.0.get() % 2 == 1 { "a" } else { "a much longer string than the first answer" }
    }
}
fn main() {
    let parts = [Flip(Cell::new(0)), Flip(Cell::new(0)), Flip(Cell::new(0))];
    let j: String = parts.join(",");
    println!("ok {}", std::str::from_utf8(j.as_bytes()).is_ok());
}
