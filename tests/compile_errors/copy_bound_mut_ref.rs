struct C<T: Copy> { value: T }
fn main() { let mut n = 1u32; let c = C::<&mut u32> { value: &mut n }; *c.value += 1; }
