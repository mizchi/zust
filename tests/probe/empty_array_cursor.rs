// `io::Cursor::new([])` read as a Read alone: the `Cursor<[u8; N]>` impl, not the first impl's `Cursor<Vec<u8>>`; a
// cursor over a filled array, and array / range-index comparisons in assert_eq! / assert_ne!
use std::io::{Read, Seek, SeekFrom};
fn main() {
    let mut c = std::io::Cursor::new([]);
    let mut buf = [0u8; 4];
    println!("{}", c.read(&mut buf).unwrap());
    let mut d = std::io::Cursor::new([1u8, 2, 3, 4, 5]);
    println!("{} {:?}", d.read(&mut buf).unwrap(), buf);
    println!("{} {}", d.seek(SeekFrom::End(-2)).unwrap(), d.read(&mut buf).unwrap());
    let a = [1u8; 2];
    let b = [1u8, 1, 3];
    assert_eq!(a, b[..2]);
    assert_ne!(a, b[1..]);
    assert_eq!(b[..2], a);
    println!("ok");
}
