struct S<T>(T);
fn main() {
    let _x: S<u16> = S::<u8>(1u16);
}
