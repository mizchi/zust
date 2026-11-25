// `dyn X<Y = str>` is no X: `type Y: Clone` and str is not Clone
trait X {
    type Y: Clone;
}
fn f<T: X + ?Sized>() {}
fn main() {
    f::<dyn X<Y = str>>();
}
