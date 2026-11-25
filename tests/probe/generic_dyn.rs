// Trait objects with type parameters: in a generic type's fields, in a generic fn's parameters and result, and
// closures coerced to them where the parameter is not known yet.
trait A<T> {
    fn get(&self) -> T;
}

struct B<'a, T: 'a>(&'a (dyn A<T> + 'a));

impl A<i32> for i32 {
    fn get(&self) -> i32 {
        *self
    }
}

impl A<String> for u8 {
    fn get(&self) -> String {
        format!("u8:{}", self)
    }
}

struct S<U> {
    f: Box<dyn Fn(U) -> U>,
}

fn mk() -> S<i32> {
    S { f: Box::new(|x| x + 1) }
}

fn app<T>(s: &S<T>, x: T) -> T {
    (s.f)(x)
}

fn by_dyn(v: &dyn A<i32>) -> i32 {
    let b = B(v);
    b.0.get()
}

fn wrap<'a>(v: &'a dyn A<String>) -> B<'a, String> {
    B(v)
}

fn unbox<X>(v: Box<dyn A<X>>) -> X {
    v.get()
}

fn repeat<'a, T: Clone + 'a>(t: T) -> Box<dyn FnMut() -> T + 'a> {
    Box::new(move || t.clone())
}

fn apply<T>(f: Box<dyn Fn(T) -> T>, x: T) -> T {
    f(x)
}

fn twice<T: Copy>(f: &dyn Fn(T) -> T, x: T) -> T {
    f(f(x))
}

fn each<T>(v: Vec<T>, f: &mut dyn FnMut(T)) {
    for x in v {
        f(x);
    }
}

fn main() {
    let s = mk();
    println!("{} {}", app(&s, 3), (s.f)(10));
    println!("{}", by_dyn(&3));
    println!("{}", wrap(&7u8).0.get());
    println!("{}", unbox(Box::new(5u8) as Box<dyn A<String>>));
    let mut c = repeat(42u32);
    println!("{}", c());
    let mut d = repeat("forty-two");
    println!("{}", d());
    println!("{}", apply(Box::new(|x| x + 1), 3));
    let typed: Box<dyn Fn(i64) -> i64> = Box::new(|x| x * 3);
    println!("{}", apply(typed, 5));
    println!("{}", twice(&|x| x * 2, 3));
    let mut sum = 0;
    each(vec![1, 2, 3], &mut |x| sum += x);
    println!("{}", sum);
}
