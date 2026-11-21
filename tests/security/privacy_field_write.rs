//! Writing a private length field from outside (E0616): with it, safe code could make an
//! unsafe `get_unchecked`-style accessor read out of bounds.
//@ expect: reject
mod buf {
    pub struct Buf {
        data: [u8; 4],
        len: usize,
    }
    impl Buf {
        pub fn new() -> Buf {
            Buf { data: [1, 2, 3, 4], len: 4 }
        }
        pub fn last(&self) -> u8 {
            unsafe { *self.data.as_ptr().add(self.len - 1) }
        }
    }
}
fn main() {
    let mut b = buf::Buf::new();
    b.len = 1000; //~ ERROR E0616
    println!("{}", b.last());
}
