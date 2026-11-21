//! A private field read from outside its module (E0616). Privacy is a safety boundary: unsafe code
//! inside the module relies on safe code outside not being able to touch its invariants.
//@ rule: privacy (a module's invariants are its own to keep)
//@ expect: reject
mod buf {
    pub struct Buf {
        len: usize,
    }
    impl Buf {
        pub fn new() -> Buf {
            Buf { len: 0 }
        }
        pub fn len(&self) -> usize {
            self.len
        }
    }
}
fn main() {
    let b = buf::Buf::new();
    println!("{}", b.len); //~ ERROR E0616
}
