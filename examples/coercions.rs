// Coercions rustc makes without being asked: distinct closures with no captures (match arms, array and vec!
// elements) to their fn pointer, and a reference to an array to a raw slice pointer.
fn main() {
    let pick = 1;
    let f = match pick {
        0 => |x: i32| x + 1,
        _ => |x: i32| x * 10,
    };
    let fs = [|x: i32| x - 1, |x: i32| x * x];
    let gs = vec![|x: i32| -> i32 { x + 3 }, |x: i32| -> i32 { x * 3 }];
    let a = [1u8, 2, 3];
    let p: *const [u8] = &a;
    let n = unsafe { (&*p).len() };
    println!("{} {} {} {} {}", f(2), fs[0](5), fs[1](5), gs[1](4), n);
}
