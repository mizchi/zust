// A program that uses the names of SIMD intrinsics and vector types for its own items, and no SIMD (issue #30):
// its own definitions are used, as rustc uses them (a local closure or fn pointer, an associated function, a
// struct named like a vector type)
#[allow(non_camel_case_types)]
struct __m128 {
    n: i32,
}

fn get(x: __m128) -> i32 {
    x.n
}

#[allow(non_camel_case_types)]
struct f32x4(u64, u64, u64, u64);

struct Ops;

impl Ops {
    fn _mm_add_ps(a: i32, b: i32) -> i32 {
        a * 100 + b
    }
    fn i32x4_add(a: i32, b: i32) -> i32 {
        a - b
    }
}

fn apply(_mm_add_ps: fn(i32, i32) -> i32) -> i32 {
    _mm_add_ps(1, 2)
}

fn f32x4_mine(x: f32) -> f32 {
    x * 2.0
}

fn main() {
    let i32x4_custom = |a: i32, b: i32| a + b;
    let s = i32x4_custom(1, 2);
    println!("closure {s}");
    let v128 = 7;
    println!("local {}", v128 + 1);
    println!("struct {}", get(__m128 { n: 41 }));
    println!("assoc {} {}", Ops::_mm_add_ps(3, 4), Ops::i32x4_add(9, 2));
    println!("fn pointer {}", apply(|a, b| a * 10 + b));
    println!("own fn {}", f32x4_mine(1.25));
    let q = f32x4(1, 2, 3, 4);
    println!("tuple struct {}", q.0 + q.3);
}
