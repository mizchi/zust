// `std::ptr::from_ref` / `from_mut`: a reference as a raw pointer (also of an unsized type)
fn main() {
    let x = 5u8;
    let p: *const u8 = std::ptr::from_ref(&x);
    let mut y = [1u32, 2];
    let q = core::ptr::from_mut(&mut y);
    unsafe {
        (*q)[1] = 7;
        println!("{} {:?}", *p, *q);
    }
    let s = "hi";
    let r = std::ptr::from_ref(s);
    println!("{}", unsafe { &*r }.len());
}
