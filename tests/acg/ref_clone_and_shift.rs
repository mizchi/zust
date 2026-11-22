// `r.clone()` of a reference to a type that is not Clone clones the reference; a shift of `&0` takes the
// annotation's type; `let _: T = e` checks e against T
use std::rc::Rc;

struct Token(u32);

#[derive(Clone, Debug)]
struct Point(i32, i32);

fn main() {
    let t = &Token(7);
    let u: &Token = t.clone();
    println!("{}", u.0);
    let o = &Some(Token(8));
    let p: &Option<Token> = o.clone();
    if let Some(x) = p {
        println!("{}", x.0);
    }
    let q = &Point(1, 2);
    let r: Point = q.clone();
    println!("{:?}", r);
    let s = &Rc::new(Token(9));
    let c: Rc<Token> = s.clone();
    println!("{} {}", c.0, Rc::strong_count(&c));
    let a: u8 = &1 << 3;
    let _: u8 = &1 << &2;
    let _: u16 = 300;
    let _: &str = &String::from("x");
    println!("{a}");
}
