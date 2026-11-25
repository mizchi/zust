// A trait's parameter's bound holds for an impl's trait argument: `isize: Clone2`
pub trait Clone2 {
    fn clone2(&self) -> Self;
}
trait Getter<T: Clone2> {
    fn get(&self) -> T;
}
impl Getter<isize> for isize {
    fn get(&self) -> isize {
        *self
    }
}
fn main() {}
