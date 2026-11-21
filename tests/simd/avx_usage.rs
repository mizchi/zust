// 256-bit vectors as values (issue #30): through parameters and results (many of them), locals that live across
// branches and loops, struct fields, arrays, Vec, Option, generics, closures, function pointers, and the ML kernels
// of experiments/zustscript at 8 lanes with FMA
use std::arch::x86_64::*;

fn h(name: &str, v: __m256i) {
    let mut o = [0u32; 8];
    unsafe { _mm256_storeu_si256(o.as_mut_ptr() as *mut __m256i, v) };
    let s: Vec<String> = o.iter().map(|x| format!("{x:08x}")).collect();
    println!("{name}: {}", s.join(" "));
}

#[derive(Clone, Copy)]
struct Body {
    pos: __m256,
    vel: __m256,
    tag: u8,
}

enum Shape {
    Empty,
    One(__m256),
    Two(__m256, __m256),
}

#[target_feature(enable = "avx2,fma")]
fn hsum(v: __m256) -> f32 {
    let s = _mm_add_ps(_mm256_castps256_ps128(v), _mm256_extractf128_ps::<1>(v));
    let mut o = [0f32; 4];
    unsafe { _mm_storeu_ps(o.as_mut_ptr(), s) };
    (o[0] + o[1]) + (o[2] + o[3])
}

#[target_feature(enable = "avx2,fma")]
fn area(s: &Shape) -> f32 {
    match s {
        Shape::Empty => 0.0,
        Shape::One(v) => hsum(*v),
        Shape::Two(a, b) => hsum(_mm256_mul_ps(*a, *b)),
    }
}

#[target_feature(enable = "avx2,fma")]
fn step(b: &mut Body, dt: f32) {
    b.pos = _mm256_fmadd_ps(b.vel, _mm256_set1_ps(dt), b.pos);
}

#[target_feature(enable = "avx2,fma")]
fn pick(c: bool, a: __m256i, b: __m256i) -> __m256i {
    if c { a } else { b }
}

#[target_feature(enable = "avx2,fma")]
fn many(a: __m256i, k: i32, b: __m256i, d: f64, c: __m256i, e: __m128i, f: __m256i) -> (__m256i, __m128i, f64) {
    let s = _mm256_add_epi32(_mm256_add_epi32(a, b), _mm256_add_epi32(c, f));
    (_mm256_add_epi32(s, _mm256_set1_epi32(k)), _mm_add_epi32(e, e), d * 2.0)
}

fn twice<T: Copy>(x: T, f: impl Fn(T, T) -> T) -> T {
    f(x, x)
}

fn apply(f: fn(__m256i, __m256i) -> __m256i, a: __m256i, b: __m256i) -> __m256i {
    f(a, b)
}

#[target_feature(enable = "avx2,fma")]
fn dense(w: &[f32], x: &[f32], b: &[f32], m: usize, n: usize) -> Vec<f32> {
    let mut y = vec![0.0f32; m];
    for r in 0..m {
        let mut acc = _mm256_setzero_ps();
        let mut i = 0;
        while i + 8 <= n {
            unsafe {
                let wv = _mm256_loadu_ps(w.as_ptr().add(r * n + i));
                let xv = _mm256_loadu_ps(x.as_ptr().add(i));
                acc = _mm256_fmadd_ps(wv, xv, acc);
            }
            i += 8;
        }
        y[r] = b[r] + hsum(acc);
    }
    y
}

#[target_feature(enable = "avx2,fma")]
fn relu(xs: &mut [f32]) {
    let z = _mm256_setzero_ps();
    let n = xs.len() / 8 * 8;
    let p = xs.as_mut_ptr();
    let mut i = 0;
    while i < n {
        unsafe { _mm256_storeu_ps(p.add(i), _mm256_max_ps(_mm256_loadu_ps(p.add(i)), z)) };
        i += 8;
    }
    for x in &mut xs[n..] {
        *x = x.max(0.0);
    }
}

#[target_feature(enable = "avx2,fma")]
fn saxpy(a: f32, xs: &[f32], ys: &mut [f32]) {
    let av = _mm256_set1_ps(a);
    let n = xs.len() / 8 * 8;
    let mut i = 0;
    while i < n {
        unsafe {
            let x = _mm256_loadu_ps(xs.as_ptr().add(i));
            let y = _mm256_loadu_ps(ys.as_ptr().add(i));
            _mm256_storeu_ps(ys.as_mut_ptr().add(i), _mm256_fmadd_ps(av, x, y));
        }
        i += 8;
    }
    for k in n..xs.len() {
        ys[k] = a.mul_add(xs[k], ys[k]);
    }
}

#[target_feature(enable = "avx2,fma")]
fn sumsq(xs: &[f32]) -> f32 {
    let n = xs.len() / 8 * 8;
    let mut acc = _mm256_setzero_ps();
    let mut i = 0;
    while i < n {
        let v = unsafe { _mm256_loadu_ps(xs.as_ptr().add(i)) };
        acc = _mm256_fmadd_ps(v, v, acc);
        i += 8;
    }
    let mut s = hsum(acc);
    for x in &xs[n..] {
        s += x * x;
    }
    s
}

fn lcg(s: &mut u32) -> f32 {
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    ((*s >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
}

#[target_feature(enable = "avx2,fma")]
fn run() {
    println!("size {} align {}", std::mem::size_of::<__m256>(), std::mem::align_of::<__m256>());
    println!("size Body {}", std::mem::size_of::<Body>());
    let mut bd = Body { pos: _mm256_setr_ps(0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0), vel: _mm256_set1_ps(0.5), tag: 3 };
    for _ in 0..4 {
        step(&mut bd, 0.25);
    }
    let copy = bd;
    h("body", _mm256_castps_si256(copy.pos));
    println!("tag {}", copy.tag);
    let bodies = vec![bd, copy, Body { pos: _mm256_set1_ps(9.0), vel: _mm256_set1_ps(-1.0), tag: 1 }];
    let total = bodies.iter().fold(_mm256_setzero_ps(), |acc, b| _mm256_add_ps(acc, b.pos));
    h("total", _mm256_castps_si256(total));
    let shapes = [Shape::Empty, Shape::One(_mm256_set1_ps(2.5)), Shape::Two(_mm256_set1_ps(1.5), _mm256_set1_ps(3.0))];
    let areas: Vec<f32> = shapes.iter().map(|s| area(s)).collect();
    println!("areas {:?}", areas);

    for c in [true, false] {
        h("pick", pick(c, _mm256_set1_epi32(1), _mm256_set1_epi32(2)));
    }
    let mut acc = _mm256_setzero_si256();
    let mut k = 0;
    while k < 10 {
        acc = if k % 3 == 0 { _mm256_add_epi32(acc, _mm256_set1_epi32(k)) } else { _mm256_sub_epi32(acc, _mm256_set1_epi32(1)) };
        k += 1;
    }
    h("loop", acc);
    let m = match k {
        10 => _mm256_set1_epi64x(10),
        _ => _mm256_setzero_si256(),
    };
    h("match", m);

    let mut arr = [_mm256_setzero_si256(); 6];
    for (i, a) in arr.iter_mut().enumerate() {
        *a = _mm256_set1_epi32(i as i32);
    }
    h("arr", arr.iter().fold(_mm256_setzero_si256(), |a, &b| _mm256_add_epi32(a, b)));
    let mut v: Vec<__m256i> = Vec::new();
    for i in 0..50 {
        v.push(_mm256_setr_epi32(i, -i, i * 2, 1, 2, 3, i * i, 7));
    }
    h("vec", v.iter().fold(_mm256_setzero_si256(), |a, &b| _mm256_add_epi32(a, b)));
    if let Some(x) = v.get(7).copied() {
        h("opt", x);
    }
    println!("none {}", v.get(99).is_none());
    h("twice", twice(_mm256_set1_epi32(21), |a, b| unsafe { _mm256_add_epi32(a, b) }));
    let base = _mm256_setr_epi32(1, 2, 3, 4, 5, 6, 7, 8);
    let add_base = |x: __m256i| unsafe { _mm256_add_epi32(x, base) };
    h("closure", add_base(_mm256_set1_epi32(10)));
    h("fnptr", apply(|a, b| unsafe { _mm256_mullo_epi32(a, b) }, _mm256_set1_epi32(3), base));
    let (r, r128, dd) = many(base, 1, base, 1.5, base, _mm_set1_epi32(4), base);
    h("many", r);
    let mut o = [0i32; 4];
    unsafe { _mm_storeu_si128(o.as_mut_ptr() as *mut __m128i, r128) };
    println!("many.1 {:?} many.2 {dd}", o);
    let t: [f32; 8] = unsafe { std::mem::transmute(_mm256_set1_ps(0.5)) };
    println!("transmute {:?}", t);

    let mut seed = 12345u32;
    let (mm, nn) = (64, 128);
    let w: Vec<f32> = (0..mm * nn).map(|_| lcg(&mut seed)).collect();
    let x: Vec<f32> = (0..nn).map(|_| lcg(&mut seed)).collect();
    let b: Vec<f32> = (0..mm).map(|_| lcg(&mut seed)).collect();
    let mut y = dense(&w, &x, &b, mm, nn);
    println!("dense: {:08x} {:08x} {:08x}", y[0].to_bits(), y[31].to_bits(), y[63].to_bits());
    relu(&mut y);
    println!("relu: {} {:08x}", y.iter().filter(|&&v| v > 0.0).count(), y.iter().fold(0.0f32, |a, &v| a + v).to_bits());
    let mut ys: Vec<f32> = (0..1003).map(|_| lcg(&mut seed)).collect();
    let xs: Vec<f32> = (0..1003).map(|_| lcg(&mut seed)).collect();
    saxpy(0.75, &xs, &mut ys);
    println!("saxpy: {:08x} {:08x} {:08x}", ys[0].to_bits(), ys[500].to_bits(), ys[1002].to_bits());
    println!("sumsq: {:08x}", sumsq(&ys).to_bits());
}

fn main() {
    unsafe { run() }
}
