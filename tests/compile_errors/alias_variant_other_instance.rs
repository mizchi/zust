enum E<T> {
    V(T),
}
type A = E<u8>;
fn main() {
    let x: E<u16> = E::V(1);
    match x {
        A::V(_) => {}
    }
}
