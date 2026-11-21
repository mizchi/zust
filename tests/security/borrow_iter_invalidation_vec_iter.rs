//! Neighbor of borrow_iter_invalidation: an iterator kept in a local while the Vec grows (E0502).
//! The iterator points into the old buffer, which the push frees.
//@ pattern: iterator invalidation
//@ expect: reject
fn main() {
    let mut v = vec![1, 2, 3];
    let mut it = v.iter();
    v.push(4); //~ ERROR E0502
    println!("{:?}", it.next());
}
