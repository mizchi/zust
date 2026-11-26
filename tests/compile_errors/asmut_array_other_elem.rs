// `[T; N]: AsMut<[T]>` is the array's own element type only: `[u8; 2]` is no `AsMut<[i32]>`
fn zero<T: AsMut<[i32]>>(mut t: T) -> T {
    for x in t.as_mut() {
        *x = 0;
    }
    t
}

fn main() {
    let a = zero([1u8, 2]);
    println!("{:?}", a);
}
