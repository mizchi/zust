//! Integer overflow in a debug build is a panic, not a silently wrapped value that later sizes an
//! allocation or an index. Each argument is one case; every case must panic (exit 101).
//@ expect: panic
//@ args: 0 1 2 3 4 5 6 7 8 9 10 11
fn arg() -> i64 {
    std::env::args().nth(1).unwrap().parse().unwrap()
}
fn main() {
    let k = arg();
    // (runtime values, so nothing is folded at compile time)
    let max = i32::MAX - (k as i32 & 0);
    let min = i32::MIN + (k as i32 & 0);
    let zero = k as i32 & 0;
    let m1 = -1 + zero;
    let r: i64 = match k {
        0 => (max + 1) as i64,
        1 => (min - 1) as i64,
        2 => (max * 2) as i64,
        3 => (-min) as i64,
        4 => (max / zero) as i64,
        5 => (min / m1) as i64,
        6 => (max % zero) as i64,
        7 => (min % m1) as i64,
        8 => (1i32 << (32 + zero)) as i64,
        9 => ((i8::MIN + zero as i8).abs()) as i64,
        10 => ((0u32 + zero as u32) - 1) as i64,
        _ => (usize::MAX - (zero as usize) + 1) as i64,
    };
    println!("not reached {r}");
}
