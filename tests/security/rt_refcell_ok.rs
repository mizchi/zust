//! The other side of rt_refcell: the ways RefCell is used every day still work with its guards. A guard is released
//! at the end of its statement (a `let` initializer's too), at the end of its binding's scope, or by `drop`; the guards
//! deref to the value for methods, fields, indexing, `*`, formatting and the coercion to `&T` / `&[T]` / `&str`.
//@ rule: RefCell's run-time borrow flags
//@ expect: run
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;
#[derive(Debug, Clone)]
struct Node {
    name: String,
    val: i32,
    kids: Vec<Rc<RefCell<Node>>>,
}
struct Bank {
    accounts: RefCell<HashMap<String, i64>>,
    log: RefCell<Vec<String>>,
}
impl Bank {
    fn deposit(&self, who: &str, n: i64) {
        *self.accounts.borrow_mut().entry(who.to_string()).or_insert(0) += n;
        self.log.borrow_mut().push(format!("{who}+{n}"));
    }
    fn total(&self) -> i64 {
        self.accounts.borrow().values().sum()
    }
    fn names(&self) -> Ref<'_, Vec<String>> {
        self.log.borrow()
    }
}
fn takes_slice(v: &[i32]) -> i32 {
    v.iter().sum()
}
fn takes_str(s: &str) -> usize {
    s.len()
}
fn bump(mut g: RefMut<'_, i32>) {
    *g += 100;
}
fn main() {
    let c = RefCell::new(5);
    *c.borrow_mut() += 1;
    let x = *c.borrow();
    println!("{x} {}", c.borrow());
    {
        let a = c.borrow();
        let b = c.borrow();
        println!("{} {}", *a + *b, c.try_borrow_mut().is_err());
    }
    println!("{}", c.try_borrow_mut().is_ok());
    let v = RefCell::new(vec![1, 2, 3]);
    v.borrow_mut().push(4);
    println!("{:?} {} {}", v.borrow(), v.borrow().len(), v.borrow()[2]);
    let s: i32 = v.borrow().iter().sum();
    println!("{s} {}", takes_slice(&v.borrow()));
    for x in v.borrow().iter() {
        print!("{x} ");
    }
    println!();
    let cl = v.borrow().clone();
    println!("{:?}", cl);
    let st = RefCell::new(String::from("hi"));
    st.borrow_mut().push_str(" there");
    println!("{} {}", takes_str(&st.borrow()), st.borrow().to_uppercase());
    let leaf = Rc::new(RefCell::new(Node { name: "leaf".into(), val: 3, kids: vec![] }));
    let root = Rc::new(RefCell::new(Node { name: "root".into(), val: 1, kids: vec![leaf.clone()] }));
    leaf.borrow_mut().val += 10;
    println!("{} {}", root.borrow().kids[0].borrow().val, Rc::strong_count(&leaf));
    let names: Vec<String> = root.borrow().kids.iter().map(|n| n.borrow().name.clone()).collect();
    println!("{:?}", names);
    let copy: Node = leaf.borrow().clone();
    println!("{} {}", copy.name, copy.val);
    let bank = Bank { accounts: RefCell::new(HashMap::new()), log: RefCell::new(vec![]) };
    bank.deposit("a", 5);
    bank.deposit("b", 7);
    bank.deposit("a", 1);
    println!("{} {}", bank.total(), bank.names().len());
    let g = c.borrow_mut();
    drop(g);
    bump(c.borrow_mut());
    println!("{}", c.borrow());
    let old = c.replace(1);
    let t = c.take();
    println!("{old} {t} {}", c.borrow());
    if let Some(first) = v.borrow().first() {
        println!("first {first}");
    }
    match *c.borrow() {
        0 => println!("zero"),
        n => println!("n={n}"),
    }
    let shared = Rc::new(RefCell::new(0));
    let adders: Vec<Box<dyn Fn()>> = (0..3).map(|i| {
        let s = shared.clone();
        Box::new(move || *s.borrow_mut() += i) as Box<dyn Fn()>
    }).collect();
    for f in &adders {
        f();
    }
    println!("{}", shared.borrow());
    // (a RefMut in a binding that is not `mut` is read through `deref`: `&self` methods, indexing, `*`)
    let cells = RefCell::new(vec![3, 1, 2]);
    let g = cells.borrow_mut();
    let s: i32 = g.iter().sum();
    println!("{s} {} {} {:?}", g[0], g.len(), *g);
    drop(g);
    cells.borrow_mut().sort();
    println!("{:?}", cells.borrow());
    // (a `&mut self` method of a trait's on a primitive type, through the guard: it changes the cell, not a copy)
    let n = RefCell::new(5i32);
    n.borrow_mut().bump();
    println!("{}", n.borrow());
}

trait Bump {
    fn bump(&mut self);
}
impl Bump for i32 {
    fn bump(&mut self) {
        *self += 1;
    }
}
