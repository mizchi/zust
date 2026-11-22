// `<T as Clone>::clone(x)`: T's clone, a derive's too, also of a reference type
#[derive(Clone, Debug)]
struct P(u8);
// (an inherent `clone` beside Clone's: the qualified call takes Clone's, the method call the inherent one)
struct Q(u8);
impl Q {
    fn clone(&self) -> u32 {
        99
    }
}
impl Clone for Q {
    fn clone(&self) -> Q {
        Q(self.0 + 1)
    }
}
fn main() {
    let q = Q(1);
    let c = <Q as Clone>::clone(&q);
    println!("{} {}", c.0, q.clone());
    let s = P(3);
    let t = <P as Clone>::clone(&s);
    let u = <&P as Clone>::clone(&&s);
    let v = Clone::clone(&s);
    println!("{:?} {:?} {:?}", t, u, v);
}
