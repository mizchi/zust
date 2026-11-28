// Methods of std's that zustc lacked: the euclidean checked / wrapping / overflowing division and remainder,
// saturating_mul, `&str`'s eq / ne (also as a generic fn's T = &str), and a derived PartialEq /
// PartialOrd's eq / ne / lt / .. called as methods
#[derive(PartialEq, PartialOrd, Debug)]
struct M {
    n: i32,
}
#[derive(PartialEq, Debug)]
enum E {
    A,
    B(u8),
}
fn same<T: Eq>(a: T, b: T) -> bool {
    a.eq(&b)
}
fn main() {
    println!("{:?} {:?} {:?}", (i8::MIN + 1).checked_div_euclid(-1), i8::MIN.checked_div_euclid(-1), 1i8.checked_div_euclid(0));
    println!("{:?} {:?} {:?}", (-7i32).checked_rem_euclid(3), i32::MIN.checked_rem_euclid(-1), 7u8.checked_rem_euclid(0));
    println!("{} {} {:?} {:?}", i8::MIN.wrapping_div_euclid(-1), i8::MIN.wrapping_rem_euclid(-1), i8::MIN.overflowing_div_euclid(-1), (-7i16).overflowing_rem_euclid(2));
    println!("{} {} {} {} {}", 100i8.saturating_mul(2), (-100i8).saturating_mul(2), (-100i8).saturating_mul(-2), 200u8.saturating_mul(2), 3i32.saturating_mul(-4));
    let a: &str = "x";
    println!("{} {} {} {}", a.eq("x"), a.ne("y"), same("p", "p"), same("p", "q"));
    let (x, y) = (M { n: 1 }, M { n: 2 });
    println!("{} {} {} {} {} {}", x.eq(&y), x.ne(&y), x.lt(&y), x.le(&y), x.gt(&y), x.ge(&y));
    let r = &x;
    println!("{} {}", r.eq(&x), E::B(3).eq(&E::B(3)) && E::A.ne(&E::B(1)));
}
