//! Neighbors of send_rc_in_struct_thread: an `Rc` ten levels deep in the program's own types is still not Send
//! (E0277). The Send check follows each type once, so a recursive type (an expression tree) does not make it slow,
//! and no depth makes it give up and say Send.
//@ pattern: Send / Sync
//@ expect: reject
use std::rc::Rc;
enum Expr {
    Num(i64),
    Add(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}
struct L1(L2);
struct L2(L3);
struct L3(L4);
struct L4(L5);
struct L5(L6);
struct L6(L7);
struct L7(L8);
struct L8(L9);
struct L9(L10);
struct L10(Option<Rc<u8>>);
fn main() {
    let e = Expr::Add(Box::new(Expr::Num(1)), Box::new(Expr::Neg(Box::new(Expr::Mul(Box::new(Expr::Num(2)), Box::new(Expr::Num(3)))))));
    let deep = L1(L2(L3(L4(L5(L6(L7(L8(L9(L10(None))))))))));
    let h = std::thread::spawn(move || { //~ ERROR E0277
        let _ = &e;
        let _ = &deep;
    });
    h.join().unwrap();
}
