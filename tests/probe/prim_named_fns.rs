// Functions named like primitive types, in two modules (so the flattening renames them, as it renames every item of a
// crate): getrandom's `pub fn u32() -> Result<u32, Error>`, fastrand's `u8` / `char`, chrono's `fn char`. The type
// beside such a fn, a number's suffix (`0u8`) and a path through the type (`u32::MAX`) stay the primitive; a call, a
// fn passed as an argument (`.map_or(x, char)`) and a fn as a value (`let f = u32;`) are the fn, and a block's
// `use .. as u8` shadows the primitive (#254)
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
    pub fn values() -> (u32, u8) {
        let f = u32;
        let g: fn(u8) -> u8 = u8;
        let it: Option<Box<dyn Iterator<Item = u8>>> = None;
        (f().unwrap_or(0) - u32::MAX + 1, g(it.map_or(3, |_| 0)))
    }
    #[derive(Debug)]
    #[allow(dead_code)]
    pub struct Byte(pub u8);
    pub fn shadowed() -> String {
        use crate::rng::Byte as u8;
        let b: u8 = u8(1);
        format!("{:?}", b)
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
    println!("{:?} {}", rng::values(), rng::shadowed());
}
