// A std path with type arguments on the type, `core::mem::MaybeUninit::<S>::uninit()` (libc 0.2's offset_of!), and a
// `let` whose pattern binds no name, `let S { b: _, .. };`
struct S {
    a: u8,
    b: u32,
}

fn main() {
    let S { b: _, .. };
    let data = core::mem::MaybeUninit::<S>::uninit();
    let _ptr = data.as_ptr();
    let v = std::vec::Vec::<u8>::new();
    let c = core::cell::Cell::<u8>::new(1);
    c.set(c.get() + 1);
    let s = S { a: 1, b: 2 };
    println!("{} {} {} {}", v.len(), c.get(), s.a, s.b);
}
