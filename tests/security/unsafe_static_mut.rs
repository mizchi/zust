//! Reading or writing a `static mut` from safe code (E0133): it is a data race in the making.
//@ expect: reject
static mut COUNTER: u32 = 0;
fn main() {
    COUNTER += 1; //~ ERROR E0133
}
