//! `&(dyn Tr + Send)` → `*const dyn Tr` drops an auto trait, as a coercion may (rustc accepts it), and so may the same
//! as an `as` cast; adding one is still refused (`&dyn Tr` → `*const (dyn Tr + Send)`: E0308, or E0606 as a cast).
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
    let x: &(dyn Tr + Send) = &v;
    let y: &(dyn Tr + Send + Sync) = &v;
    let (c, d) = (x as *const dyn Tr, y as *const (dyn Tr + Sync));
    println!("{} {} {}", unsafe { (*c).get() }, unsafe { (*d).get() }, (x as &dyn Tr).get());
}
