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

// a struct laid out before the definition of the struct with an unsized tail it points to
// (a tail through an alias, requested (by AHolder's field) before AOuter's fields are resolved)
struct AHolder<T> {
    p: *const AOuter<[u8]>,
    marker: T,
}
struct AInner<T: ?Sized>(u8, T);
type AAlias<T> = AInner<T>;
struct AOuter<T: ?Sized>(u16, AAlias<T>);

struct Holder<T> {
    p: *const Tail<[u8]>,
    marker: T,
}
struct Tail<T: ?Sized>(T);

// a tail nested ten structs deep
struct W0<T: ?Sized>(T);
struct W1<T: ?Sized>(W0<T>);
struct W2<T: ?Sized>(W1<T>);
struct W3<T: ?Sized>(W2<T>);
struct W4<T: ?Sized>(W3<T>);
struct W5<T: ?Sized>(W4<T>);
struct W6<T: ?Sized>(W5<T>);
struct W7<T: ?Sized>(W6<T>);
struct W8<T: ?Sized>(W7<T>);
struct W9<T: ?Sized>(W8<T>);

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

    let t = Tail([1u8, 2, 3]);
    let h = Holder { p: &t as *const Tail<[u8; 3]> as *const Tail<[u8]>, marker: 0u8 };
    println!("{} {} {}", std::mem::size_of::<Holder<u8>>(), unsafe { (&(*h.p).0).len() }, h.marker);
    let w: &W9<[u8]> = &W9(W8(W7(W6(W5(W4(W3(W2(W1(W0([1u8, 2]))))))))));
    println!("{} {}", std::mem::size_of::<&W9<[u8]>>(), std::mem::size_of_val(w));
    // (dropped in place once, through a pointer to the unsized struct; ManuallyDrop keeps it from a second drop)
    let mut gm = std::mem::ManuallyDrop::new(Guard { id: 3, tail: [Loud(4)] });
    let gp: *mut Guard<[Loud]> = &mut *gm as &mut Guard<[Loud; 1]> as *mut Guard<[Loud; 1]> as *mut Guard<[Loud]>;
    unsafe { std::ptr::drop_in_place(gp) };
    let _ah: Option<AHolder<u8>> = None;
    println!("{} {}", std::mem::size_of::<&AOuter<[u8]>>(), std::mem::size_of::<AHolder<u8>>());
    println!("end");
}
