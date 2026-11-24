// Coercions rustc makes without being asked: distinct closures with no captures (match arms, array and vec!
// elements) to their fn pointer, and a reference to an array to a raw slice pointer.
fn foo(x: usize) -> usize {
    x + 100
}

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
    // (a fn item, closures whose parameters are not written, closure locals, and an arm that diverges)
    let clo = |a| a * 2;
    let h = match pick {
        0 => foo,
        1 => |a| a + 1,
        2 => clo,
        _ => unimplemented!(),
    };
    let hs = [foo, |a| a * 3];
    println!("{} {} {}", h(4), hs[0](1), hs[1](1));
}
