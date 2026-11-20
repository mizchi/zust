// Traits of one name in two modules, three more rules: a call that names its trait (`crate::a::M::m(&x)`) takes that
// trait's impl whatever is in scope; a named import shadows a glob's whatever their order (`use b::*; use b::M;
// use a::*;`); and a module that re-exports both (`pub use a::*; pub use b::M;`) passes on only the one that wins.
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
    pub fn run() -> u64 {
        crate::a::M::m(&5u32) * 10 + 5u32.m()
    }
}
mod c {
    pub use crate::a::*;
    pub use crate::b::M;
}
mod d {
    use crate::b::*;
    use crate::b::M;
    use crate::a::*;
    pub fn run() -> u64 {
        3u32.m()
    }
}
mod e {
    use crate::c::M;
    pub fn run() -> u64 {
        3u32.m()
    }
}
fn main() {
    println!("{} {} {}", b::run(), d::run(), e::run());
}
