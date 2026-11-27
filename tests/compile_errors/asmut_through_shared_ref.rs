// std's `impl AsMut<U> for &mut T` has no `&T` counterpart: a `&[u8; 2]` is no `AsMut<[u8]>`
fn first<T: AsMut<[u8]>>(mut t: T) -> u8 {
    t.as_mut()[0]
}

fn main() {
    let a = [1u8, 2];
    println!("{}", first(&mut [3u8, 4]));
    println!("{}", first(&a));
}
