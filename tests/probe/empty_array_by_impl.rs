// either's seek test: `Left(io::Cursor::new([]))` beside `Right(io::Cursor::new(&mockdata[..]))`, read as a Read: the
// empty array is a `[u8; 0]` by the one Read impl of Cursor that fits (`Cursor<[u8; N]>`); `assert_eq!(buf, mockdata[..n])`
use std::io::{self, Read, Seek, SeekFrom};
enum Either<L, R> { Left(L), Right(R) }
use Either::*;
impl<L: Read, R: Read> Read for Either<L, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self { Left(x) => x.read(buf), Right(x) => x.read(buf) }
    }
}
impl<L: Seek, R: Seek> Seek for Either<L, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self { Left(x) => x.seek(pos), Right(x) => x.seek(pos) }
    }
}
fn main() {
    let use_empty = false;
    let mut mockdata = [0x00; 256];
    for (i, data) in mockdata.iter_mut().enumerate() {
        *data = i as u8;
    }
    let mut reader = if use_empty {
        Left(io::Cursor::new([]))
    } else {
        Right(io::Cursor::new(&mockdata[..]))
    };
    let mut buf = [0u8; 16];
    assert_eq!(reader.read(&mut buf).unwrap(), buf.len());
    assert_eq!(buf, mockdata[..buf.len()]);
    assert_eq!(reader.read(&mut buf).unwrap(), buf.len());
    assert_ne!(buf, mockdata[..buf.len()]);
    reader.seek(io::SeekFrom::Start(0)).unwrap();
    assert_eq!(reader.read(&mut buf).unwrap(), buf.len());
    assert_eq!(buf, mockdata[..buf.len()]);
    println!("{:?}", &buf[..4]);
}
