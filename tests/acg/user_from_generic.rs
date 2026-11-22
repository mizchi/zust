// `U::from(x)` with a program's `impl From<Code> for u32`: that impl's body, not a cast
enum Code {
    A = 7,
}
impl From<Code> for u32 {
    fn from(_: Code) -> u32 {
        99
    }
}
fn convert<U: From<Code>>(x: Code) -> U {
    U::from(x)
}
fn main() {
    let v: u32 = convert(Code::A);
    println!("{}", v);
}
