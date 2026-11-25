// `<Self as Get>::Value` in a trait's method needs `Self: Get`: not a supertrait, no where clause
trait Get {
    type Value;
}
trait Fine: Get {
    fn ok(&self, v: <Self as Get>::Value);
}
trait Other {
    fn uhoh(&self, bar: <Self as Get>::Value);
}
fn main() {}
