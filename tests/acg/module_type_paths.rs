// `M::S<3>` / `M::W<u8>`: a type through a module whose name starts with an uppercase letter, which reads like a
// generic associated type of a type `M`
#[allow(non_snake_case)]
mod M {
    pub struct S<const N: usize>;
    impl<const N: usize> S<N> {
        pub fn n(&self) -> usize {
            N
        }
    }
    pub struct W<T>(pub T);
}
use M::{S, W};

// (`B::Box<dyn Send>`: an auto trait alone in a Box, as for a Box without a module)
#[allow(non_snake_case)]
mod B {
    pub use std::boxed::Box;
}

fn h(b: B::Box<dyn Send>) -> u8 {
    drop(b);
    3
}

fn f(s: M::S<3>) -> usize {
    s.n()
}

fn g(w: M::W<u8>) -> u8 {
    w.0
}

fn main() {
    let s: S<3> = S;
    println!("{} {} {}", f(s), g(W(7)), h(Box::new(5u8)));
}
