// Cases from the review of the W1 work: two functions' extern blocks with one name (each keeps its own symbol), a
// local that shadows a std item imported under another name (the call before it, and after the block of another
// such local, is still the std item), `::name::..`
// from an extern crate next to a one-name `::path`, a macro_rules! call with a path in statement position (its `let`
// is the block's), and explicit type arguments of a method of an impl for `&&T`
use std::mem::drop as release;

fn a(x: i32) -> i32 {
    unsafe extern "C" {
        fn abs(x: i32) -> i32;
    }
    unsafe { abs(x) }
}

fn b(x: i64) -> i64 {
    unsafe extern "C" {
        #[link_name = "labs"]
        fn abs(x: i64) -> i64;
    }
    unsafe { abs(x) }
}

macro_rules! bind {
    ($n:ident, $v:expr) => {
        let $n = $v;
    };
}
pub(crate) use bind;

trait Size {
    fn size<U>(&self) -> usize;
}
impl Size for &u8 {
    fn size<U>(&self) -> usize {
        std::mem::size_of::<U>()
    }
}
impl Size for &&u8 {
    fn size<U>(&self) -> usize {
        std::mem::size_of::<U>() + 100
    }
}

fn main() {
    println!("{} {}", a(-3), b(-5_000_000_000));
    let v = vec![1, 2];
    {
        let release = 5;
        println!("{}", release);
    }
    release(v);
    let release = 3;
    println!("{}", release + 1);
    println!("{}", ::std::cmp::max(1, 2));
    crate::bind!(value, 41);
    println!("{}", value + 1);
    let x = 7u8;
    let r = &x;
    let rr = &r;
    println!("{} {}", r.size::<u16>(), rr.size::<u32>());
}
