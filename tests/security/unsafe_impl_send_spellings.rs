//! Neighbor of unsafe_impl_send: an attribute whose text holds a `]`, and an impl header split over lines, are the
//! same impl. Neither may hide a safe `impl Send` from E0200.
//@ pattern: E0200 (a check that reads the impl's header)
//@ expect: reject
use std::rc::Rc;
struct A(Rc<u8>);
#[doc = "a ] inside the attribute's string"]
impl Send for A {} //~ ERROR E0200
fn main() {
    let a = A(Rc::new(1));
    println!("{}", a.0);
}
