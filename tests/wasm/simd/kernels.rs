// The array kernels of experiments/zustscript (dense layer, relu, saxpy, sum of squares), written with
// core::arch::wasm32 (issue #30). Floats are printed as bits: the lanes are summed in the same order as rustc's
use std::arch::wasm32::*;

fn hsum(v: v128) -> f32 {
    f32x4_extract_lane::<0>(v) + f32x4_extract_lane::<1>(v) + f32x4_extract_lane::<2>(v) + f32x4_extract_lane::<3>(v)
}

/// y = W x + b, W is m×n in rows (n a multiple of 4)
fn dense(w: &[f32], x: &[f32], b: &[f32], m: usize, n: usize, y: &mut [f32]) {
    for r in 0..m {
        let row = &w[r * n..r * n + n];
        let mut acc = f32x4_splat(0.0);
        let mut i = 0;
        while i < n {
            unsafe {
                let wv = v128_load(row.as_ptr().add(i) as *const v128);
                let xv = v128_load(x.as_ptr().add(i) as *const v128);
                acc = f32x4_add(acc, f32x4_mul(wv, xv));
            }
            i += 4;
        }
        y[r] = b[r] + hsum(acc);
    }
}

fn relu(xs: &mut [f32]) {
    let n = xs.len() / 4 * 4;
    let z = f32x4_splat(0.0);
    let p = xs.as_mut_ptr();
    let mut i = 0;
    while i < n {
        unsafe {
            let v = v128_load(p.add(i) as *const v128);
            v128_store(p.add(i) as *mut v128, f32x4_max(v, z));
        }
        i += 4;
    }
    for x in &mut xs[n..] {
        *x = x.max(0.0);
    }
}

fn saxpy(a: f32, xs: &[f32], ys: &mut [f32]) {
    let n = xs.len() / 4 * 4;
    let av = f32x4_splat(a);
    let mut i = 0;
    while i < n {
        unsafe {
            let x = v128_load(xs.as_ptr().add(i) as *const v128);
            let y = v128_load(ys.as_ptr().add(i) as *const v128);
            v128_store(ys.as_mut_ptr().add(i) as *mut v128, f32x4_add(f32x4_mul(av, x), y));
        }
        i += 4;
    }
    for k in n..xs.len() {
        ys[k] = a * xs[k] + ys[k];
    }
}

fn sumsq(xs: &[f32]) -> f32 {
    let n = xs.len() / 4 * 4;
    let mut acc = f32x4_splat(0.0);
    let mut i = 0;
    while i < n {
        let v = unsafe { v128_load(xs.as_ptr().add(i) as *const v128) };
        acc = f32x4_add(acc, f32x4_mul(v, v));
        i += 4;
    }
    let mut s = hsum(acc);
    for x in &xs[n..] {
        s += x * x;
    }
    s
}

/// an i32 histogram-like kernel: counts of lanes above a threshold, with masks
fn count_above(xs: &[i32], t: i32) -> i32 {
    let n = xs.len() / 4 * 4;
    let tv = i32x4_splat(t);
    let mut c = i32x4_splat(0);
    let mut i = 0;
    while i < n {
        let v = unsafe { v128_load(xs.as_ptr().add(i) as *const v128) };
        c = i32x4_sub(c, i32x4_gt(v, tv));
        i += 4;
    }
    let mut s = i32x4_extract_lane::<0>(c) + i32x4_extract_lane::<1>(c) + i32x4_extract_lane::<2>(c) + i32x4_extract_lane::<3>(c);
    for &x in &xs[n..] {
        s += (x > t) as i32;
    }
    s
}

fn lcg(s: &mut u32) -> f32 {
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    ((*s >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
}

fn main() {
    let mut seed = 12345u32;
    let (m, n) = (64, 128);
    let w: Vec<f32> = (0..m * n).map(|_| lcg(&mut seed)).collect();
    let x: Vec<f32> = (0..n).map(|_| lcg(&mut seed)).collect();
    let b: Vec<f32> = (0..m).map(|_| lcg(&mut seed)).collect();
    let mut y = vec![0.0f32; m];
    dense(&w, &x, &b, m, n, &mut y);
    println!("dense: {:08x} {:08x} {:08x}", y[0].to_bits(), y[31].to_bits(), y[63].to_bits());
    relu(&mut y);
    let pos = y.iter().filter(|&&v| v > 0.0).count();
    println!("relu: {pos} {:08x}", y.iter().fold(0.0f32, |a, &v| a + v).to_bits());
    let mut ys: Vec<f32> = (0..1003).map(|_| lcg(&mut seed)).collect();
    let xs: Vec<f32> = (0..1003).map(|_| lcg(&mut seed)).collect();
    saxpy(0.75, &xs, &mut ys);
    println!("saxpy: {:08x} {:08x} {:08x}", ys[0].to_bits(), ys[500].to_bits(), ys[1002].to_bits());
    println!("sumsq: {:08x}", sumsq(&ys).to_bits());
    let is: Vec<i32> = (0..1001).map(|i| (i * 7919) % 1000 - 500).collect();
    println!("count_above: {} {}", count_above(&is, 0), count_above(&is, 400));
}
