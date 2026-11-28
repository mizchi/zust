// (a program's own trait named Any has no Debug: std's `Any { .. }` is only for std::any::Any (E0277))
mod custom {
    pub trait Any {}
    impl Any for u8 {}
}
fn main() {
    let x: &dyn custom::Any = &1u8;
    println!("{:?}", x);
}
