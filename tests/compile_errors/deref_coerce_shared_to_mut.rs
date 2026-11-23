// `&&mut u8` is no `&mut u8`: the deref coercion does not reach a `&mut` through a shared reference
fn get_mut<'a>(x: &'a &'a mut u8) -> &'a mut u8 { x }
fn main() { let mut v = 1u8; let r = &mut v; println!("{}", get_mut(&r)); }
