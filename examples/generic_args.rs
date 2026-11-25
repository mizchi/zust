// A callee's type parameters decided by more than the first argument: a turbofish with holes the argument fills,
// and a fn pointer parameter that gives an earlier literal its type.
fn pass<T>(t: T) -> T {
    t
}

fn spawn<T: Send>(val: T, f: fn(T) -> T) -> T {
    f(val)
}

fn double(i: isize) -> isize {
    i * 2
}

fn main() {
    let b = pass::<Box<_>>(Box::new(100));
    let v = pass::<Vec<_>>(vec![1u8, 2]);
    let r = spawn(21, double);
    let big = spawn(1 << 40, double);
    println!("{} {:?} {} {}", b, v, r, big);
}
