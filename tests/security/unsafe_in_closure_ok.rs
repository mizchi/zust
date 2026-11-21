//! The other side of unsafe_in_closure: a closure written inside `unsafe { }` or an `unsafe fn` may
//! do unsafe operations (and so may closures nested in it). These must still build and run.
//@ expect: run
unsafe fn in_unsafe_fn(p: *const u32) -> u32 {
    let f = || unsafe { *p } + 1;
    let g = |q: *const u32| unsafe { *q };
    f() + g(p)
}
fn main() {
    let x = 41u32;
    let p = &x as *const u32;
    let r = unsafe {
        let f = || *p;
        let nested = || {
            let inner = |k: u32| *p + k;
            inner(1)
        };
        f() + nested()
    };
    let safe_closure = |a: u32| a * 2;
    println!("{} {} {}", r, unsafe { in_unsafe_fn(p) }, safe_closure(3));
}
