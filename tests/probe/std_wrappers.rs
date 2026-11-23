// std types that wasm-bindgen names: AssertUnwindSafe (an impl of the program's trait for it, its deref, the
// UnwindSafe bound), LocalKey as a type with `with`, and LazyCell (made on the first deref, once)
use std::cell::LazyCell;
use std::panic::{AssertUnwindSafe, UnwindSafe};
use std::thread::LocalKey;

trait Describe {
    fn describe() -> u32;
}
impl Describe for u8 {
    fn describe() -> u32 {
        8
    }
}
impl<T: Describe> Describe for AssertUnwindSafe<T> {
    fn describe() -> u32 {
        T::describe() + 100
    }
}
fn need<T: UnwindSafe>(_: &T) -> u32 {
    1
}

struct Holder<T: 'static> {
    inner: &'static LocalKey<T>,
}
impl<T: Copy + 'static> Holder<T> {
    fn get(&self) -> T {
        self.inner.with(|v| *v)
    }
}
thread_local! {
    static N: u32 = 7;
}

fn main() {
    let mut x = 5;
    let mut w = AssertUnwindSafe(&mut x);
    **w += 1;
    let a = AssertUnwindSafe(7u8);
    println!("{} {} {} {}", x, *a, <AssertUnwindSafe<u8> as Describe>::describe(), need(&a));
    // (a lock is UnwindSafe whatever it holds)
    let mut y = 1;
    println!("{}", need(&std::sync::Mutex::new(&mut y)));
    N.with(|n| println!("{n}"));
    let _f: fn(&Holder<u32>) -> u32 = Holder::get;
    let calls = std::cell::Cell::new(0);
    let c: LazyCell<u32, _> = LazyCell::new(|| {
        calls.set(calls.get() + 1);
        40
    });
    println!("{} {} {}", *c + 1, *LazyCell::force(&c), calls.get());
}
