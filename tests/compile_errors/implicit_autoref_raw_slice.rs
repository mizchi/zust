// dangerous_implicit_autorefs (deny by default): `(*p).len()` on a raw slice pointer makes an implicit `&*p`
fn main() {
    let a = [1u8, 2, 3];
    let p: *const [u8] = &a;
    let fine = unsafe { (&*p).len() };
    let n = unsafe { (*p).len() };
    println!("{fine} {n}");
}
