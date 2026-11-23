enum E<T, const N: usize> {
    V(T),
}
fn main() {
    let e: E<u8, 3> = E::V(1);
    match e {
        E::<u16, 3>::V(..) => {}
    }
}
