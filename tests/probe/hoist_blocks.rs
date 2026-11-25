// Items written in blocks of one fn: same-named fns (and structs) in sibling blocks, which call each other, an
// item in a block that shadows one of its fn's body or a top-level one, same-named items in two fns, and a fn beside
// a braced struct of its name in a block (a value and a type: each use takes the one of its namespace)
#![allow(non_camel_case_types, dead_code)]
fn f() -> u8 {
    9
}

fn a() -> u8 {
    fn h() -> u8 {
        1
    }
    h()
}

fn b() -> u8 {
    fn h() -> u8 {
        2
    }
    h()
}

fn main() {
    {
        fn g() -> u8 {
            1
        }
        fn k() -> u8 {
            g() + 10
        }
        println!("{}", k());
    }
    {
        fn g() -> u8 {
            2
        }
        fn k() -> u8 {
            g() + 20
        }
        println!("{}", k());
    }
    {
        fn f() -> u8 {
            3
        }
        println!("{}", f());
    }
    println!("{}", f());
    fn m() -> u8 {
        4
    }
    {
        fn m() -> u8 {
            5
        }
        println!("{}", m());
    }
    println!("{}", m());
    {
        struct P(u8);
        println!("{}", P(6).0);
    }
    {
        struct P(u16, u16);
        println!("{}", P(7, 8).1);
    }
    println!("{} {}", a(), b());
    fn n() -> u8 {
        10
    }
    {
        struct n {}
        let _x: n = n {};
        println!("{}", n());
    }
    struct t {
        v: u8,
    }
    {
        fn t() -> u8 {
            11
        }
        fn h(x: t, y: u8) -> u8 {
            x.v + y
        }
        let a = t { v: 12 };
        let b: t = t { v: 1 };
        let mut c: t;
        c = t { v: 2 };
        c.v += 1;
        let read = |x: t| x.v;
        let add = |x: t, y: u8| x.v + y;
        println!("{} {} {} {} {}", t(), a.v, b.v, c.v, h(t { v: 4 }, 5));
        println!("{} {}", read(t { v: 21 }), add(t { v: 1 }, 22));
    }
    // (a type's path and return type, `u::mk()` / `-> u`, beside a fn of its name in a block)
    struct u {
        v: u8,
    }
    impl u {
        fn mk() -> u {
            u { v: 13 }
        }
    }
    {
        fn u() -> u8 {
            14
        }
        fn mk2() -> u {
            u { v: 17 }
        }
        println!("{} {} {}", u::mk().v, u(), mk2().v);
    }
    // (a braced struct with a `where` clause whose bound has parentheses)
    fn q() -> u8 {
        15
    }
    {
        struct q<F>
        where
            F: Fn(u8) -> u8,
        {
            f: F,
        }
        println!("{}", q());
    }
    // (and one whose generics' bound has an arrow)
    fn r() -> u8 {
        16
    }
    {
        struct r<F: Fn() -> ()> {
            f: F,
        }
        println!("{}", r());
    }
    // (a turbofish: `p::<u8>()` calls the fn, `p::<u8>::mk(..)` names the struct)
    fn p<T>() -> u8 {
        18
    }
    struct p<T> {
        v: T,
    }
    impl<T> p<T> {
        fn mk(v: T) -> p<T> {
            p { v }
        }
    }
    {
        fn p() -> u8 {
            19
        }
        println!("{}", p());
    }
    println!("{} {}", p::<u8>(), p::<u8>::mk(20).v);
}
