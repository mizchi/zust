// What a partial move leaves of a local is dropped where the local is, field by field: after a move out of a field
// (`let x = foo.0;`) or by a pattern (`(_x, _)`, `Some((_x, _))`, `E::A(_x, _)`); a reassignment drops it first
struct S(i32);
impl Drop for S {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
struct Big {
    a: u8,
    b: S,
    c: u64,
    d: S,
    e: (S, u16, S),
}
enum E {
    A(S, S),
    B(S),
}
fn reassign() {
    let mut foo = (S(1), S(2));
    let _x = foo.0;
    foo = (S(3), S(4));
    println!("end reassign");
}
fn field_reassign() {
    let mut foo = (S(5), S(6));
    let x = foo.0;
    drop(x);
    foo.0 = S(7);
    println!("end field_reassign");
}
fn cond(c: bool) {
    let foo = (S(8), S(9));
    if c {
        let _x = foo.0;
        println!("moved");
    }
    println!("end cond {c}");
}
fn looped() {
    for i in 0..3 {
        let foo = (S(10 + i), S(20 + i));
        if i % 2 == 0 {
            let _x = foo.1;
        }
        println!("iter {i}");
    }
}
fn nested() {
    let foo = ((S(30), S(31)), S(32));
    let _x = (foo.0).1;
    println!("end nested");
}
fn order() {
    let b = Big {
        a: 1,
        b: S(40),
        c: 2,
        d: S(41),
        e: (S(42), 3, S(43)),
    };
    let _x = b.d;
    let _y = b.e.0;
    println!("end order {} {}", b.a, b.c);
}
fn early(c: bool) -> i32 {
    let foo = (S(50), S(51));
    let _x = foo.0;
    if c {
        return 1;
    }
    println!("end early");
    0
}
fn brk() {
    loop {
        let foo = (S(60), S(61));
        let _x = foo.1;
        break;
    }
    println!("end brk");
}
fn enum_pat() {
    let e = E::A(S(70), S(71));
    match e {
        E::A(_x, _) => println!("arm A"),
        E::B(_) => {}
    }
    println!("end enum");
}
fn enum_pat2() {
    let e = E::B(S(72));
    match e {
        E::A(_, _y) => println!("arm A"),
        E::B(_z) => println!("arm B"),
    }
    println!("end enum2");
}
fn struct_pat() {
    let b = Big {
        a: 1,
        b: S(80),
        c: 2,
        d: S(81),
        e: (S(82), 3, S(83)),
    };
    let Big { d, .. } = b;
    println!("bound {}", d.0);
}
fn if_let() {
    let o = Some((S(90), S(91)));
    if let Some((_, y)) = o {
        println!("got {}", y.0);
    }
    println!("end if_let");
}
fn closure_move() {
    let foo = (S(100), S(101));
    let f = move || {
        let _ = &foo.0;
    };
    f();
    println!("end closure");
}
fn t1() {
    let b = (S(1), S(2), S(3));
    let _x = b.0;
    let _y = b.2;
    println!("end t1");
}
fn t2() {
    let b = (S(4), (S(5), S(6)));
    let _x = b.0;
    let _y = b.1.0;
    println!("end t2");
}
fn t3() {
    let b = (S(7), (S(8), S(9)));
    let _y = b.1.0;
    let _x = b.0;
    println!("end t3");
}
fn main() {
    reassign();
    field_reassign();
    cond(true);
    cond(false);
    looped();
    nested();
    order();
    early(true);
    early(false);
    brk();
    enum_pat();
    enum_pat2();
    struct_pat();
    if_let();
    closure_move();
    t1();
    t2();
    t3();
}
