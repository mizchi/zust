// Part 40 of the region check: std::cell::OnceCell, Rc / Arc's try_unwrap and into_inner, a method of an impl for
// `Box<dyn Tr>` in an `impl Trait` result, and values of types that bound a parameter by 'static built where the
// parameter is 'static
use std::cell::OnceCell;
use std::fmt::Display;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

struct D(u8);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

type Storage = Rc<OnceCell<Box<dyn Display + 'static>>>;

fn store<T: Display + 'static>(x: T, s: Storage) {
    s.set(Box::new(x)).ok().unwrap();
}

trait O {
    fn v(&self) -> u8;
}
trait MyTrait {
    fn use_self(&self) -> &u8;
}
impl MyTrait for Box<dyn O> {
    fn use_self(&self) -> &u8 {
        &7
    }
}
struct A;
impl O for A {
    fn v(&self) -> u8 {
        4
    }
}
fn through_box(val: &Box<dyn O>) -> impl Display + '_ {
    val.use_self()
}

struct St<T: 'static>(Option<T>);
enum En<T: 'static> {
    A(T),
    B,
}
struct Ph<T: 'static>(PhantomData<T>);
impl<T: 'static> St<T> {
    fn new(x: T) -> Self {
        Self(Some(x))
    }
}
fn wrap<T: 'static>(x: T) -> En<T> {
    if true { En::A(x) } else { En::<T>::B }
}
fn phantom<T>() -> u8
where
    T: 'static,
{
    let _ = Ph::<T>(PhantomData);
    1
}
// (the signature's `St<T>` implies T: 'static in the body, in its closures too)
fn implied<T>(s: St<T>) -> usize {
    let f = || {
        let _ = Ph::<T>(PhantomData);
    };
    f();
    s.0.is_some() as usize
}

fn main() {
    let c: OnceCell<u32> = OnceCell::new();
    println!("{:?} {:?}", c.get(), c.set(5));
    println!("{:?} {:?}", c.set(6), c.get());
    let d = OnceCell::new();
    let v = d.get_or_init(|| String::from("x"));
    println!("{} {:?}", v, d.get());
    let mut e = OnceCell::new();
    let _ = e.set(1i64);
    println!("{:?} {:?}", e.take(), e.into_inner());

    let a = Rc::new(D(1));
    let b = a.clone();
    let r = Rc::try_unwrap(a);
    println!("{}", r.is_err());
    let a = r.err().unwrap();
    drop(b);
    let v = Rc::try_unwrap(a).ok().unwrap();
    println!("got {}", v.0);
    let c1 = Rc::new(D(2));
    let c2 = c1.clone();
    println!("{}", Rc::into_inner(c1).is_none());
    println!("{}", Rc::into_inner(c2).map(|d| d.0).unwrap());
    println!("{:?}", Arc::into_inner(Arc::new(String::from("s"))));

    let s: Storage = Rc::new(OnceCell::new());
    store(9u8, s.clone());
    println!("{}", Rc::into_inner(s).unwrap().into_inner().unwrap());

    let bx: Box<dyn O> = Box::new(A);
    println!("{} {}", through_box(&bx), bx.v());

    let st = St::new(3);
    println!("{} {} {}", st.0.unwrap(), matches!(wrap(1u8), En::A(1)), phantom::<u8>());
    println!("{}", implied(St(Some(2))));
    println!("{}", v.0);
}
