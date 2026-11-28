// A false `#[cfg]` takes away the whole thing it is on: a match arm whose pattern has braces (serde_json's
// `Compound::Number { .. } => unreachable!()`, proc-macro2's, der's), a field whose type has `,` in its generic
// arguments (anstyle-parse's `ArrayVec<u8, MAX_OSC_RAW>`), and a statement with a turbofish of several arguments
// (zerovec's `assert_serde_roundtrips::<VarTupleULE<u16, ..>>(..)`)
#[allow(unused_imports)]
use std::collections::HashMap;

#[allow(dead_code)]
enum E {
    A { x: u8 },
    B(u8),
    C,
}

struct V<T, const N: usize>([T; N]);

#[allow(dead_code)]
struct S {
    a: u8,
    #[cfg(any())]
    b: V<u8, 4>,
    #[cfg(any())]
    m: HashMap<String, Vec<(u8, u8)>>,
    #[cfg(all())]
    c: V<u16, 2>,
    #[cfg(any())]
    f: Box<dyn Fn(u8, u8) -> u8>,
    #[cfg(any())]
    k: V<u8, { 2 + 2 }>,
}

struct T(#[cfg(any())] HashMap<u8, u8>, u8);

fn size<T, U>(_: &T) -> usize {
    std::mem::size_of::<U>()
}

fn f(e: &E) -> u8 {
    match e {
        #[cfg(any())]
        E::A { x } => *x,
        #[cfg(any())]
        E::A { .. } => unreachable!(),
        #[cfg(any())]
        E::A { x, .. } | E::B(x) => *x + 9,
        #[cfg(any())]
        E::A { x } if *x > 0 => 1,
        #[cfg(any())]
        E::C => if true { 1 } else { 2 },
        #[cfg(any())]
        E::A { x } => {
            *x
        }
        E::A { x } => *x + 1,
        E::B(y) => *y,
        E::C => 7,
    }
}

fn g(#[cfg(any())] m: HashMap<u8, u8>, n: u8) -> u8 {
    n * 2
}

fn main() {
    let s = S { a: 1, c: V([3, 4]) };
    #[cfg(any())]
    let n = size::<u8, Vec<u8>>(&s.a);
    #[cfg(any())]
    size::<
        V<u16, 3>,
        HashMap<u8, Vec<u8>>,
    >(&s.a);
    #[cfg(any())]
    {
        println!("gone");
    }
    if s.a == 1 {
        println!("kept");
    }
    let t = T(5);
    let lt = 3;
    // (a `<` that closes no `<..>` is a comparison: the field ends at its `,`)
    let r = S { a: 2, #[cfg(any())] b: lt < 4, c: V([0, 0]) };
    println!("{} {} {} {}", f(&E::A { x: 4 }), f(&E::B(5)), f(&E::C), s.c.0[0] + s.c.0[1]);
    println!("{} {} {} {}", t.0, g(6), r.a, size::<u8, u16>(&s.a));
}
