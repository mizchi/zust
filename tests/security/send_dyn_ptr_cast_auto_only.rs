//! A pointer cast between objects of auto traits alone may add one as well as drop one, as rustc's: E0804 is for an
//! object with a principal trait (send_dyn_ptr_cast_add_auto.rs), and `dyn Send` or `dyn Sync` has no methods to call
//! through the pointer.
//@ rule: Send / Sync
//@ expect: run
fn main() {
    let x = 5u32;
    let p: *const dyn Send = &x;
    let q = p as *const (dyn Send + Sync);
    let s: *const dyn Sync = &x;
    let t = s as *const (dyn Send + Sync);
    let u = q as *const dyn Sync;
    let v = t as *const dyn Send;
    let _ = (u, v);
    println!("ok");
}
