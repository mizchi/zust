struct S;
impl S { fn run(&self, x: u8) -> u32 { x as u32 } }
trait T { fn run(&self, x: u16) -> u32; }
impl T for S { fn run(&self, x: u16) -> u32 { x as u32 + 1000 } }
fn main() { let s = S; println!("{}", s.run(7u16)); }
