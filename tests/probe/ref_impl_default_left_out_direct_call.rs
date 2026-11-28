// A direct call on `&&S` (`(&r).value()`) takes the impl for `&T` at rustc's first probe step, whose left-out default gives 1, not S's override (2) (#144)
trait Tr { fn value(&self) -> u32 { 1 } fn other(&self) -> u32; }
struct S;
impl Tr for S { fn value(&self) -> u32 { 2 } fn other(&self) -> u32 { 3 } }
impl<T: Tr> Tr for &T { fn other(&self) -> u32 { (**self).other() } }
fn main() { let s = S; let r = &s; println!("{}", (&r).value()); }
