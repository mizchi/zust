// Traits of one name in two modules: the trait in scope picks b's `unsafe fn m`, and the call outside `unsafe` is E0133
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
        unsafe fn m(&self) -> u64;
    }
    impl M for u32 {
        unsafe fn m(&self) -> u64 {
            2
        }
    }
    pub fn run() -> u64 {
        5u32.m()
    }
}
fn main() {
    println!("{}", b::run());
}
