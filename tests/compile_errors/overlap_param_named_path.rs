trait Pick<T> {}
struct S;
impl<T> Pick<T> for S {}
impl<Path> Pick<Path> for S {}
fn main() {}
