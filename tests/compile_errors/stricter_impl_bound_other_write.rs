// Bounds are told by the trait their paths name (#264): `std::io::Write` is not the trait's `fmt::Write`, whose last
// segment it shares
use core::fmt;

trait WriteHex {
    fn write_hex<W: fmt::Write>(&self, writer: W) -> fmt::Result;
}

impl WriteHex for u8 {
    fn write_hex<W: std::io::Write>(&self, mut writer: W) -> fmt::Result {
        writer.write_all(b"ff").map_err(|_| fmt::Error)
    }
}

fn main() {}
