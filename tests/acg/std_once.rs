// std::sync::Once (call_once, call_once_force, is_completed, ONCE_INIT) in a static, in a struct, and lazy_static's
// Lazy: `static L: Lazy<u64> = Lazy::INIT;` (an associated constant as a static's value) and Cell::as_ptr (#250)
use std::cell::Cell;
use std::mem::MaybeUninit;
use std::sync::{Once, ONCE_INIT};
static INIT: Once = Once::new();
static mut COUNT: u32 = 0;
pub struct Lazy<T: Sync>(Cell<MaybeUninit<T>>, Once);
unsafe impl<T: Sync> Sync for Lazy<T> {}
impl<T: Sync> Lazy<T> {
    pub const INIT: Self = Lazy(Cell::new(MaybeUninit::uninit()), ONCE_INIT);
    pub fn get<F: FnOnce() -> T>(&'static self, f: F) -> &T {
        self.1.call_once(|| {
            self.0.set(MaybeUninit::new(f()));
        });
        unsafe { &*(*self.0.as_ptr()).as_ptr() }
    }
}
static L: Lazy<u64> = Lazy::INIT;
struct Collector { once: Once, v: Cell<u32> }
impl Collector {
    fn new() -> Self { Collector { once: Once::new(), v: Cell::new(0) } }
    fn init(&self) { self.once.call_once(|| self.v.set(self.v.get() + 10)); }
}
fn bump() { unsafe { COUNT += 1 } }
fn main() {
    println!("{}", INIT.is_completed());
    INIT.call_once(bump);
    INIT.call_once(bump);
    INIT.call_once_force(|s| { let _ = s.is_poisoned(); bump() });
    println!("{} {}", INIT.is_completed(), unsafe { COUNT });
    println!("{} {}", L.get(|| 41 + 1), L.get(|| 0));
    let c = Collector::new();
    c.init();
    c.init();
    println!("{}", c.v.get());
}
