// A derive written as std's prelude writes it, `#[::core::prelude::v1::derive(::core::clone::Clone, ::core::marker::Copy)]`
// (libc 0.2's `s!`): the struct is Copy, so `self.v` is a copy, not a move out of a reference
#[::core::prelude::v1::derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct V { pub p: *mut u8 }
#[derive(Clone, Copy)]
pub struct W { pub p: *mut u8 }
struct H { v: V, w: W }
impl H {
    fn v(&self) -> V { self.v }
    fn w(&self) -> W { self.w }
}
fn main() {
    let h = H { v: V { p: core::ptr::null_mut() }, w: W { p: core::ptr::null_mut() } };
    println!("{} {}", h.v().p.is_null(), h.w().p.is_null());
}
