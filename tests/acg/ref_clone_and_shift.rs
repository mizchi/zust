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
    let v = &vec![Token(10)];
    let w = Some(v.clone());
    let vr: Option<&Vec<Token>> = w;
    println!("{}", vr.map_or(0, |v| v.len()));
    let (tx, rx) = std::sync::mpsc::channel::<Token>();
    let rt = &tx;
    let tx2: std::sync::mpsc::Sender<Token> = rt.clone();
    let _ = tx2.send(Token(11));
    drop(tx2);
    println!("{}", rx.recv().unwrap().0);
    let toks = [Token(12), Token(13)];
    let it = toks.iter();
    let ri = &it;
    let mut it2 = ri.clone();
    it2.next();
    println!("{} {}", it2.next().unwrap().0, it.len());
    let mut n = 14;
    let om = Some(&mut n);
    let rm = &om;
    let cm: &Option<&mut i32> = rm.clone();
    println!("{}", cm.is_some());
    let a: u8 = &1 << 3;
    let _: u8 = &1 << &2;
    let _: u16 = 300;
    let _: &str = &String::from("x");
    println!("{a}");
}
