// Items of std under other names (`use std::cell::Cell as C;`), in a file without modules: as a type, a constructor
// path and a generic type with arguments
use std::cell::Cell as C;
use std::cell::LazyCell as Lazy;
use std::collections::HashMap as Map;
struct H {
    m: Map<u8, u8>,
    c: C<u8>,
    l: Lazy<u32>,
}
fn main() {
    let mut h = H { m: Map::new(), c: C::new(3), l: Lazy::new(|| 40) };
    h.m.insert(1, 2);
    h.c.set(h.c.get() + 1);
    println!("{} {} {}", h.m.len(), h.c.get(), *h.l + 2);
}
