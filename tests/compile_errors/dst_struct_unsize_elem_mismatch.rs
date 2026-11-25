struct Fat<T: ?Sized> { f1: isize, ptr: T }
fn main() {
    let f = Fat { f1: 1, ptr: [1i32, 2, 3] };
    let r: &Fat<[u8]> = &f;
    println!("{}", r.f1);
}
