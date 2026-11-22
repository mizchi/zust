// Struct literals, tuple constructors and patterns that write their type arguments (`S::<u16> { .. }`,
// `P::<u16>` as a function, `let P::<u16>(a, b) = ..`): the instance is the one the turbofish writes
struct P<T>(T, u8);
struct S<T> {
    t: T,
    n: u8,
}

fn main() {
    let a = S::<u16> { t: 300, n: 1 };
    let b: S<u64> = S::<_> { t: 5, n: 2 };
    let c = S::<String> { t: String::from("hi"), ..S { t: String::new(), n: 3 } };
    println!("{} {} {} {} {}", a.t, a.n, b.t, c.t, c.n);
    // (an integer literal borrowed for a turbofish's `&u32`: a u32)
    let k = 7;
    let r = P::<&u32>(&k, 0);
    println!("{}", r.0.leading_zeros());
    let f = P::<u16>;
    let g = Some::<u32>;
    let v: Vec<P<i64>> = vec![4i64, 5].into_iter().map(|x| P::<i64>(x, 9)).collect();
    println!("{} {:?} {} {}", f(2, 3).0, g(3), v[1].0, v[1].1);
    let P::<u16>(x, y) = P(300, 1);
    let s = S { t: String::from("x"), n: 2 };
    match s {
        S::<String> { t, n: 2 } => println!("{} {} {}", x, y, t),
        S::<String> { .. } => println!("other"),
    }
    let q = &P(5u8, 6);
    let &P::<u8>(m, n) = q;
    if let P::<u8>(e, _) = q {
        println!("{} {} {}", m, n, e);
    }
}
