//! Neighbor of unsafe_union_field: reading a union field by a pattern is a read too (E0133).
//@ pattern: E0133 context
//@ todo: rejected, but as an unsupported pattern (a union pattern of one field), not as E0133
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
