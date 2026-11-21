//! Region checking against signatures (crates/typeck/src/acg/regionck.rs): "lifetime may not live long enough" where
//! the constraints that are known require it, and nothing on code rustc accepts
use zustc::{Options2, Session2};

fn check(src: &str) -> Vec<(usize, String)> {
    let mut s = Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() });
    s.check(src).into_iter().map(|d| (d.line, d.msg)).collect()
}

fn one_error(src: &str, line: usize) {
    let d = check(src);
    assert!(d.len() == 1 && d[0].0 == line && d[0].1.contains("lifetime may not live long enough"), "{src}\n{d:?}");
}

#[test]
fn errors_against_signatures() {
    // (a value of 'b returned as 'a)
    one_error("fn f<'a, 'b>(x: &'a u32, y: &'b u32) -> &'a u32 {\n    if *x > 0 { x } else { y }\n}\nfn main() {}\n", 2);
    // (through `Vec<T>::push`: T is invariant under `&mut`)
    one_error("fn foo(x: &mut Vec<&u8>, y: &u8) {\n    x.push(y);\n}\nfn main() {}\n", 2);
    // (into a field of a struct with a lifetime)
    one_error("struct S<'a> { r: &'a u32 }\nfn g<'a, 'b>(s: &mut S<'a>, y: &'b u32) {\n    s.r = y;\n}\nfn main() {}\n", 3);
    // ('static required)
    one_error("fn f<'a>(x: &'a str) -> &'static str {\n    x\n}\nfn main() {}\n", 2);
    // (a method storing a borrow of its own lifetime parameter in self)
    one_error("struct P<'a> { src: &'a str }\nimpl<'a> P<'a> {\n    fn set<'b>(&mut self, s: &'b str) {\n        self.src = s;\n    }\n}\nfn main() {}\n", 4);
}

#[test]
fn accepted() {
    let src = r#"
struct P<'a> { src: &'a str, pos: usize }
impl<'a> P<'a> {
    fn new(src: &'a str) -> P<'a> { P { src, pos: 0 } }
    fn rest(&self) -> &'a str { let s = self.src; &s[self.pos..] }
    fn peek(&self) -> &str { self.src }
    fn take(&mut self) -> &'a str { let r = self.rest(); self.pos += 1; r }
}
fn f1<'a, 'b: 'a>(x: &'a u32, y: &'b u32) -> &'a u32 { if *x > 0 { x } else { y } }
fn f2<'a, 'b>(x: &'a &'b u32) -> &'a u32 { *x }
fn f3<'a>(v: &'a Vec<&'a str>) -> &'a str { v[0] }
fn f4<'a>(x: &'a str, y: &'a str) -> &'a str { if x.len() > y.len() { x } else { y } }
fn f5<'a>(_x: &'a str) -> &'static str { "lit" }
fn f6(x: &mut Vec<u8>, y: &u8) { x.push(*y); }
fn f7<'a>(v: &mut Vec<&'a str>, s: &'a str) { v.push(s); let t: &'a str = s; v.push(t); }
fn f8<'a, 'b>(x: &'a str, _y: &'b str) -> &'a str { let z = x; z }
fn f9<'a>(m: &'a std::collections::HashMap<u32, String>) -> Option<&'a String> { m.get(&1) }
fn f10<'a>(s: &'a String) -> &'a str { s.as_str() }
fn f11<'a>(xs: &'a [u32]) -> &'a u32 { &xs[0] }
fn f12<'a, 'b>(p: &'b P<'a>) -> &'a str { p.src }
fn f13<'a>(v: Vec<&'static str>) -> Vec<&'a str> { v }
fn f14<'a, 'b>(x: &'a mut &'b str, y: &'b str) { *x = y; }
fn main() {
    let s = String::from("abc");
    let mut p = P::new(&s);
    println!("{} {}", p.rest(), p.peek());
    println!("{}", p.take());
    let (a, b) = (1u32, 2u32);
    println!("{} {}", f1(&a, &b), f2(&&a));
    let v = vec!["x"];
    println!("{} {} {}", f3(&v), f4("a", "bb"), f5("q"));
    let mut w = vec![];
    f6(&mut w, &3);
    let mut z = vec![];
    f7(&mut z, "k");
    println!("{} {:?} {:?} {}", f8("u", "v"), w, z, f10(&s));
    let m = std::collections::HashMap::new();
    println!("{:?} {} {} {:?}", f9(&m), f11(&[4, 5]), f12(&p), f13(vec!["y"]));
    let mut r = "a";
    f14(&mut r, "b");
}
"#;
    assert_eq!(check(src), vec![]);
}

/// A callee's signature changed only in its lifetimes: its callers are checked again (the regions are part of the
/// interface's fingerprint)
#[test]
fn lifetime_only_change_rechecks_callers() {
    let v1 = "fn pick<'a>(x: &'a u32, _y: &u32) -> &'a u32 { x }\nfn g<'a, 'b>(x: &'a u32, y: &'b u32) -> &'a u32 {\n    pick(x, y)\n}\nfn main() {}\n";
    let v2 = v1.replace("fn pick<'a>(x: &'a u32, _y: &u32)", "fn pick<'a>(_y: &u32, x: &'a u32)");
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    assert!(s.check(v1).is_empty());
    let d = s.check(&v2);
    assert!(d.len() == 1 && d[0].line == 3 && d[0].msg.contains("lifetime may not live long enough"), "{d:?}");
    assert!(s.check(v1).is_empty());
}

/// A temporary borrowed by what a `let` keeps, dropped at the end of the statement (E0716), and the temporaries that
/// live long enough: extended by the `let`, or not kept
#[test]
fn temporaries() {
    let bad = "struct Defer<'a> { x: &'a [&'a str] }\nfn defer<'r>(x: &'r [&'r str]) -> Defer<'r> { Defer { x } }\nfn main() {\n    let x = defer(&vec![\"a\", \"b\"]);\n    println!(\"{}\", x.x[0]);\n}\n";
    let d = check(bad);
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0716"), "{d:?}");
    let ok = r#"
fn first(v: &Vec<String>) -> &String { &v[0] }
fn len(s: &String) -> usize { s.len() }
struct S<'a> { r: &'a String }
struct K<'a>(&'a ());
fn main() {
    let a = &String::from("x");
    let n = len(&String::from("yy"));
    let s = S { r: &String::from("z") };
    let t = (&String::from("w"), 1);
    let f = first(&vec![String::from("q")]).clone();
    let g = { let k = &String::from("k"); k.len() };
    let k = K(&());
    let _ = k.0;
    println!("{} {} {} {} {} {}", a, n, s.r, t.0, f, g);
}
"#;
    assert_eq!(check(ok), vec![]);
    // (a borrow of a temporary that must be 'static)
    let d = check("fn bar<'a>() -> &'a mut u32 {\n    &mut 4\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2, "{d:?}");
}

/// What the review of the first version found (#19): code rustc accepts that must stay accepted, and errors that must
/// be found
#[test]
fn review_cases() {
    let ok = r#"
fn take_g(_g: fn(&'static u8, &'static u8) -> &u8) {}
fn take_h<F: Fn(&'static u8, &'static u8) -> &u8>(_f: F) {}
trait Source { type Item; }
trait Target { type Out; }
struct Wrap<T>(T);
impl<'a, T: Source<Item = &'a u8>> Target for Wrap<T> { type Out = &'a u8; }
trait Get { fn get(&self) -> &'static str; }
impl Get for &'static str { fn get(&self) -> &'static str { *self } }
fn make() -> u32 { 1 }
fn label(_: *const u32) -> &'static str { "ok" }
fn main() {
    let r = label(&make());
    println!("{}", r);
    let s: &'static str = "x";
    println!("{}", s.get());
}
"#;
    assert_eq!(check(ok), vec![]);
    // (two elided inputs are ambiguous whatever an input not understood holds: E0106)
    let d = check("struct Data;\nfn take(_f: fn(&u8, &Data) -> &u8) {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].1.contains("E0106"), "{d:?}");
    // (`let .. else` whose else block matches another value binds from its own scrutinee)
    assert_eq!(check("fn f<'a, 'b>(x: Option<&'a u32>, y: Option<&'b u32>, d: &'a u32) -> &'a u32 {\n    let Some(r) = x else { match y { Some(_) => return d, None => return d } };\n    r\n}\nfn main() {}\n"), vec![]);
    // (an or-pattern arm with a guard is lowered as one arm per alternative)
    one_error("enum E<'a> { L(&'a u32), R(&'a u32), N }\nfn f<'a, 'b>(x: &'a u32, y: E<'b>, flag: bool) -> &'a u32 {\n    match y { E::L(r) | E::R(r) if flag => r, _ => x }\n}\nfn main() {}\n", 3);
    // (a shadowing `let` reads the value it shadows)
    one_error("fn f<'a, 'b>(_x: &'a u32, y: &'b u32) -> &'a u32 {\n    let y = y;\n    y\n}\nfn main() {}\n", 3);
    // (a struct-like variant)
    one_error("enum E<'a> { V { r: &'a u32 } }\nfn f<'a, 'b>(_x: &'a u32, y: &'b u32) -> E<'a> {\n    E::V { r: y }\n}\nfn main() {}\n", 3);
    // (the receiver's tree is the outer call's, though an argument calls a method too)
    one_error("struct S<'a> { r: &'a u32 }\nimpl<'a> S<'a> {\n    fn set(&mut self, v: &'a u32) { self.r = v; }\n    fn get(&self) -> &'a u32 { self.r }\n}\nfn f<'a, 'b>(s: &mut S<'a>, y: &S<'b>) {\n    s.set(y.get());\n}\nfn main() {}\n", 7);
}

/// Variance of recursive types: a cycle is computed to a fixed point (an invariant use anywhere in it makes the whole
/// cycle invariant; a cycle of covariant uses stays covariant)
#[test]
fn recursive_variance() {
    let inv = "struct A<'a> { b: Option<Box<B<'a>>>, p: *mut &'a u32 }\nstruct B<'a> { a: *const A<'a> }\nfn f<'a, 'b: 'a>(x: B<'b>) -> B<'a> {\n    x\n}\nfn main() {}\n";
    one_error(inv, 4);
    let co = "struct List<'a> { next: Option<Box<List<'a>>>, v: &'a u32 }\nfn g<'a, 'b: 'a>(x: List<'b>) -> List<'a> {\n    x\n}\nfn main() {}\n";
    assert_eq!(check(co), vec![]);
    // (E0207: a lifetime only an outlives predicate names is not constrained)
    let d = check("trait Tr { type Out; }\nstruct S;\nimpl<'a> Tr for S where S: 'a { type Out = &'a u8; }\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].1.contains("E0207"), "{d:?}");
    // (E0261 on the bounds of the type's own parameters)
    let d = check("struct S<'a: 'missing>(&'a u8);\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].1.contains("E0261"), "{d:?}");
}

/// A function the gate leaves out (no two regions in its signature) is checked from a `let` annotated with `'static`:
/// a borrow of a local or of a temporary block cannot be 'static
#[test]
fn static_annotations() {
    let d = check("fn main() {\n    let x: &'static u32 = {\n        let y = 42;\n        &y\n    };\n    println!(\"{}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("does not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let x: &'static u32 = &{\n        let y = 42;\n        y\n    };\n    println!(\"{}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("temporary"), "{d:?}");
    assert_eq!(check("fn main() {\n    let x: &'static u32 = &42;\n    let s: &'static str = \"a\";\n    println!(\"{} {}\", x, s);\n}\n"), vec![]);
}

/// Drop check: a value whose type implements `Drop` uses what it borrows when it is dropped, unless it was moved
#[test]
fn drop_check() {
    let bad = "struct Foo<'a> { x: Option<&'a u32> }\nimpl<'a> Drop for Foo<'a> { fn drop(&mut self) {} }\nfn main() {\n    let mut x = Foo { x: None };\n    let y = 0;\n    x.x = Some(&y);\n}\n";
    let d = check(bad);
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("does not live long enough"), "{d:?}");
    let ok = r#"
struct D<'a>(&'a u32);
impl<'a> Drop for D<'a> { fn drop(&mut self) {} }
struct W<'p> { p: &'p mut i32 }
impl<'p> Drop for W<'p> { fn drop(&mut self) { *self.p += 1; } }
fn main() {
    let y = 1;
    let d = D(&y);
    let v = vec![&y];
    println!("{} {:?}", d.0, v);
    let mut x = 0;
    let w = W { p: &mut x };
    std::mem::drop(w);
    x = 1;
    println!("{}", x);
}
"#;
    assert_eq!(check(ok), vec![]);
}

/// A function the gate leaves out is checked from where something must be 'static: a callee's `T: 'static` or
/// `&'static T`, a struct literal with a field written `'static`; `&(&y)` borrows a temporary
#[test]
fn late_static() {
    let d = check("struct Foo { x: &'static u32 }\nfn foo() {\n    let a = 3;\n    let foo = Foo { x: &a };\n    let _ = foo.x;\n}\nfn main() { foo(); }\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`a` does not live long enough"), "{d:?}");
    let d = check("fn keep(_: &'static u32) {}\nfn main() {\n    let n = 42;\n    keep(&n);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("does not live long enough"), "{d:?}");
    let d = check("fn call1<'a>(_x: &'a usize) {\n    let y: usize = 3;\n    let z: &'a &usize = &(&y);\n    let _ = z;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("struct Foo { x: &'static u32 }\nstatic N: u32 = 1;\nfn keep(_: &'static u32) {}\nfn main() {\n    let foo = Foo { x: &N };\n    keep(&N);\n    keep(&5);\n    let _ = foo.x;\n}\n"), vec![]);
}
