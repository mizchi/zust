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

/// `s.fmt(f)` of a str takes the fmt trait the program imports (#256): changing `use std::fmt::{self, Debug};` to
/// Display, the fn that calls it the same, compiles it again (its cache key holds the program's fmt imports)
#[test]
fn fmt_import_invalidates() {
    let dir = std::env::temp_dir().join(format!("d2cache_fmt_{}", std::process::id()));
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
    let src = |tr: &str| {
        format!(
            "use std::fmt::{{self, {tr}}};\nfn show(s: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {{ s.fmt(f) }}\nstruct W;\nimpl fmt::Display for W {{ fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{ show(\"text\", f) }} }}\nfn main() {{ println!(\"{{}}\", W); }}\n"
        )
    };
    assert_eq!(run(&src("Debug")), "\"text\"\n");
    assert_eq!(run(&src("Display")), "text\n");
    assert_eq!(run(&src("Debug")), "\"text\"\n");
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

/// The reference graph refers to names, not to each fn of a name (#188): a change to the signature of one of a
/// name's fns (`S3::area`) compiles again every fn that names it, as before: `uses_area`, which calls `S0::area` and
/// `S1::area` only (the over-approximation by the name's last segment), main, and each `area` (its definition names
/// it). Not `other`
#[test]
fn a_names_signature_change_reaches_its_callers() {
    let program = |ret: &str| {
        let mut s = String::new();
        for i in 0..6 {
            let r = if i == 3 { ret } else { "u64" };
            s.push_str(&format!("struct S{i}(u64);\nimpl S{i} {{\n    fn area(&self) -> {r} {{ (self.0 * {}) as {r} }}\n}}\n", i + 2));
        }
        s.push_str("fn uses_area(x: u64) -> u64 { S0(x).area() + S1(x).area() }\n");
        s.push_str("fn other(x: u64) -> u64 { x.wrapping_mul(3) }\n");
        s.push_str("fn main() {\n    let v = uses_area(2) + other(5) + S3(1).area() as u64;\n    println!(\"{v}\");\n}\n");
        s
    };
    let dir = std::env::temp_dir().join(format!("d2names_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("p");
    let cp = dir.join("p.cache");
    let run = |src: &str| {
        let mut s = Session::new(Options { jobs: 1, ..Default::default() });
        s.load_cache(&cp);
        let rep = s.build(src, &out).unwrap();
        s.save_cache(&cp).unwrap();
        let o = std::process::Command::new(&out).output().unwrap();
        (rep, String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let (_, a) = run(&program("u64"));
    assert_eq!(a, "30\n");
    let (r, b) = run(&program("u64"));
    assert_eq!((r.fns_compiled, b.as_str()), (0, "30\n"), "{r:?}");
    // (the six `area`s, uses_area and main)
    let (r, c) = run(&program("u32"));
    assert_eq!((r.fns_compiled, c.as_str()), (8, "30\n"), "{r:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
