trait MyCmp { fn cmp(&self) -> u32 { 7 } }
impl MyCmp for f32 {}
trait Wide { fn wide(&self) -> u64; }
impl Wide for u8 { fn wide(&self) -> u64 { *self as u64 * 2 } }
fn main() {
    println!("{}", 0.0.cmp());
    println!("{}", 21.wide());
    println!("{}", 3.0_f64.sqrt() as i32);
}
