// A 128-bit receiver whose type the program never spells (from #187's review): NonZeroU128::get gives one, and the
// prelude's 128-bit helpers (division, isqrt) come in with it, not only with the name of the type in the source
use std::num::NonZeroU128;

fn main() {
    let nz = NonZeroU128::new(170).unwrap();
    println!("{} {} {}", nz.get().isqrt(), nz.get() / 3, nz.get().wrapping_rem(7));
    println!("{:?}", nz.get().overflowing_div(4));
}
