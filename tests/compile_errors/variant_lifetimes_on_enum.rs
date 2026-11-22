enum E<'a, 'b, T> {
    V(&'a T, &'b T),
}
fn main() {
    let _ = E::<'static>::V::<u8>(&1u8, &2u8);
}
