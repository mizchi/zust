// `saturating_abs` exists only on signed integers: on u32 it is no method (#187: it was an internal error)
fn main() {
    let x: u32 = 3;
    println!("{}", x.saturating_abs());
}
