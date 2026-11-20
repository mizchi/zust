// Traits of one name in two modules, and what comes before a trait in scope: a name imported by `use` shadows the same
// name from a glob (`use a::*; use b::M;` is b's M), and an inherent method of the type comes before any trait's.
mod a {
    pub trait M {
        fn m(&self) -> u64;
    }
    impl M for u32 {
        fn m(&self) -> u64 {
            1
        }
    }
}

mod b {
    pub trait M {
        fn m(&self) -> u64;
    }
    impl M for u32 {
        fn m(&self) -> u64 {
            2
        }
    }
}

mod c {
    use crate::a::*;
    use crate::b::M;
    pub fn run() -> u64 {
        1u32.m()
    }
}

pub struct S;

impl S {
    fn m(&self) -> u64 {
        10
    }
}

impl a::M for S {
    fn m(&self) -> u64 {
        20
    }
}

use a::M;

fn main() {
    println!("{} {} {}", c::run(), S.m(), 3u32.m());
}
