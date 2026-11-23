//! zustscript_helper: the values a zustscript module passes between JS and wasm, in one stable byte format. The JS
//! side is the npm package `zustscript-helper` (helper/ts); zsc embeds this file into a module's Rust as
//! `mod zustscript_helper` (zustc compiles one file), and the glue calls the package.
//!
//! The format (version 1), little-endian, no padding, and no type tags: both sides know the type.
//!
//! | type | bytes |
//! |---|---|
//! | `i32` / `u32` / `f32` | 4 |
//! | `i64` / `f64` | 8 |
//! | `bool` | 1 (0 or 1) |
//! | `String` | its UTF-8 length as a `u32`, then the UTF-8 |
//! | `Vec<T>` | its length as a `u32`, then each element |
//! | a record | its fields in declaration order (zsc writes the `Wire` impl) |
//!
//! (Written with shifts and `to_bits`, not `to_le_bytes`: zustc's std has only what it needs.)

/// Reads values from bytes, front to back
pub struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, i: 0 }
    }
    pub fn take(&mut self, n: usize) -> &'a [u8] {
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        s
    }
}

/// A value in the format
pub trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(r: &mut Reader) -> Self;
}

fn put_le(out: &mut Vec<u8>, x: u64, n: usize) {
    let mut k = 0;
    while k < n {
        out.push((x >> (8 * k)) as u8);
        k += 1;
    }
}

fn get_le(r: &mut Reader, n: usize) -> u64 {
    let s = r.take(n);
    let mut x: u64 = 0;
    let mut k = 0;
    while k < n {
        x |= (s[k] as u64) << (8 * k);
        k += 1;
    }
    x
}

impl Wire for i32 {
    fn put(&self, out: &mut Vec<u8>) {
        put_le(out, *self as u32 as u64, 4);
    }
    fn get(r: &mut Reader) -> i32 {
        get_le(r, 4) as u32 as i32
    }
}

impl Wire for u32 {
    fn put(&self, out: &mut Vec<u8>) {
        put_le(out, *self as u64, 4);
    }
    fn get(r: &mut Reader) -> u32 {
        get_le(r, 4) as u32
    }
}

impl Wire for i64 {
    fn put(&self, out: &mut Vec<u8>) {
        put_le(out, *self as u64, 8);
    }
    fn get(r: &mut Reader) -> i64 {
        get_le(r, 8) as i64
    }
}

impl Wire for f32 {
    fn put(&self, out: &mut Vec<u8>) {
        put_le(out, self.to_bits() as u64, 4);
    }
    fn get(r: &mut Reader) -> f32 {
        f32::from_bits(get_le(r, 4) as u32)
    }
}

impl Wire for f64 {
    fn put(&self, out: &mut Vec<u8>) {
        put_le(out, self.to_bits(), 8);
    }
    fn get(r: &mut Reader) -> f64 {
        f64::from_bits(get_le(r, 8))
    }
}

impl Wire for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(if *self { 1 } else { 0 });
    }
    fn get(r: &mut Reader) -> bool {
        r.take(1)[0] != 0
    }
}

impl Wire for String {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        out.extend_from_slice(self.as_bytes());
    }
    fn get(r: &mut Reader) -> String {
        let n = u32::get(r) as usize;
        String::from_utf8(r.take(n).to_vec()).unwrap()
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        (self.len() as u32).put(out);
        for x in self {
            x.put(out);
        }
    }
    fn get(r: &mut Reader) -> Vec<T> {
        let n = u32::get(r) as usize;
        let mut v = Vec::with_capacity(n);
        let mut k = 0;
        while k < n {
            v.push(T::get(r));
            k += 1;
        }
        v
    }
}

/// `v` in the format
pub fn to_bytes<T: Wire>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    v.put(&mut out);
    out
}

/// The value `b` holds
pub fn from_bytes<T: Wire>(b: &[u8]) -> T {
    T::get(&mut Reader::new(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let xs = vec![vec![1.5f64, -2.0], vec![]];
        assert_eq!(from_bytes::<Vec<Vec<f64>>>(&to_bytes(&xs)), xs);
        let s = String::from("héllo 😀");
        let b = to_bytes(&s);
        assert_eq!(b.len(), 4 + s.len());
        assert_eq!(from_bytes::<String>(&b), s);
        assert_eq!(from_bytes::<i32>(&to_bytes(&-5i32)), -5);
        assert_eq!(from_bytes::<i64>(&to_bytes(&i64::MIN)), i64::MIN);
        assert!(from_bytes::<bool>(&to_bytes(&true)));
        // the layout is fixed: little-endian, no padding
        assert_eq!(to_bytes(&258u32), vec![2, 1, 0, 0]);
        assert_eq!(to_bytes(&vec![true, false]), vec![2, 0, 0, 0, 1, 0]);
    }
}
