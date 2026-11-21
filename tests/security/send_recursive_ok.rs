//! The other side of send_deep_and_recursive: a recursive type of the program's that is Send moves to a thread.
//@ pattern: Send / Sync
//@ expect: run
enum Expr {
    Num(i64),
    Add(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
}
fn eval(e: &Expr) -> i64 {
    match e {
        Expr::Num(n) => *n,
        Expr::Add(a, b) => eval(a) + eval(b),
        Expr::Mul(a, b) => eval(a) * eval(b),
        Expr::Neg(a) => -eval(a),
    }
}
fn main() {
    let e = Expr::Add(Box::new(Expr::Num(1)), Box::new(Expr::Neg(Box::new(Expr::Mul(Box::new(Expr::Num(2)), Box::new(Expr::Num(3)))))));
    let h = std::thread::spawn(move || -> i64 { eval(&e) });
    println!("{}", h.join().unwrap());
}
