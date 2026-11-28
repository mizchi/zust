// (std's Debug for Any is for `dyn Any`, `dyn Any + Send` and `dyn Any + Send + Sync`: not `dyn Any + Sync` (E0277))
use std::any::Any;
fn main() {
    let x: &(dyn Any + Sync) = &1u8;
    println!("{:?}", x);
}
