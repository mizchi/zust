// `x.borrow()` (#261): std's Borrow impls by the expected type or the bound of the parameter the receiver is declared
// with, and the program's own `borrow` where it is the one rustc's lookup finds (std's Borrow is not in the prelude)
mod own {
    pub trait MyBorrow {
        fn borrow(&self) -> &u32;
    }

    pub static SEVEN: u32 = 7;

    impl MyBorrow for u32 {
        fn borrow(&self) -> &u32 {
            println!("mine");
            &SEVEN
        }
    }
}

fn mine() -> u32 {
    use own::MyBorrow;
    let n = 5u32;
    let r: &u32 = n.borrow();
    *r
}

mod with_std {
    use std::borrow::Borrow;

    // (K's bound is not s's: s is a `&String`, its borrow the annotated `&String`)
    pub fn other_param<K: Borrow<str>>(k: &K, s: &String) -> usize {
        let whole: &String = s.borrow();
        let part: &str = k.borrow();
        whole.len() * 10 + part.len()
    }

    // (a `&mut` receiver: borrowed shared, and used after; u64, which no `borrow` of the program's is for)
    pub fn through_mut(x: &mut u64) -> u64 {
        let r: &u64 = x.borrow();
        let a = *r;
        *x += 1;
        let s: &u64 = x.borrow();
        a + *s + *x
    }
}

fn main() {
    println!("{}", mine());
    println!("{}", with_std::other_param(&String::from("ab"), &String::from("hello")));
    let mut n = 20u64;
    println!("{}", with_std::through_mut(&mut n));
    println!("{}", n);
}
