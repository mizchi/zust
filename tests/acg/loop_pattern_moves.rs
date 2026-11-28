// In a loop, a part of a local moved out by a pattern (`let (a, _) = x`, `match x.c { (u, _) => .. }`,
// `let (.., last) = y`) or assigned inside a macro call's arguments (`assert!({ x.1 = v; true })`): each value is
// dropped once, and what is left at the end, as rustc's build does. And the value of a `vec![..];` / `format!(..);`
// statement is dropped at the statement's end. A `while let`'s own pattern (`while let (moved, _) = x`) splits the
// local in the loop's header, before the body; one inside another loop is in that loop's pre-scan
struct S(&'static str);
impl Drop for S {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
struct P {
    a: S,
    b: S,
    c: (S, S),
}
fn main() {
    let mut x = (S("a"), S("b"));
    loop {
        let (moved, _) = x;
        drop(moved);
        assert!({
            x.1 = S("c");
            true
        });
        vec![{
            x.1 = S("d");
        }];
        break;
    }
    println!("- {}", x.1 .0);
    let mut p = P { a: S("pa"), b: S("pb"), c: (S("c0"), S("c1")) };
    let mut i = 0;
    while i < 2 {
        match p.c {
            (u, _) => drop(u),
        }
        let P { a, .. } = p;
        drop(a);
        p.a = S("pa'");
        p.c.0 = S("c0'");
        if let (_, ref k) = p.c {
            println!("k {}", k.0);
        }
        i += 1;
    }
    println!("- {}", p.b.0);
    let mut y = (S("y0"), S("y1"), S("y2"));
    for _ in 0..2 {
        let (.., last) = y;
        println!("got {}", last.0);
        y.2 = S("y2'");
    }
    let mut z = (S("z0"), (S("z1"), S("z2")));
    loop {
        let (_, (q, _)) = z;
        drop(q);
        vec![z.0];
        break;
    }
    vec![S("v")];
    format!("{}", 1);
    while_let_patterns();
    println!("end");
}

fn while_let_patterns() {
    let mut x = (S("wa"), S("wb"));
    let mut n = 0;
    while let (moved, _) = x {
        println!("got {}", moved.0);
        n += 1;
        if n == 3 {
            break;
        }
        x = (S("wc"), S("wd"));
    }
    let mut y = (S("ya"), S("yb"));
    for i in 0..2 {
        let mut m = 0;
        while let (moved, _) = y {
            println!("got {} {}", i, moved.0);
            m += 1;
            y.0 = S("yr");
            if m == 2 {
                break;
            }
        }
        y.1 = S("yb'");
    }
    let mut p = P { a: S("qa"), b: S("qb"), c: (S("qc0"), S("qc1")) };
    let mut k = 0;
    loop {
        while let P { a, .. } = p {
            println!("got {}", a.0);
            k += 1;
            p.a = S("qa'");
            if k % 2 == 0 {
                break;
            }
        }
        if k >= 4 {
            break;
        }
        p.c.1 = S("qc1'");
    }
    println!("- {} {}", p.b.0, p.c.0 .0);
}
