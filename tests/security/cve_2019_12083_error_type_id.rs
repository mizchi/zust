//! CVE-2019-12083 (std, fixed in 1.34.2): `Error::type_id` could be overridden; returning another type's TypeId made
//! `downcast` cast the error to the wrong type. The method is no longer one an impl can define.
//@ cve: CVE-2019-12083
//@ expect: reject
use std::any::TypeId;
use std::fmt;
#[derive(Debug)]
struct E;
impl fmt::Display for E {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "E")
    }
}
impl std::error::Error for E {
    fn type_id(&self) -> TypeId { //~ ERROR
        TypeId::of::<u64>()
    }
}
fn main() {
    let e: Box<dyn std::error::Error> = Box::new(E);
    println!("{}", e);
}
