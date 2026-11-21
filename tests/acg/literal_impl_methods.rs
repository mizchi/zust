trait MyCmp { fn cmp(&self) -> u32 { 7 } }
impl MyCmp for f32 {}
trait Tag { fn tag(&self) -> u8; }
impl Tag for f32 { fn tag(&self) -> u8 { 32 } }
impl Tag for u8 { fn tag(&self) -> u8 { 8 } }
trait Wide { fn wide(&self) -> u64; }
impl Wide for u8 { fn wide(&self) -> u64 { *self as u64 * 2 } }
fn main() {
    // (a suffixed literal keeps its type: `1f32` is no integer literal)
    println!("{} {}", 1f32.tag(), 1.0.tag());
    println!("{}", 0.0.cmp());
    println!("{}", 21.wide());
    println!("{}", 3.0_f64.sqrt() as i32);
}
