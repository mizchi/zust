// In a loop, a part of a local moved out by a pattern (`let (a, _) = x`, `match x.c { (u, _) => .. }`,
// `let (.., last) = y`) or assigned inside a macro call's arguments (`assert!({ x.1 = v; true })`): each value is
// dropped once, and what is left at the end, as rustc's build does. And the value of a `vec![..];` / `format!(..);`
// statement is dropped at the statement's end
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
    println!("end");
}
