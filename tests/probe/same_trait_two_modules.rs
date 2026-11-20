// Two modules define traits of one name and implement them for the same types. A method call names no trait: it is
// the impl of the trait in scope at the call (defined in the module, or brought in by `use`, `use .. as`, a glob).
// zustc flattens modules and renames the traits (`a__M`, `b__M`); the call used to take the first impl. A closure in
// two impls' `m` (`Vec::m::{closure#0}`) also had one name for the two.
mod a {
    pub trait M {
        fn m(&self) -> u64;
    }
    impl M for u32 {
        fn m(&self) -> u64 {
            *self as u64
        }
    }
    impl<T: M> M for Vec<T> {
        fn m(&self) -> u64 {
            self.iter().map(|t| t.m()).sum()
        }
    }
    pub fn run() -> u64 {
        vec![1u32, 2].m() + 5u32.m()
    }
}

mod b {
    pub trait M {
        fn m(&self) -> u64;
    }
    impl M for u32 {
        fn m(&self) -> u64 {
            *self as u64 * 2
        }
    }
    impl<T: M> M for Vec<T> {
        fn m(&self) -> u64 {
            self.iter().map(|t| t.m()).sum()
        }
    }
    pub fn run() -> u64 {
        vec![1u32, 2].m() + 5u32.m()
    }
    pub mod inner {
        use super::M;
        pub fn run() -> u64 {
            5u32.m()
        }
    }
}

mod c {
    use crate::b::M as BM;
    pub fn run() -> u64 {
        7u32.m()
    }
}

mod d {
    use crate::a::*;
    pub fn run() -> u64 {
        9u32.m()
    }
}

use b::M;

fn main() {
    println!("{} {} {}", a::run(), b::run(), b::inner::run());
    println!("{} {} {}", c::run(), d::run(), 1u32.m());
}
