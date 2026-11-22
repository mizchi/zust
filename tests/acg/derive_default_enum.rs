// `#[derive(Default)]` on an enum with a `#[default]` unit variant: no bound on its arguments
#[derive(Default)]
enum E<T> {
    #[default]
    Empty,
    Full(T),
}
struct X;
fn d<T: Default>() -> T { T::default() }
fn main() {
    let e: E<X> = d();
    let _ = E::Full(X);
    println!("{}", matches!(e, E::Empty));
}
