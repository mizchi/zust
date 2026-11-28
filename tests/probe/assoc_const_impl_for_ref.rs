// der's `impl FixedTag for OctetStringRef` beside `impl FixedTag for &OctetStringRef`: two impls of one trait whose
// self types are a type and a reference to it, each with its `TAG`. The self type of the path chooses
trait FixedTag {
    const TAG: u8;
}
struct Oct;
impl FixedTag for Oct {
    const TAG: u8 = 4;
}
impl FixedTag for &Oct {
    const TAG: u8 = 5;
}
impl FixedTag for &mut Oct {
    const TAG: u8 = 6;
}
struct Only;
impl FixedTag for &Only {
    const TAG: u8 = 7;
}
fn tag<T: FixedTag>(_: T) -> u8 {
    T::TAG
}
fn tag_of<T: FixedTag>() -> u8 {
    <T as FixedTag>::TAG
}
trait Tagged {
    fn tag(&self) -> u8;
}
impl<T: FixedTag> Tagged for T {
    fn tag(&self) -> u8 {
        T::TAG
    }
}
impl Oct {
    fn own(&self) -> u8 {
        Self::TAG
    }
}
fn main() {
    println!("{} {} {}", Oct::TAG, <&Oct>::TAG, <&Oct as FixedTag>::TAG);
    println!("{} {} {}", <Oct as FixedTag>::TAG, <&mut Oct>::TAG, Oct.own());
    let mut o = Oct;
    println!("{} {} {}", tag(Oct), tag(&Oct), tag(&mut o));
    println!("{} {} {}", tag_of::<Oct>(), tag_of::<&Oct>(), tag_of::<&mut Oct>());
    println!("{} {}", <&Only>::TAG, tag(&Only));
    println!("{} {}", Oct.tag(), (&Oct).tag());
}
