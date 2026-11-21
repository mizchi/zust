//! Iterator invalidation (E0502): pushing to a Vec while iterating over it reallocates the buffer
//! under the iterator, the classic use-after-free of C++.
//@ expect: reject
fn main() {
    let mut v = vec![1, 2, 3];
    for x in &v {
        v.push(*x); //~ ERROR E0502
    }
    println!("{:?}", v);
}
