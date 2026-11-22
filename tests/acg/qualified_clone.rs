// `<T as Clone>::clone(x)`: T's clone, a derive's too, also of a reference type
#[derive(Clone, Debug)]
struct P(u8);
fn main() {
    let s = P(3);
    let t = <P as Clone>::clone(&s);
    let u = <&P as Clone>::clone(&&s);
    let v = Clone::clone(&s);
    println!("{:?} {:?} {:?}", t, u, v);
}
