// `&x` passed or returned as `*const T` gives x the pointee's type; `for v in r` over a Result visits Ok's value
struct G<T, U>(*const T, *const U);

fn simple() -> *const u8 {
    static X: u8 = 7;
    &X
}

fn width(p: *const u16) -> usize {
    if p.is_null() { 0 } else { 2 }
}

fn main() {
    let y = 3;
    println!("{}", width(&y));
    let g = G(&(), &());
    println!("{}", g.0.is_null() || g.1.is_null());
    println!("{}", unsafe { *simple() });
    let z = 4;
    let q: *const u32 = &z;
    println!("{}", unsafe { *q } + 1);
    for v in Ok::<i32, ()>(5) {
        println!("ok {v}");
    }
    for v in Err::<i32, &str>("no") {
        println!("never {v}");
    }
    let r: Result<u8, ()> = Ok(9);
    println!("{}", r.iter().count());
}
