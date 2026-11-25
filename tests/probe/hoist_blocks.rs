// Items written in blocks of one fn: same-named fns (and structs) in sibling blocks, which call each other, an
// item in a block that shadows one of its fn's body or a top-level one, and same-named items in two fns
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
}
