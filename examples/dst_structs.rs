// Structs whose last field is unsized (`Foo<[T]>`): `&Foo<[T]>` is a (data, length) pair, as `&[T]` is,
// made by coercing `&Foo<[T; N]>` (and `&mut`, `*const`, `Box`).

struct Fat<T: ?Sized> {
    f1: isize,
    f2: &'static str,
    ptr: T,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct Bar(u8);

trait ToBar {
    fn to_bar(&self) -> Bar;
}

impl ToBar for Bar {
    fn to_bar(&self) -> Bar {
        *self
    }
}

fn foo(x: &Fat<[isize]>) -> isize {
    let y = &x.ptr;
    x.f1 + y[0] + x.ptr[x.ptr.len() - 1] + x.f2.len() as isize
}

fn foo2<T: ToBar>(x: &Fat<[T]>) -> Vec<Bar> {
    x.ptr.iter().map(|b| b.to_bar()).collect()
}

fn deep(x: &Fat<Fat<[u16]>>) -> u16 {
    x.ptr.ptr.iter().sum::<u16>() + x.ptr.f1 as u16
}

fn sum_tail(val: &Fat<[u8]>) -> u32 {
    match *val {
        Fat { f1, ref ptr, .. } => f1 as u32 + ptr.iter().map(|&b| b as u32).sum::<u32>(),
    }
}

fn main() {
    let f1 = Fat { f1: 5, f2: "some str", ptr: [1, 2, 3] };
    println!("{}", foo(&f1));
    let f2 = &f1;
    let f3: &Fat<[isize]> = f2;
    println!("{} {}", foo(f3), f3.ptr.len());
    let f4: &Fat<[isize]> = &Fat { f1: 1, f2: "", ptr: [] };
    println!("{}", f4.ptr.is_empty());

    let bars = Fat { f1: 0, f2: "bars", ptr: [Bar(1), Bar(2)] };
    println!("{:?}", foo2(&bars));

    // assignment through `&mut Fat<[T]>`
    let m: &mut Fat<[isize]> = &mut Fat { f1: 5, f2: "x", ptr: [1, 2, 3] };
    m.ptr[1] = 34;
    m.f1 += 1;
    for v in m.ptr.iter_mut() {
        *v *= 2;
    }
    println!("{} {:?}", m.f1, &m.ptr);

    // nested: the tail is a struct with an unsized tail
    let d = Fat { f1: 1, f2: "outer", ptr: Fat { f1: 8, f2: "deep", ptr: [1u16, 2, 3] } };
    println!("{}", deep(&d));
    let inner: &Fat<[u16]> = &d.ptr;
    println!("{} {}", inner.f2, inner.ptr.len());

    // patterns on the pointee
    let s: &Fat<[u8]> = &Fat { f1: 32, f2: "", ptr: [1, 2, 3] };
    println!("{}", sum_tail(s));
    let Fat { f1, ptr, .. } = s;
    println!("{} {:?}", f1, ptr);

    // boxes and raw pointers
    let b: Box<Fat<[isize; 3]>> = Box::new(Fat { f1: 5, f2: "some str", ptr: [1, 2, 3] });
    println!("{}", foo(&*b));
    let b: Box<Fat<[isize]>> = b;
    println!("{} {}", foo(&*b), b.ptr.len());
    let p: *const Fat<[u8]> = s;
    let r = unsafe { &*p };
    println!("{} {}", r.f1, r.ptr[2]);
    println!("{} {}", std::mem::size_of::<&Fat<[u8]>>(), std::mem::size_of::<Box<Fat<Fat<[u8]>>>>());
}
