struct S;
impl S {
    const C: u8 = 1;
}
impl S {
    const C: u8 = 2;
}
fn main() {
    println!("{}", S::C);
}
