// Ordering comparisons in a small program without the glue (__exr_cmp): codegen expands tuples, arrays and derive(PartialOrd) enums in place
#[derive(PartialEq, PartialOrd, Clone, Copy)]
enum Lvl { Lo, Hi(u8) }
fn main() {
    let a = (1, 2);
    let b = (1, 3);
    if a < b {
        println!("lt");
    }
    let x = [(1u8, Lvl::Lo), (0, Lvl::Hi(2))];
    let y = [(1u8, Lvl::Hi(0)), (0, Lvl::Lo)];
    println!("{} {} {}", x < y, Lvl::Hi(3) > Lvl::Hi(2), &a >= &b);
}
