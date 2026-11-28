// `slice::from_ref(r)` / `slice::from_mut(r)` are one-value slices, not ptr::from_ref's raw pointer: by path
// (utf_types' `core::slice::from_ref`), and by a `use core::slice::from_ref;` in a fn body (regex-syntax's)
use std::slice;

enum Seq {
    One(u16),
    Two([u16; 2]),
}

impl Seq {
    fn as_slice(&self) -> &[u16] {
        match *self {
            Seq::One(ref r) => slice::from_ref(r),
            Seq::Two(ref r) => &r[..],
        }
    }
}

mod m {
    pub fn subs(x: &u32) -> &[u32] {
        use core::slice::from_ref;
        from_ref(x)
    }
}

fn main() {
    let x = 7u16;
    let s: &[u16] = core::slice::from_ref(&x);
    let mut y = 3u32;
    let m = slice::from_mut(&mut y);
    m[0] += 1;
    let p: *const u16 = std::ptr::from_ref(&x);
    println!("{:?} {:?} {:?} {}", s, Seq::One(1).as_slice(), Seq::Two([2, 3]).as_slice(), y);
    println!("{:?} {}", m::subs(&9), unsafe { *p });
}
