// An impl's where clause on a concrete type with a binding: `(): Array<Element = &()>` does not hold, so `(): Visit`
// does not
pub trait Array {
    type Element;
}
pub trait Visit {
    fn visit() {}
}
impl Array for () {
    type Element = ();
}
impl<'a> Visit for () where (): Array<Element = &'a ()> {}
fn main() {
    <() as Visit>::visit();
}
