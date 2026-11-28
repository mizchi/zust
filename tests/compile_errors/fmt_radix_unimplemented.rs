// (`{:x}` of a type with no LowerHex: E0277, the trait bound, not a Display error)
struct S;
fn main() {
    println!("{:x}", S);
}
