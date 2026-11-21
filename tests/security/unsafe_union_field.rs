//! Reading a union field from safe code (E0133): it reinterprets bytes, so a `&T` could be forged.
//@ rule: the unsafe boundary (E0133 / E0200 / E0512)
//@ expect: reject
union U {
    i: usize,
    r: &'static u32,
}
fn main() {
    let u = U { i: 8 };
    let r = u.r; //~ ERROR E0133
    println!("{}", r);
}
