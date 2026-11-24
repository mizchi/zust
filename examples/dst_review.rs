// Structs with an unsized tail, the cases a review found: a method on `&Wrap<[u8]>` through a box, an explicit
// `&Wrap { .. }` pattern, a recursive struct, a tail nested through three structs, a generic call whose argument is
// unsized already, and a struct's own `impl Drop` when its box is dropped.

struct Wrap<T: ?Sized> {
    a: u8,
    tail: T,
}

impl Wrap<[u8]> {
    fn sum(&self) -> u32 {
        self.a as u32 + self.tail.iter().map(|&b| b as u32).sum::<u32>()
    }
}

struct Node<T: ?Sized> {
    next: Option<Box<Node<u8>>>,
    tail: T,
}

struct Inner<T: ?Sized>(u8, T);
struct Middle<T: ?Sized>(u16, Inner<T>);
struct Outer<T: ?Sized>(u32, Middle<T>);

fn view<T: ?Sized>(x: &Wrap<T>) -> &T {
    &x.tail
}

fn first<T: Copy>(x: &Wrap<[T]>) -> T {
    x.tail[0]
}

struct Loud(u8);
impl Drop for Loud {
    fn drop(&mut self) {
        println!("drop Loud {}", self.0);
    }
}

struct Guard<T: ?Sized> {
    id: u8,
    tail: T,
}
impl<T: ?Sized> Drop for Guard<T> {
    fn drop(&mut self) {
        println!("drop Guard {}", self.id);
    }
}

trait Named {
    fn n(&self) -> u8;
}
impl Named for Loud {
    fn n(&self) -> u8 {
        self.0
    }
}

fn main() {
    let w: &Wrap<[u8]> = &Wrap { a: 1, tail: [2, 3] };
    println!("{}", w.sum());
    let b: Box<Wrap<[u8]>> = Box::new(Wrap { a: 4, tail: [5, 6, 7] });
    println!("{} {}", b.sum(), b.tail.len());
    let &Wrap { a, tail: ref bytes } = w;
    println!("{} {:?}", a, bytes);
    let n: &Node<[u8]> = &Node { next: None, tail: [1, 2, 3] };
    println!("{} {}", n.tail.len(), n.next.is_none());
    let o: &Outer<[u8]> = &Outer(1, Middle(2, Inner(3, [4, 5, 6, 7])));
    println!("{} {}", (o.1).1 .1.len(), std::mem::size_of_val(o));
    println!("{}", view(w).len());
    let x: u8 = first(&Wrap { a: 0, tail: [9, 2] });
    println!("{}", x);

    let g: Box<Guard<[Loud]>> = Box::new(Guard { id: 1, tail: [Loud(1), Loud(2)] });
    println!("{}", g.tail.len());
    drop(g);
    let d: Box<Guard<dyn Named>> = Box::new(Guard { id: 2, tail: Loud(3) });
    println!("{}", d.tail.n());
}
