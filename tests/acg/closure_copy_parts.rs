// A `move` closure that uses only `Copy` parts of a variable that needs dropping: the closure's copy of the other
// parts is emptied and the variable stays where it was, as rustc captures only the parts (the Rc counts show it)
use std::rc::Rc;

struct P {
    n: u32,
    r: Rc<i32>,
}

fn make(p: P) -> impl Fn() -> u32 {
    move || p.n + 1
}

enum E {
    V(u32, Rc<i32>),
}

fn main() {
    let shared = Rc::new(5);
    let p = P { n: 7, r: shared.clone() };
    let c = move || p.n * 2;
    println!("{}", Rc::strong_count(&shared));
    drop(p.r);
    println!("{}", Rc::strong_count(&shared));
    println!("{}", c());
    drop(c);
    println!("{}", Rc::strong_count(&shared));
    let mut q = P { n: 1, r: shared.clone() };
    let mut d = move || {
        q.n += 1;
        q.n
    };
    println!("{} {}", d(), d());
    println!("{} {}", q.n, Rc::strong_count(&q.r));
    drop(d);
    println!("{}", Rc::strong_count(&shared));
    let f = make(P { n: 41, r: shared.clone() });
    println!("{} {}", f(), Rc::strong_count(&shared));
    // (an enum of one variant: the copy keeps its `Copy` field)
    let e = E::V(7, shared.clone());
    let g = move || {
        let E::V(n, _) = e;
        n
    };
    println!("{} {}", g(), Rc::strong_count(&shared));
}
