struct S<T>(T);
fn main() {
    let _ = S::<u8, u16>(1u8);
}
