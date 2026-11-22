const N: u8 = 1;
fn flag<const B: bool>() {}
fn main() {
    flag::<N>();
}
