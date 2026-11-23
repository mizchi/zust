struct Error;
#[derive(Hash)]
struct Wrapper<T> {
    t: T,
    x: Error,
}
fn main() {}
