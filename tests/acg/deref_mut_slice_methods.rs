// A `&mut self` method of the slice a program's `DerefMut` reaches (`c.reverse()` with `C: DerefMut<Target = [i32]>`)
// runs `deref_mut`, and a `&self` one (`c.len()`) `deref`, as rustc's autoderef
use std::ops::{Deref, DerefMut};
struct C([i32; 3]);
impl Deref for C { type Target = [i32]; fn deref(&self) -> &[i32] { println!("d"); &self.0 } }
impl DerefMut for C { fn deref_mut(&mut self) -> &mut [i32] { println!("dm"); &mut self.0 } }
fn main() {
    let mut c = C([1, 2, 3]);
    c.reverse();
    println!("{} {:?}", c.len(), c.0);
    c.sort();
    println!("{:?} {:?}", c.first(), c.0);
}
