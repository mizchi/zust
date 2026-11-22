// v128 as a value (issue #30): in locals, branches, loops, structs, arrays, Vec, Option, generics, closures,
// function pointers, transmute, and many parameters
use std::arch::wasm32::*;

fn h(name: &str, v: v128) {
    println!(
        "{name}: {:08x} {:08x} {:08x} {:08x}",
        u32x4_extract_lane::<0>(v),
        u32x4_extract_lane::<1>(v),
        u32x4_extract_lane::<2>(v),
        u32x4_extract_lane::<3>(v)
    );
}

#[derive(Clone, Copy)]
struct Particle {
    pos: v128,
    vel: v128,
    id: u8,
}

struct Pair(v128, u64, v128);

enum Shape {
    Empty,
    One(v128),
    Two(v128, v128),
}

fn area(s: &Shape) -> f32 {
    match s {
        Shape::Empty => 0.0,
        Shape::One(v) => f32x4_extract_lane::<0>(*v),
        Shape::Two(a, b) => f32x4_extract_lane::<1>(f32x4_mul(*a, *b)),
    }
}

fn step(p: &mut Particle, dt: f32) {
    p.pos = f32x4_add(p.pos, f32x4_mul(p.vel, f32x4_splat(dt)));
}

fn pick(c: bool, a: v128, b: v128) -> v128 {
    if c { a } else { b }
}

fn twice<T: Copy>(x: T, f: impl Fn(T, T) -> T) -> T {
    f(x, x)
}

fn first<T: Copy>(xs: &[T]) -> Option<T> {
    xs.first().copied()
}

#[allow(clippy::too_many_arguments)]
fn many(a: v128, b: i32, c: v128, d: f64, e: v128, f: u8, g: v128, h_: i64) -> (v128, f64) {
    let s = i32x4_add(i32x4_add(a, c), i32x4_add(e, g));
    (i32x4_add(s, i32x4_splat(b + f as i32 + h_ as i32)), d * 2.0)
}

fn apply(f: fn(v128, v128) -> v128, a: v128, b: v128) -> v128 {
    f(a, b)
}

fn sum_lanes(v: v128) -> i32 {
    i32x4_extract_lane::<0>(v) + i32x4_extract_lane::<1>(v) + i32x4_extract_lane::<2>(v) + i32x4_extract_lane::<3>(v)
}

fn main() {
    println!("size {} align {}", std::mem::size_of::<v128>(), std::mem::align_of::<v128>());
    println!("size Particle {} Pair {}", std::mem::size_of::<Particle>(), std::mem::size_of::<Pair>());

    let mut p = Particle { pos: f32x4(0.0, 1.0, 2.0, 3.0), vel: f32x4_splat(0.5), id: 7 };
    for _ in 0..4 {
        step(&mut p, 0.25);
    }
    let q = p;
    h("particle", q.pos);
    println!("id {}", q.id);
    let ps = vec![p, q, Particle { pos: f32x4_splat(9.0), vel: f32x4_splat(-1.0), id: 1 }];
    let total = ps.iter().fold(f32x4_splat(0.0), |acc, p| f32x4_add(acc, p.pos));
    h("total", total);

    let pr = Pair(i32x4(1, 2, 3, 4), 99, i32x4_splat(-1));
    h("pair", i32x4_add(pr.0, pr.2));
    println!("pair.1 {}", pr.1);

    let shapes = [Shape::Empty, Shape::One(f32x4_splat(2.5)), Shape::Two(f32x4(1.0, 2.0, 3.0, 4.0), f32x4_splat(3.0))];
    let areas: Vec<f32> = shapes.iter().map(area).collect();
    println!("areas {:?}", areas);

    // branches and loops
    for c in [true, false] {
        h("pick", pick(c, i32x4_splat(1), i32x4_splat(2)));
    }
    let mut acc = i32x4_splat(0);
    let mut k = 0;
    while k < 10 {
        acc = if k % 3 == 0 { i32x4_add(acc, i32x4(k, k, k, k)) } else { i32x4_sub(acc, i32x4_splat(1)) };
        k += 1;
    }
    h("loop", acc);
    let m = match k {
        10 => i32x4_splat(10),
        _ => i32x4_splat(0),
    };
    h("match", m);

    // arrays, Vec, Option
    let mut arr = [i32x4_splat(0); 8];
    for (i, a) in arr.iter_mut().enumerate() {
        *a = i32x4_splat(i as i32);
    }
    h("arr", arr.iter().fold(i32x4_splat(0), |a, &b| i32x4_add(a, b)));
    let mut v: Vec<v128> = Vec::new();
    for i in 0..100 {
        v.push(i32x4(i, -i, i * 2, 1));
    }
    let s = v.iter().fold(i32x4_splat(0), |a, &b| i32x4_add(a, b));
    h("vec", s);
    println!("sum {}", sum_lanes(s));
    let o: Option<v128> = v.get(5).copied();
    if let Some(x) = o {
        h("opt", x);
    }
    println!("none {}", v.get(500).is_none());
    h("first", first(&v).unwrap());

    // generics, closures, function pointers
    h("twice", twice(i32x4_splat(21), |a, b| i32x4_add(a, b)));
    let base = i32x4(1, 2, 3, 4);
    let add_base = |x: v128| i32x4_add(x, base);
    h("closure", add_base(i32x4_splat(10)));
    h("fnptr", apply(|a, b| i32x4_mul(a, b), i32x4_splat(3), base));
    let fs: [fn(v128, v128) -> v128; 3] = [|a, b| i32x4_add(a, b), |a, b| i32x4_sub(a, b), |a, b| v128_xor(a, b)];
    for (i, f) in fs.iter().enumerate() {
        h(&format!("fns {i}"), f(base, i32x4_splat(1)));
    }
    let (r, d) = many(base, 1, base, 1.5, base, 2, base, 3);
    h("many", r);
    println!("many.1 {d}");

    // transmute to and from arrays
    let fa: [f32; 4] = unsafe { std::mem::transmute(f32x4(1.0, 2.0, 3.0, 4.0)) };
    println!("transmute {:?}", fa);
    let back: v128 = unsafe { std::mem::transmute([1u32, 2, 3, 0xffff_ffff]) };
    h("transmute back", back);
    let bytes: [u8; 16] = unsafe { std::mem::transmute(i16x8(1, 2, 3, 4, 5, 6, 7, -1)) };
    println!("bytes {:?}", bytes);
}
