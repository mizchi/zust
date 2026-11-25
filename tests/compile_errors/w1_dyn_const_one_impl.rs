trait Obj {}
trait Flag { const N: bool; }
impl Flag for dyn Obj { const N: bool = false; const N: bool = false; }
fn main() {}
