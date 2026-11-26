// Region check part 39: `<dyn Error + 'static>::is`, a `let` assigned later through a deref (RefCell's `Ref`, Rc, a
// user Deref, String, Vec), and closures held as fn pointers with holes (`fn(_, _) -> _`), run against rustc's output
use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;
#[derive(Debug)]
struct A;
impl fmt::Display for A {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "A")
    }
}
impl Error for A {}
#[derive(Debug)]
struct B;
impl fmt::Display for B {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "B")
    }
}
impl Error for B {}
fn check(e: &(dyn Error + 'static)) -> (bool, bool) {
    (e.is::<A>(), e.is::<B>())
}
struct W(u8);
impl Deref for W {
    type Target = u8;
    fn deref(&self) -> &u8 {
        &self.0
    }
}
struct G<T>(T);
impl<T> Deref for G<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
fn second<'a>() -> fn(&'a u32, &'a u32) -> &'a u32 {
    let g: fn(_, _) -> _ = |_x, y| y;
    g
}
fn first<'a, 'b>() -> fn(&'a u32, &'b u32) -> &'a u32 {
    let g: fn(_, _) -> _ = |x, _y| x;
    g
}
fn main() {
    println!("{:?} {:?}", check(&A), check(&B));
    let m = RefCell::new(4);
    let p;
    {
        let b = m.borrow();
        p = *b;
    }
    let s = String::from("ab");
    let a;
    a = &*s;
    let v = vec![1u8, 2, 3];
    let c;
    c = &*v;
    let r = Rc::new(5u32);
    let d;
    d = *r;
    let w = W(7);
    let e;
    e = *w;
    let g = G(3i64);
    let h;
    h = *g;
    println!("{} {} {} {} {} {}", p, a.len(), c.len(), d + 1, e + 1, h * 2);
    let add: fn(_, _) -> _ = |x: u8, y: u8| x + y;
    println!("{} {} {}", second()(&1, &2), first()(&3, &4), add(5, 6));
}
