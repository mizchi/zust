//! session's cache and parallelism: a second build reads everything from the cache and gives the same .o,
//! a change to one fn recompiles only that fn, and compiling in parallel with fork gives the same .o as one process.
#![cfg(feature = "acg")]
use zustc::session::{Options, Session};

fn program(k: u32) -> String {
    let mut s = String::new();
    for i in 0..120 {
        let m = if i == 7 { k } else { 31 };
        s.push_str(&format!("fn f{i}(x: u64) -> u64 {{ let mut s = x; for j in 0..{} {{ s = s.wrapping_mul({m}).wrapping_add(j * {i}); }} s % 1000 }}\n", i % 5 + 2));
    }
    s.push_str("fn main() { let mut t = 1u64; let v: Vec<u64> = (0..10).collect();\n");
    for i in 0..120 {
        s.push_str(&format!("    t = t.wrapping_add(f{i}(t + v[{}]));\n", i % 10));
    }
    s.push_str("    println!(\"{t}\");\n}\n");
    s
}

#[test]
fn cache_and_parallel() {
    let dir = std::env::temp_dir().join(format!("d2cache_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("p");
    let cp = dir.join("p.cache");
    let run = |src: &str, jobs: usize, cache: bool| {
        let mut s = Session::new(Options { jobs, no_cache: !cache, ..Default::default() });
        if cache {
            s.load_cache(&cp);
        }
        let rep = s.build(src, &out).unwrap();
        if cache {
            s.save_cache(&cp).unwrap();
        }
        let obj = std::fs::read(out.with_extension("o")).unwrap();
        let o = std::process::Command::new(&out).output().unwrap();
        (rep, obj, o.stdout)
    };
    let src = program(31);
    let (r1, o1, s1) = run(&src, 1, true);
    assert!(r1.fns_compiled > 120 && r1.fns_hit == 0, "{r1:?}");
    let (r2, o2, s2) = run(&src, 1, true);
    assert_eq!(r2.fns_compiled, 0, "{r2:?}");
    assert_eq!(r2.items_parsed, 0, "{r2:?}");
    assert!(o1 == o2 && s1 == s2);
    // Change just one fn
    let src2 = program(37);
    let (r3, o3, s3) = run(&src2, 1, true);
    assert_eq!(r3.fns_compiled, 1, "{r3:?}");
    // The same .o and output in parallel (fork) and in one process without a cache
    let (_, o4, s4) = run(&src2, 4, false);
    let (_, o5, s5) = run(&src2, 1, false);
    assert!(o3 == o4 && o4 == o5, "objects differ");
    assert!(s3 == s4 && s4 == s5);
}

/// A constant's cast through a type alias: changing the alias changes the constant, and the function that uses
/// it is compiled again (its body and the constant's text are the same)
#[test]
fn alias_in_a_constant_invalidates() {
    let dir = std::env::temp_dir().join(format!("d2cache_alias_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("p");
    let cp = dir.join("p.cache");
    let run = |src: &str| {
        let mut s = Session::new(Options { jobs: 1, ..Default::default() });
        s.load_cache(&cp);
        s.build(src, &out).unwrap();
        s.save_cache(&cp).unwrap();
        String::from_utf8(std::process::Command::new(&out).output().unwrap().stdout).unwrap()
    };
    let src = |lane: &str| {
        format!(
            "type Lane = {lane};\nconst INDEX: usize = (((257u32 as Lane) as u32) / 256) as usize;\nfn pick() -> u32 {{ let t = [10u32, 11, 12]; t[INDEX] }}\nfn main() {{ println!(\"{{}}\", pick()); }}\n"
        )
    };
    assert_eq!(run(&src("u8")), "10\n");
    assert_eq!(run(&src("u16")), "11\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An associated type's value changed, the function that writes the projection the same: it is compiled and checked
/// again (`type Out = u32;` to `u64` changes its size; `&'a u32` to `&'static u32` makes its `let` an error)
#[test]
fn associated_type_invalidates() {
    let dir = std::env::temp_dir().join(format!("d2cache_assoc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("p");
    let cp = dir.join("p.cache");
    let run = |src: &str| {
        let mut s = Session::new(Options { jobs: 1, ..Default::default() });
        s.load_cache(&cp);
        let r = s.build(src, &out);
        s.save_cache(&cp).unwrap();
        r.map(|_| String::from_utf8(std::process::Command::new(&out).output().unwrap().stdout).unwrap())
    };
    let size = |t: &str| {
        format!(
            "trait Foo {{ type Out; }}\nimpl Foo for () {{ type Out = {t}; }}\nfn f() -> usize {{ let x: <() as Foo>::Out = 3; std::mem::size_of_val(&x) }}\nfn main() {{ println!(\"{{}}\", f()); }}\n"
        )
    };
    assert_eq!(run(&size("u32")).unwrap(), "4\n");
    assert_eq!(run(&size("u64")).unwrap(), "8\n");
    let lt = |t: &str| {
        format!(
            "trait Foo {{ type Out; }}\nimpl<'a> Foo for &'a () {{ type Out = {t}; }}\nfn f<'b>(r: &'b u32) -> u32 {{ let x: <&'b () as Foo>::Out = r; *x }}\nfn main() {{ let v = 3; println!(\"{{}}\", f(&v)); }}\n"
        )
    };
    assert_eq!(run(&lt("&'a u32")).unwrap(), "3\n");
    let e = run(&lt("&'static u32")).unwrap_err().to_string();
    assert!(e.contains("lifetime may not live long enough"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
