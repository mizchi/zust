// Functions named like primitive types, in two modules (so the flattening renames them, as it renames every item of a
// crate): getrandom's `pub fn u32() -> Result<u32, Error>`, fastrand's `u8` / `char`, chrono's `fn char`. The type
// beside such a fn, a number's suffix (`0u8`) and a path through the type (`u32::MAX`) stay the primitive; a call and
// a fn passed as an argument (`.map_or(x, char)`) are the fn (#254)
mod rng {
    pub fn u8(n: u8) -> u8 {
        n.wrapping_add(1)
    }
    pub fn char(c: char) -> char {
        c.to_ascii_uppercase()
    }
    pub fn fill(s: &mut [u8]) {
        for b in s.iter_mut() {
            *b = u8(*b);
        }
    }
    pub fn u32() -> Result<u32, ()> {
        Ok(u32::MAX - 0u32)
    }
    pub fn first() -> char {
        char::from_u32(0x61u32).map_or('z', char)
    }
    pub fn pair() -> (u8, char) {
        (u8(1), char('p'))
    }
}

mod other {
    pub fn u8() -> u8 {
        255u8
    }
    pub fn u32(x: u32) -> u32 {
        x + 1_000u32
    }
    pub fn char() -> char {
        'b'
    }
}

use rng::*;

struct Wrap(u8, char);

#[allow(non_camel_case_types)]
struct u8x2(u8, u8);

fn main() {
    let x = 0u8 as char;
    let mut buf = [0u8, 7, 254];
    fill(&mut buf);
    let v: Vec<u8> = vec![u8(1), other::u8()];
    println!("{} {:?} {:?} {:?}", x as u32, buf, u32(), v);
    println!("{} {} {} {} {:?}", char('q'), other::char(), first(), other::u32(2), pair());
    let w = Wrap(u8(9), char('w'));
    let p = u8x2(u8(3), 4);
    println!("{} {} {} {} {} {}", w.0, w.1, u8::MAX, <u32>::BITS, 1.5f32 * 2.0, p.0 + p.1);
}
