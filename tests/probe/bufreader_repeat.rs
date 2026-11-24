// std::io::BufReader (fill_buf / consume / read, through `&mut B: BufRead`), an array repeat `&[0; N * M]` where a
// slice is expected (the element's type is the slice's), and `use` inside a fn body that a fn beside it sees
use std::io::{BufRead, BufReader, Read};

fn sum<R: BufRead>(mut r: R) -> (u64, usize) {
    let (mut s, mut fills) = (0u64, 0);
    loop {
        let n = {
            let b = r.fill_buf().unwrap();
            if b.is_empty() {
                break;
            }
            fills += 1;
            for &x in b {
                s += x as u64;
            }
            b.len()
        };
        r.consume(n);
    }
    (s, fills)
}

fn len_first(b: &[u8]) -> usize {
    b.len() + b[0] as usize
}

fn set(b: &mut [u16]) -> usize {
    b[1] = 7;
    b.len() + b[1] as usize
}

const N: usize = 3;

fn main() {
    let data = vec![3u8; 20000];
    println!("{:?}", sum(BufReader::new(&data[..])));
    println!("{:?}", sum(BufReader::with_capacity(7, &b"hello world"[..])));
    let mut b = BufReader::with_capacity(3, &[2u8; 10][..]);
    println!("{:?}", sum(&mut b));
    let mut b = BufReader::with_capacity(4, &b"abcdefghij"[..]);
    let mut out = [0u8; 3];
    let n = b.read(&mut out).unwrap();
    println!("{} {:?} {:?} {}", n, out, b.buffer(), b.capacity());
    let mut big = [0u8; 8];
    let m = b.read(&mut big).unwrap();
    println!("{} {:?}", m, &big[..m]);
    let mut b = BufReader::with_capacity(2, &b"xyz"[..]);
    let mut all = [0u8; 3];
    b.read_exact(&mut all).unwrap();
    println!("{:?} {}", all, b.into_inner().len());

    println!("{}", len_first(&[0; 100]));
    println!("{}", len_first(&[255; 1024 * 1024]));
    println!("{}", len_first(&[7; N]));
    println!("{}", set(&mut [0; N * 2]));
    let k = 9u8;
    println!("{}", len_first(&[k; 2 + 2]));

    {
        use std::io::BufReader as Br;
        fn test(data: &[u8], want: u64) {
            let buf = Br::new(data);
            assert_eq!(sum(buf).0, want);
        }
        test(&[], 0);
        test(&[1; 10], 10);
        test(&[0xA5; 64 * 1024], 0xA5 * 64 * 1024);
        println!("nested ok");
    }
}
