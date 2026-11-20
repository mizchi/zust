// Traits of one name in two modules, more ways of having one in scope: a re-export (`pub use` in a facade, then a
// use or a glob of the facade), a declaration with a newline between `trait` and its name, and impl methods that
// return closures (`-> impl Fn`, a Box<dyn Fn>) on a type both modules share.
pub struct S(pub u64);

mod a {
    pub trait
        M {
        fn m(&self) -> u64;
        fn make(&self) -> impl Fn(u64) -> u64;
    }
    impl M for u32 {
        fn m(&self) -> u64 {
            *self as u64
        }
        fn make(&self) -> impl Fn(u64) -> u64 {
            let k: u64 = *self as u64;
            move |x| x + k
        }
    }
    impl M for crate::S {
        fn m(&self) -> u64 {
            self.0
        }
        fn make(&self) -> impl Fn(u64) -> u64 {
            let k: u64 = self.0;
            move |x| x + k
        }
    }
    pub fn run() -> u64 {
        5u32.m() + 5u32.make()(1) + crate::S(3).make()(1)
    }
}

mod b {
    pub trait M {
        fn m(&self) -> u64;
        fn make(&self) -> impl Fn(u64) -> u64;
    }
    impl M for u32 {
        fn m(&self) -> u64 {
            *self as u64 * 2
        }
        fn make(&self) -> impl Fn(u64) -> u64 {
            let k: u64 = *self as u64;
            move |x| x * k
        }
    }
    impl M for crate::S {
        fn m(&self) -> u64 {
            self.0 * 2
        }
        fn make(&self) -> impl Fn(u64) -> u64 {
            let k: u64 = self.0;
            move |x| x * k
        }
    }
    pub fn run() -> u64 {
        5u32.m() + 5u32.make()(2) + crate::S(3).make()(2)
    }
}

mod facade {
    pub use crate::b::M;
}

mod user {
    use crate::facade::M;
    pub fn run() -> u64 {
        5u32.m() + crate::S(4).m()
    }
}

mod user2 {
    use crate::facade::*;
    pub fn run() -> u64 {
        6u32.m()
    }
}

fn main() {
    println!("{} {}", a::run(), b::run());
    println!("{} {}", user::run(), user2::run());
}
