// `-> impl Tr`: the type the body gives must be Tr; a blanket impl's where clause decides it here
pub trait Super {
    type Assoc;
}
impl Super for () {
    type Assoc = u8;
}
pub trait Test {}
impl<T> Test for T where T: Super<Assoc = ()> {}
fn test() -> impl Test {
    ()
}
fn main() {
    let _a = test();
}
