// A `match` value as a `&self` receiver is a temporary: a result borrowing it is not used after the statement (E0716)
#[derive(Clone, Copy)]
struct S(u32);
trait Loan { fn loan(&self) -> &S; }
impl Loan for S { fn loan(&self) -> &S { self } }
fn mk(x: u32) -> S { S(x) }
fn main() {
    let f = true;
    let r = (match f { true => mk(1), false => mk(2) }).loan();
    println!("{}", r.0);
}
