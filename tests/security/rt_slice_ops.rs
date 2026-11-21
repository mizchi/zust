//! Neighbors of rt_bounds: the slice operations whose lengths or positions come from the caller.
//@ pattern: bounds
//@ expect: panic
//@ args: 0 1 2 3 4 5 6 7
fn main() {
    let k: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let n = 3 + (k & 0);
    let mut a = vec![1u32, 2, 3];
    let b = vec![9u32; n + 1];
    match k {
        0 => a.copy_from_slice(&b),
        1 => a.clone_from_slice(&b[..n - 1]),
        2 => a.rotate_left(n + 1),
        3 => a.rotate_right(n + 1),
        4 => {
            let (x, y) = a.split_at_mut(n + 1);
            println!("{:?} {:?}", x, y);
        }
        5 => println!("{}", a.chunks_exact(n - 3).count()),
        6 => a.swap(0, n),
        _ => println!("{:?}", &a[n + 1..]),
    }
    println!("not reached {:?}", a);
}
