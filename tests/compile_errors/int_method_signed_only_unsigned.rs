// `next_multiple_of` is stable only on unsigned integers: on i32 it is no method (#187: it was an internal error)
fn main() {
    let x: i32 = 3;
    println!("{}", x.next_multiple_of(2));
}
