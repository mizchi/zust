//! CVE-2021-28875 (std, fixed in 1.50.0): `read_to_end` trusted the count a `Read` impl returned and grew the Vec by
//! it, past what was written (a buffer overflow). A reader that claims more bytes than it was given room for must
//! not make the Vec longer than what was really read: std panics.
//! Not applicable yet: zustc cannot use `read_to_end` with a `Read` impl of the program's, so the build is rejected; when it builds, this case fails
//! and must become a test of the behavior above.
//@ cve: CVE-2021-28875
//@ expect: unsupported
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
