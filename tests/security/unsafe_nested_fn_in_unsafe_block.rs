//! A fn item written inside an `unsafe { }` block is not itself unsafe code (E0133): its body is checked.
//@ pattern: E0133 context
//@ todo: rejected with E0133, but at a wrong line (a nested fn's lines counted from its item)
//@ expect: reject
fn main() {
    let v = unsafe {
        fn inner(p: *const u8) -> u8 {
            *p //~ ERROR E0133
        }
        inner(&7u8)
    };
    println!("{v}");
}
