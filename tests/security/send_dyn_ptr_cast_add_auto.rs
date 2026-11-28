//! A pointer cast may drop a trait object's auto traits, never add them (E0804): `*const dyn Job` as `*const (dyn Job
//! + Send)` would make the object behind it Send without its type being checked (an `Rc` inside).
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
trait Job {
    fn get(&self) -> u32;
}
impl Job for Rc<u32> {
    fn get(&self) -> u32 {
        **self
    }
}
fn main() {
    let rc = Rc::new(5u32);
    let p: *const dyn Job = &rc;
    let q = p as *const (dyn Job + Send); //~ ERROR E0804
    println!("{}", unsafe { (*q).get() });
}
