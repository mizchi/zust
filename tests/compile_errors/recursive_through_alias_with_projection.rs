trait Tr { type A; }
impl Tr for () { type A = u8; }
type Alias<T> = (T, <() as Tr>::A);
struct Bad { x: Alias<Bad> }
fn main() {}
