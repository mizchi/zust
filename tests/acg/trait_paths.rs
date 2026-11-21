// A trait's method named by its path (`Foo::<i32>::tag(&s)`, `Bar::tag(&s)`) is the trait's, not an inherent method
// of the same name; an attribute in an `extern "C" fn` body's macro argument is no attribute
trait Foo<T> {
    fn tag(&self) -> usize;
}
trait Bar {
    fn tag(&self) -> usize;
}
struct S;
impl S {
    fn tag(&self) -> usize {
        1
    }
}
impl Foo<i32> for S {
    fn tag(&self) -> usize {
        2
    }
}
impl Bar for S {
    fn tag(&self) -> usize {
        3
    }
}
#[allow(dead_code)]
extern "C" fn f() -> usize {
    let s: &str = stringify!(#[thread_local]);
    s.len()
}
fn main() {
    let s = S;
    println!("{} {} {}", Foo::<i32>::tag(&s), Bar::tag(&s), s.tag());
    println!("{}", f());
}
