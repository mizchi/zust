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
// (`*b;` moves what a Box holds out; `*b = v` then drops nothing old, and b holds v again)
fn box_reinit() {
    let mut b = Box::new(S(110));
    let c = *b;
    println!("moved {}", c.0);
    *b = S(111);
    let mut d = Box::new(S(112));
    *d = S(113);
    println!("end box_reinit");
}
// (a match arm that assigns the scrutinee: after it, the scrutinee is not known to be the variant the arm matched)
enum E2 {
    A(S, S),
    B(S, S),
}
fn arm_assigns(mut e: E2) {
    match e {
        E2::A(..) => e = E2::B(S(123), S(124)),
        E2::B(..) => return,
    }
    match e {
        E2::B(x, _) => drop(x),
        _ => {}
    }
    println!("end arm_assigns");
}
// (all of a local moved after a part of it was: no part is dropped again)
fn whole_after_part() {
    let mut x = (S(131), S(132));
    let a = x.0;
    x.0 = S(133);
    match x {
        y => drop(y),
    }
    drop(a);
    println!("end whole_after_part");
}
// (a field assigned whose parts a move split: the parts still there are dropped first)
fn nested_assign() {
    let mut x = ((S(141), S(142)), S(143));
    let a = x.0.0;
    x.0 = (S(144), S(145));
    drop(a);
    println!("end nested_assign");
}
// (a local moved, all of it and a part, in a loop it outlives)
fn loop_whole() {
    let mut x = (S(151), S(152));
    for i in 0..2 {
        let y = x;
        drop(y);
        x = (S(160 + i), S(170 + i));
        if i == 1 {
            break;
        }
        let a = x.0;
        drop(a);
        x.0 = S(180 + i);
    }
    println!("end loop_whole");
}
// (a part moved out and assigned in loops the local outlives: none of it leaks)
fn loop_parts() {
    let mut x = (S(191), S(192));
    loop {
        drop(x.0);
        for k in 0..2 {
            x.1 = S(193 + k);
        }
        break;
    }
    println!("end loop_parts");
}
struct P {
    a: S,
    b: (S, S),
    c: u8,
}
fn loop_nested_parts() {
    let mut p = P { a: S(201), b: (S(202), S(203)), c: 0 };
    let mut i = 0;
    while i < 3 {
        if i == 2 {
            p.b.1 = S(204);
            break;
        }
        let t = p.b.0;
        drop(t);
        p.b.0 = S(205 + i);
        p.a = S(208 + i);
        i += 1;
        p.c += 1;
    }
    println!("end loop_nested_parts {}", p.c);
}
// (a field whose own part was moved out and assigned again, then moved out as a whole: its parts are the new owner's)
fn take_pair(p: (S, S)) {
    println!("took {} {}", (p.0).0, (p.1).0);
}
fn reinit_then_move() {
    let mut x = ((S(211), S(212)), S(213));
    let a = x.0.0;
    drop(a);
    x.0.0 = S(214);
    let b = x.0;
    take_pair(b);
    println!("end reinit_then_move");
}
fn reinit_then_move_early(c: bool) -> u32 {
    let mut x = ((S(221), S(222)), S(223));
    let a = x.0.0;
    drop(a);
    x.0.0 = S(224);
    if c {
        let b = x.0;
        drop(b);
        return 1;
    }
    println!("end reinit_then_move_early");
    0
}
// An or-pattern whose alternatives move out of different variants: what is left of the local is that variant's
// (A's third field, B's second), dropped at the end of the scope (CodeRabbit on #224)
enum E3 {
    A(S, u64, S),
    B(S, S),
}
fn or_parts(a: bool) {
    let e = if a { E3::A(S(131), 7, S(133)) } else { E3::B(S(134), S(135)) };
    match e {
        E3::A(x, ..) | E3::B(x, ..) => drop(x),
    }
    println!("or_parts {a}");
}

fn main() {
    reinit_then_move();
    reinit_then_move_early(true);
    reinit_then_move_early(false);
    loop_parts();
    loop_nested_parts();
    arm_assigns(E2::A(S(121), S(122)));
    whole_after_part();
    nested_assign();
    loop_whole();
    box_reinit();
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
    or_parts(true);
    or_parts(false);
}
