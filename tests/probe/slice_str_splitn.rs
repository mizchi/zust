// Slice and str methods the top crates call (#256): a slice's split_first / split_last / splitn / to_owned
// (form_urlencoded, percent-encoding, utf8_iter), str's splitn and is_ascii (iana-time-zone, version_check,
// httpdate), div_ceil (utf8_iter, utf16_iter), wrapping_add_signed (unicode-width) and a str trait's method of a
// String (`OsString::to_string_lossy`, version_check). And from #280's review: a slice's to_owned of a derive(Clone)
// type with a String in it, a str's `s[..n].to_owned()` (a String), and an empty separator, which matches at every
// char boundary
use std::ffi::OsString;

#[derive(Clone, Debug)]
struct Named {
    name: String,
    n: u8,
}

struct Bytes<'a> {
    bytes: &'a [u8],
}

impl<'a> Bytes<'a> {
    fn front(&mut self) -> Option<u8> {
        if let Some((&first, tail)) = self.bytes.split_first() {
            self.bytes = tail;
            Some(first)
        } else {
            None
        }
    }

    fn back(&mut self) -> Option<u8> {
        let (&last, before) = self.bytes.split_last()?;
        self.bytes = before;
        Some(last)
    }
}

fn pair(input: &[u8]) -> (&[u8], &[u8]) {
    let mut split2 = input.splitn(2, |&b| b == b'&');
    let seq = split2.next().unwrap();
    (seq, split2.next().unwrap_or(&[][..]))
}

fn quoted(s: &str) -> Option<(&str, &str)> {
    let mut iter = s[1..].splitn(2, '\'');
    match (iter.next(), iter.next()) {
        (Some(item), Some(tail)) => Some((item, tail)),
        _ => None,
    }
}

fn decode(input: &[u8]) -> Vec<u8> {
    match input.iter().position(|&b| b == b'+') {
        None => input.to_owned(),
        Some(i) => {
            let mut r = input.to_owned();
            r[i] = b' ';
            r
        }
    }
}

fn main() {
    let mut b = Bytes { bytes: b"abcd" };
    println!("{:?} {:?} {:?}", b.front(), b.back(), b.bytes);
    println!("{:?} {:?} {:?}", b.front(), b.front(), b.front());
    let e: &[u8] = &[];
    println!("{:?} {:?}", e.split_first(), e.split_last());
    let a = [1, 2, 3];
    println!("{:?} {:?}", a.split_first(), a.split_last());
    let v = vec![4, 5];
    println!("{:?}", v.split_first());

    println!("{:?} {:?}", pair(b"x=1&y=2&z"), pair(b"x=1"));
    let parts: Vec<&[u8]> = b"a,b,,c".splitn(3, |&c| c == b',').collect();
    println!("{:?}", parts);
    println!("{}", b"abc".splitn(0, |&c| c == b'b').count());
    println!("{:?} {:?}", decode(b"a+b"), decode(b"ab"));
    let s = &b"hello"[..];
    let mut owned = s[..3].to_owned();
    owned.push(b'!');
    println!("{:?}", owned);

    println!("{:?} {:?}", quoted("'ab'cd"), quoted("'ab"));
    let field = |s: &str| s.splitn(2, ":").nth(1).map(|s| s.trim().to_string());
    println!("{:?} {:?}", field("rustc: 1.97: x"), field("none"));
    let v: Vec<&str> = "a,b,c,d".splitn(3, ',').collect();
    println!("{:?}", v);
    let w: Vec<&str> = "abc".splitn(1, ',').collect();
    println!("{:?} {}", w, "abc".splitn(0, ',').count());
    println!("{} {} {}", "abc".is_ascii(), "h\u{e9}llo".is_ascii(), String::from("x y").is_ascii());

    let len = 7usize;
    println!("{} {} {} {}", len.div_ceil(2), 8u32.div_ceil(4), 9u64.div_ceil(4), 0u8.div_ceil(3));
    let d: i8 = -2;
    println!("{} {} {}", len.wrapping_add_signed(isize::from(d)), 1u8.wrapping_add_signed(-2), 250u8.wrapping_add_signed(10));

    let flags = OsString::from("-Zallow-features=a,b");
    println!("{}", flags.to_string_lossy());

    let named = vec![Named { name: "a".to_string(), n: 1 }, Named { name: "b".to_string(), n: 2 }];
    let all: &[Named] = &named;
    let copy = all.to_owned();
    let first = named[..1].to_owned();
    println!("{:?} {:?} {}", copy, first, copy[1].n + first[0].n);
    let text = "hello world";
    let k = 5;
    let head = text[..k].to_owned();
    let owned = String::from("xyz");
    println!("{} {} {}", head, text[k + 1..].to_owned(), owned[1..].to_owned());

    let e1: Vec<&str> = "ab".splitn(2, "").collect();
    let e2: Vec<&str> = "ab".splitn(5, "").collect();
    let e3: Vec<&str> = "h\u{e9}y".splitn(3, "").collect();
    let e4: Vec<&str> = "".splitn(3, "").collect();
    let e5: Vec<&str> = "ab".split("").collect();
    println!("{:?} {:?} {:?} {:?} {:?}", e1, e2, e3, e4, e5);
}
