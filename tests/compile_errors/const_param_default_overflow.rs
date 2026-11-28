// (a const parameter's default is evaluated where the type is declared, used or not)
struct Wide<const N: u8 = 2>;
struct Ooops<const N: u8 = { u8::MAX + 1 }>;
fn main() {
    let _ = Wide::<3>;
}
