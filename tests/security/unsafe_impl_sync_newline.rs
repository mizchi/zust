//! Neighbor of unsafe_impl_send: `impl Sync` with `for` on the next line is the same impl (E0200).
//@ pattern: E0200 (a check that reads the impl's header)
//@ expect: reject
use std::cell::Cell;
struct B(Cell<u8>);
impl Sync //~ ERROR E0200
for B {}
fn main() {
    let b = B(Cell::new(1));
    println!("{}", b.0.get());
}
