// Generic associated types: `P::Pointer<String>` as a field, `Self::Pointer<T>` as a return type
use std::rc::Rc;
trait PointerFamily {
    type Pointer<T>;
    fn new<T>(value: T) -> Self::Pointer<T>;
}
struct BoxFamily;
impl PointerFamily for BoxFamily {
    type Pointer<T> = Box<T>;
    fn new<T>(value: T) -> Self::Pointer<T> {
        Box::new(value)
    }
}
struct RcFamily;
impl PointerFamily for RcFamily {
    type Pointer<T> = Rc<T>;
    fn new<T>(value: T) -> Self::Pointer<T> {
        Rc::new(value)
    }
}
struct Named<P: PointerFamily> {
    name: P::Pointer<String>,
    n: P::Pointer<u32>,
}
fn main() {
    let b: Named<BoxFamily> = Named { name: BoxFamily::new(String::from("box")), n: BoxFamily::new(3u32) };
    let r: Named<RcFamily> = Named { name: RcFamily::new(String::from("rc")), n: RcFamily::new(4u32) };
    println!("{} {} {} {}", b.name, *b.n + 1, r.name, *r.n * 2);
}
