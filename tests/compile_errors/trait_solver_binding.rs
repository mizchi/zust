// A bound's binding against what the impl defines: `<W<i32> as Tr>::Assoc` is u32, not () (E0271)
trait Tr {
    type Assoc;
}
struct W<T>(T);
impl Tr for W<i32> {
    type Assoc = u32;
}
impl Tr for W<u32> {
    type Assoc = i32;
}
fn needs_unit<T: Tr<Assoc = ()>>() {}
fn main() {
    needs_unit::<W<u32>>();
    needs_unit::<W<i32>>();
}
