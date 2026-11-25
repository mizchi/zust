// std::io::Seek / SeekFrom on a Cursor (Start, End, Current, an offset before the start is an error, rewind,
// stream_position), and Read through a Cursor<&[u8]> beside the one for Cursor<Vec<u8>>: as either's seek test does
use std::io::{self, Cursor, Read, Seek, SeekFrom};

fn main() {
    let data: Vec<u8> = (0..32).collect();
    let mut c = Cursor::new(&data[..]);
    let mut buf = [0u8; 4];
    c.read(&mut buf).unwrap();
    println!("{:?} {}", buf, c.position());
    println!("{}", c.seek(SeekFrom::Start(10)).unwrap());
    c.read(&mut buf).unwrap();
    println!("{:?}", buf);
    println!("{}", c.seek(SeekFrom::Current(-2)).unwrap());
    println!("{}", c.seek(SeekFrom::End(-4)).unwrap());
    c.read(&mut buf).unwrap();
    println!("{:?}", buf);
    println!("{}", c.seek(SeekFrom::Current(-100)).is_err());
    c.rewind().unwrap();
    println!("{}", c.stream_position().unwrap());
    let mut v = Cursor::new(vec![1u8, 2, 3]);
    println!("{}", v.seek(io::SeekFrom::End(0)).unwrap());
    let mut b2 = [0u8; 2];
    v.set_position(1);
    v.read(&mut b2).unwrap();
    println!("{:?}", b2);
    let arr = [7u8, 8, 9];
    let mut a = Cursor::new(&arr[..]);
    a.read(&mut b2).unwrap();
    println!("{:?}", b2);
}
