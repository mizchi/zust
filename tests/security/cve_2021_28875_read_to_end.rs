//! CVE-2021-28875 (std, fixed in 1.50.0): `read_to_end` trusted the count a `Read` impl returned and grew the Vec by
//! it, past what was written (a buffer overflow). A reader that claims more bytes than it was given room for must
//! not make the Vec longer than what was really read: std panics (at its slice of the buffer it read into), and so
//! does the prelude's `Read::read_to_end`, before the Vec grows (exit 101; valgrind: no write past the Vec).
//@ cve: CVE-2021-28875
//@ expect: panic
use std::io::Read;
struct Liar(u32);
impl Read for Liar {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0 == 0 {
            return Ok(0);
        }
        self.0 -= 1;
        buf[0] = b'x';
        Ok(buf.len() + 4096)
    }
}
fn main() {
    let mut v = Vec::new();
    let r = Liar(3).read_to_end(&mut v);
    println!("{:?} {}", r.is_ok(), v.len());
}
