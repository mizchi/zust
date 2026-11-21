//! A trait with a generic method is not object safe (E0038): its vtable has no entry to call.
//@ rule: dyn compatibility (E0038)
//@ expect: reject
trait Visit {
    fn visit<T: std::fmt::Debug>(&self, t: T);
}
struct S;
impl Visit for S {
    fn visit<T: std::fmt::Debug>(&self, t: T) {
        println!("{:?}", t);
    }
}
fn main() {
    let v: &dyn Visit = &S; //~ ERROR E0038
    let _ = v;
}
