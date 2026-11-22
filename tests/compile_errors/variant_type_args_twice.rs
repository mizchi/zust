enum E<T> {
    V(T),
}
fn main() {
    let _ = E::V::<u8>(1);
    let _ = E::<u8>::V(2);
    let _ = E::<u8>::V::<u8>(3);
}
