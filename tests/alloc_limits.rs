//! A Vec asked for more than fits stops with "capacity overflow" (exit 101), as with rustc, instead of computing a
//! wrapped size and writing past a small block (formal/kani-prelude: with_capacity_any_size, reserve_any_size).
#![cfg(feature = "acg")]

fn run(src: &str, name: &str) -> std::process::Output {
    let dir = std::env::temp_dir().join(format!("alloc_limits_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join(name);
    zustc::driver2::build(src, &exe, false, None).unwrap();
    std::process::Command::new(&exe).output().unwrap()
}

#[test]
fn capacity_overflow_panics() {
    let cases = [
        ("with_capacity", "fn main() { let n: usize = std::env::args().count() << 61; let mut v: Vec<u64> = Vec::with_capacity(n); v.push(1); println!(\"{}\", v.len()); }"),
        ("reserve", "fn main() { let mut v: Vec<u8> = Vec::new(); v.push(1); v.reserve(usize::MAX); v.push(2); println!(\"{}\", v.len()); }"),
        ("reserve_isize", "fn main() { let mut v: Vec<u32> = Vec::new(); v.reserve(usize::MAX / 4); println!(\"{}\", v.len()); }"),
    ];
    for (name, src) in cases {
        let o = run(src, name);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(101), "{name}: {err}");
        assert!(err.contains("capacity overflow"), "{name}: {err}");
        assert!(o.stdout.is_empty(), "{name}");
    }
}

#[test]
fn growth_still_works() {
    let o = run("fn main() { let mut v = Vec::new(); for i in 0..10000u64 { v.push(i); } v.reserve(3); let mut z: Vec<()> = Vec::new(); for _ in 0..100 { z.push(()); } println!(\"{} {} {}\", v.len(), v.iter().sum::<u64>(), z.len()); }", "growth");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "10000 49995000 100\n");
}
