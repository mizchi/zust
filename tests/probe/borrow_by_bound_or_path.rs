// `x.borrow()` by std's Borrow where no `use` brings it into scope (#261): a bound of the receiver's type parameter
// that names it by its path (inline or in a where clause), a call that names it by its path, a trait of the program's
// named Borrow that a `use` brings, and RefCell's own `borrow`
use std::cell::RefCell;

mod own {
    pub trait Borrow {
        fn borrow(&self) -> u8;
    }

    impl Borrow for u16 {
        fn borrow(&self) -> u8 {
            *self as u8 + 1
        }
    }
}

fn inline_bound<K: std::borrow::Borrow<str>>(k: &K) -> usize {
    let r = k.borrow();
    r.len()
}

fn where_bound<K>(k: &K) -> usize
where
    K: std::borrow::Borrow<[u8]>,
{
    let r: &[u8] = k.borrow();
    r.len()
}

fn by_path(s: &String) -> &str {
    std::borrow::Borrow::borrow(s)
}

fn own_trait() -> u8 {
    use own::Borrow;
    7u16.borrow()
}

fn main() {
    println!("{}", inline_bound(&String::from("abc")));
    println!("{}", where_bound(&vec![1u8, 2]));
    println!("{}", by_path(&String::from("xy")));
    println!("{}", own_trait());
    let c = RefCell::new(5);
    println!("{}", *c.borrow());
}
