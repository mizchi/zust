use std::cmp::Ordering;
#[derive(PartialEq, Debug, Clone, Copy)]
enum E { A, B }
struct S(i32);
impl PartialEq<i32> for S { fn eq(&self, o: &i32) -> bool { self.0 == *o } }
struct P(i32);
impl PartialEq for P { fn eq(&self, o: &P) -> bool { self.0 == o.0 } }
impl PartialOrd for P { fn partial_cmp(&self, o: &P) -> Option<Ordering> { self.0.partial_cmp(&o.0) } }
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Q { a: u8 }
struct W<T>(T);
impl<T: PartialEq> PartialEq for W<T> { fn eq(&self, o: &Self) -> bool { self.0 == o.0 } }
mod m { #[derive(PartialEq)] pub struct R; }
fn main() {
    let e = E::A;
    println!("{} {}", e == E::A, e != E::B);
    println!("{}", S(3) == 3);
    println!("{} {}", P(1) == P(1), P(1) < P(2));
    println!("{}", Q { a: 1 } < Q { a: 2 });
    println!("{}", W(1) == W(1));
    println!("{}", [E::A, E::B] == [E::A, E::B]);
    println!("{}", (E::A, 1) == (E::A, 1));
    let r = &P(2);
    println!("{}", r == &P(2));
    println!("{}", m::R == m::R);
    let v = vec![Q { a: 1 }];
    println!("{}", v[0] == Q { a: 1 });
    println!("{:?}", by_ref());
}
// (`impl PartialEq for &X` only: `&X(1) == &X(2)` compares through it, not by the fields)
struct RX(i32);
impl PartialEq for &RX {
    fn eq(&self, o: &Self) -> bool {
        self.0 % 2 == o.0 % 2
    }
}
#[allow(dead_code)]
fn by_ref() -> (bool, bool, bool) {
    (&RX(1) == &RX(3), &RX(1) == &RX(2), &RX(2) != &RX(4))
}
