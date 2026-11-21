//! Building a struct with private fields from outside its module (E0451).
//@ rule: privacy (a module's invariants are its own to keep)
//@ expect: reject
mod m {
    pub struct NonZero {
        pub(crate) _p: (),
        v: u32,
    }
    impl NonZero {
        pub fn get(&self) -> u32 {
            self.v
        }
    }
}
fn main() {
    let z = m::NonZero { _p: (), v: 0 }; //~ ERROR E0451
    println!("{}", z.get());
}
