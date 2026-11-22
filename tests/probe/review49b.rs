// Review cases of #49 (2): a u64 repr discriminant widened, a type alias beside a value binding, a program's From into Box
type Mask = u16;
#[repr(u64)]
#[derive(Clone, Copy)]
enum E { A = 1u64 << 63 }
const HI: u128 = (E::A as u128) >> 127;
const HI2: u128 = (E::A as u128) >> 63;
struct Src(u64);
impl From<Src> for Box<u64> { fn from(s: Src) -> Box<u64> { Box::new(s.0 + 1) } }
fn main() {
    let x: u8 = 7;
    let shift = 1;
    let y = x.wrapping_shl(shift);
    let _: u8 = x;
    #[allow(non_snake_case)]
    let Mask = ();
    let _ = Mask;
    let bits = Mask::BITS;
    let b: Box<u64> = Box::from(Src(4));
    println!("{} {} {} {} {}", y, bits, HI, HI2, b);
}
