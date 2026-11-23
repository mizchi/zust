// A turbofish with a const argument: its types are the fn's type parameters in order, the const argument its const
// parameter's (`take::<u64, 2>(1 << 40)` is a u64, not an i32)
fn take<T, const N: usize>(x: T) -> T {
    x
}

fn rep<T: Copy, const N: usize>(x: T) -> [T; N] {
    [x; N]
}

fn main() {
    let y = take::<u8, 1>(200);
    println!("{}", y.wrapping_add(100));
    let z = take::<u64, 2>(1 << 40);
    println!("{}", z);
    let a = rep::<u16, 3>(7);
    println!("{:?} {}", a, std::mem::size_of_val(&a));
}
