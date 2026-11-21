//! Calling an integer as a function pointer: a table index that is not there, never a jump to arbitrary code.
fn main() {
    let f: fn() = unsafe { std::mem::transmute(std::hint::black_box(123456usize)) };
    f();
    println!("after");
}
