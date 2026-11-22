struct S<T>(T);
fn main() {
    let _ = S::<Missing>(1u8);
}
