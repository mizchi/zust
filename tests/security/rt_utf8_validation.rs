//! `str::from_utf8` / `String::from_utf8` / `char::from_u32` are the gates from bytes and integers
//! to `str` and `char`: overlong encodings, surrogates, values past U+10FFFF and truncated sequences
//! must all be rejected, or later code that trusts the UTF-8 invariant reads out of bounds.
//@ expect: run
fn main() {
    let bad: [&[u8]; 9] = [
        b"\xC0\x80",
        b"\xE0\x80\x80",
        b"\xED\xA0\x80",
        b"\xF4\x90\x80\x80",
        b"\xF8\x88\x80\x80\x80",
        b"\xE2\x82",
        b"\x80",
        b"a\xFF",
        b"\xF0\x9F\x98",
    ];
    for b in bad {
        let e = std::str::from_utf8(b).is_err();
        let e2 = String::from_utf8(b.to_vec()).is_err();
        println!("{:x?} {} {}", b, e, e2);
    }
    let good: [&[u8]; 3] = [b"\xF4\x8F\xBF\xBF", b"\xED\x9F\xBF", b"\xE2\x82\xAC"];
    for b in good {
        println!("{:x?} {}", b, std::str::from_utf8(b).is_ok());
    }
    for x in [0xD800u32, 0xDFFF, 0x110000, u32::MAX, 0x10FFFF, 0x41] {
        println!("{:x} {:?}", x, char::from_u32(x).map(|c| c as u32));
    }
}
