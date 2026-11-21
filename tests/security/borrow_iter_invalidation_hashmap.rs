//! Iterator invalidation on a HashMap (E0502): an insert may rehash under a live iterator.
//@ pattern: iterator invalidation
//@ expect: reject
use std::collections::HashMap;
fn main() {
    let mut m: HashMap<u32, String> = HashMap::new();
    m.insert(1, "a".to_string());
    let mut it = m.iter();
    for i in 0..100 {
        m.insert(i, i.to_string()); //~ ERROR E0502
    }
    println!("{:?}", it.next());
}
