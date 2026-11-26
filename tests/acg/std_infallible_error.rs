// Infallible is an Error and a Display: rand_core's `type Error: Error` with `type Error = Infallible` (#250)
use std::convert::Infallible;
use std::error::Error;
fn needs_err<E: Error>(_e: &E) -> &'static str { "an error" }
trait TryRng { type Error: Error; fn try_next(&mut self) -> Result<u32, Self::Error>; }
struct R(u32);
impl TryRng for R { type Error = Infallible; fn try_next(&mut self) -> Result<u32, Infallible> { self.0 += 1; Ok(self.0) } }
fn main() {
    let mut r = R(4);
    let v: Result<u32, Infallible> = r.try_next();
    println!("{:?}", v);
    let r2: Result<u8, Infallible> = Ok(7);
    if let Err(e) = &r2 { println!("{}", needs_err(e)); }
}
