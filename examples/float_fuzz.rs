// For comparing float formatting: prints 3000 pseudo-random f64 / f32 with {} / {:?} / {:.3} / {:.0} (must be identical to rustc, not a single char different)
fn main() {
    let mut s: u64 = 12345;
    let mut out = String::new();
    for i in 0..3000 {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let a = (s >> 11) as f64;
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let b = ((s >> 40) + 1) as f64;
        let k = (i % 40) as i32 - 20;
        let x = a / b * 10f64.powi(k);
        let y = if i % 3 == 0 { -x } else { x };
        out.push_str(&format!("{} {:?} {:.3} {:.0}\n", y, y, y, y));
        let f = y as f32;
        out.push_str(&format!("{} {:?}\n", f, f));
    }
    print!("{}", out);
}
