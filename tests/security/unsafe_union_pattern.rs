//! Neighbor of unsafe_union_field: reading a union field by a pattern is a read too (E0133).
//@ pattern: E0133 context
//@ expect: reject
union U {
    i: usize,
    r: &'static u32,
}
fn main() {
    let u = U { i: 8 };
    let U { r } = u; //~ ERROR E0133 "union field"
    println!("{}", r);
}
