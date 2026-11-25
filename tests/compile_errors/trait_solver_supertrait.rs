// An impl needs its trait's supertraits, all the way up, under its own bounds: `impl<T> Complete for T` needs
// `T: Partial`, which (through `impl<T> Partial for T where T: Complete`) needs `T: Copy`
pub trait Partial: Copy {}
pub trait Complete: Partial {}
impl<T> Partial for T where T: Complete {}
impl<T> Complete for T {}
fn main() {}
