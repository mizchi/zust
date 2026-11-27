//! `&(dyn Tr + Send)` → `*const dyn Tr` drops an auto trait, as a coercion may (rustc accepts it); adding one is still
//! E0308 (`&dyn Tr` → `*const (dyn Tr + Send)`).
//@ rule: Send / Sync
//@ expect: run
trait Tr {
    fn get(&self) -> u32;
}
impl Tr for u32 {
    fn get(&self) -> u32 {
        *self
    }
}
fn erase(x: &(dyn Tr + Send)) -> *const (dyn Tr + '_) {
    return x;
}
fn erase2(x: &(dyn Tr + Send + Sync)) -> *const (dyn Tr + Sync + '_) {
    x
}
fn main() {
    let v = 7u32;
    let p = erase(&v);
    let q = erase2(&v);
    println!("{} {}", unsafe { (*p).get() }, unsafe { (*q).get() });
}
