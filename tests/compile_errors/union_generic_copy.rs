// A generic union's instance: its fields are `Copy` or `ManuallyDrop`, as the definition's bound requires
use std::mem::ManuallyDrop;
use std::rc::Rc;

union U<T: Copy> {
    a: T,
}

union M<T> {
    a: ManuallyDrop<T>,
}

fn main() {
    let _ok = U::<u32> { a: 1 };
    let _m = M { a: ManuallyDrop::new(Rc::new(0u32)) };
    let _u = U::<Rc<u32>> { a: Default::default() };
}
