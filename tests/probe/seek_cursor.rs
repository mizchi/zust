// std::io::Seek / SeekFrom on a Cursor (Start, End, Current, an offset before the start is an error, rewind,
// stream_position, positions above i64::MAX), and Read through a Cursor<&[u8]> beside the one for Cursor<Vec<u8>>: as either's seek test does
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
    // positions above i64::MAX: a relative seek applies the signed offset to the u64 position (#203)
    let d = [0u8; 4];
    let mut c = Cursor::new(&d[..]);
    c.seek(SeekFrom::Start(1 << 63)).unwrap();
    println!("{:?}", c.stream_position().is_ok());
    println!("{:?}", c.seek(SeekFrom::Current(5)).ok());
    println!("{:?}", c.seek(SeekFrom::Current(-6)).ok());
    c.seek(SeekFrom::Start(u64::MAX - 1)).unwrap();
    println!("{:?}", c.seek(SeekFrom::Current(1)).ok());
    // past u64::MAX, or below 0, is an error and leaves the position where it was
    println!("{} {}", c.seek(SeekFrom::Current(1)).is_err(), c.position());
    println!("{} {}", c.seek(SeekFrom::Current(i64::MIN)).ok().unwrap(), c.position());
    c.set_position(3);
    println!("{} {}", c.seek(SeekFrom::Current(i64::MIN)).is_err(), c.position());
    println!("{} {}", c.seek(SeekFrom::End(-5)).is_err(), c.position());
    println!("{:?}", c.seek(SeekFrom::End(i64::MAX)).ok());
    let mut v = Cursor::new(vec![0u8; 2]);
    v.set_position(u64::MAX);
    println!("{} {}", v.seek(SeekFrom::Current(1)).is_err(), v.position());
    println!("{:?}", v.seek(SeekFrom::Current(-1)).ok());
}
