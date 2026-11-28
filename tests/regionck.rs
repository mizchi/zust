//! Region checking against signatures (crates/typeck/src/acg/regionck.rs): "lifetime may not live long enough" where
//! the constraints that are known require it, and nothing on code rustc accepts
use zustc::{Options, Session};

fn check(src: &str) -> Vec<(usize, String)> {
    let mut s = Session::new(Options { jobs: 1, no_cache: true, ..Options::default() });
    s.check(src).into_iter().map(|d| (d.line, d.msg)).collect()
}

/// check in an edition
fn check_ed(src: &str, edition: u16) -> Vec<(usize, String)> {
    let mut s = Session::new(Options { jobs: 1, no_cache: true, edition, ..Options::default() });
    s.check(src).into_iter().map(|d| (d.line, d.msg)).collect()
}

fn temp_error(src: &str, line: usize) {
    let d = check(src);
    assert!(d.len() == 1 && d[0].0 == line && d[0].1.contains("temporary value dropped while borrowed"), "{src}\n{d:?}");
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
    let mut s = Session::new(Options { jobs: 1, ..Options::default() });
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

/// A local that keeps a loan of a value already dead, through a call of the program's fn whose result borrows from
/// its argument
#[test]
fn keep_through_calls() {
    let d = check("fn gimme(x: &(u32,)) -> &u32 { &x.0 }\nfn main() {\n    let x = gimme({\n        let v = (22,);\n        &v\n    });\n    println!(\"{:?}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`v` does not live long enough"), "{d:?}");
    // (a block's value that borrows its local is used as an argument after the local is gone: E0597 in rustc too,
    // whatever the callee returns (part 16). One that borrows nothing of the block's is fine)
    let d = check("fn first<'a>(_x: &u32, y: &'a u32) -> &'a u32 { y }\nfn main() {\n    let k = 1;\n    let x = first({ let v = 2; &v }, &k);\n    println!(\"{}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`v` does not live long enough"), "{d:?}");
    assert_eq!(check("fn first<'a>(_x: &u32, y: &'a u32) -> &'a u32 { y }\nfn main() {\n    let k = 1;\n    let x = first({ let v = &k; v }, &k);\n    println!(\"{}\", x);\n}\n"), vec![]);
}

/// A cast's type, with the regions written in it: `&x as &'static u32` borrows x for 'static
#[test]
fn casts() {
    let d = check("fn main() {\n    let x = 22_u32;\n    let y: &u32 = (&x) as &'static u32;\n    let _ = y;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    assert_eq!(check("fn main() {\n    let y = (&5u32) as &'static u32;\n    let s = \"a\" as &'static str;\n    let _ = (y, s);\n}\n"), vec![]);
}

/// Tuple literals, and `ref` / `ref mut` bindings (a `&mut` keeps what it points to invariant)
#[test]
fn tuples_and_ref_bindings() {
    one_error("fn foo(z: &mut Vec<(&u8, &u8)>, (x, y): (&u8, &u8)) {\n    z.push((x, y));\n}\nfn main() {}\n", 2);
    one_error("struct S<'b>(&'b i32);\nimpl<'b> S<'b> {\n    fn bar<'a>(&'a mut self) -> &'a mut &'a i32 {\n        let ref mut x = self.0;\n        x\n    }\n}\nfn main() {}\n", 5);
    assert_eq!(check("fn pair<'a>(x: &'a u8, y: &'a u8) -> (&'a u8, &'a u8) { (x, y) }\nfn main() { let (a, b) = (1, 2); let p = pair(&a, &b); let ref q = p.0; println!(\"{} {}\", p.1, q); }\n"), vec![]);
}

/// A trait object's lifetime: written, or the default (the reference around it, the type parameter's one bound,
/// else 'static in a signature)
#[test]
fn object_lifetimes() {
    one_error("trait Test {}\nfn f<'a>(x: Box<dyn Test + 'a>) -> Box<dyn Test> {\n    x\n}\nfn main() {}\n", 3);
    one_error("trait Test {}\nfn f<'a, 'b>(x: &'a dyn Test) -> &'b dyn Test {\n    x\n}\nfn main() {}\n", 3);
    let ok = r#"
trait Test {}
struct Ref<'a, T: 'a + ?Sized> { r: &'a T }
struct SomeStruct<'a> { t: Ref<'a, dyn Test>, u: Ref<'a, dyn Test + 'a> }
fn c<'a>(t: Ref<'a, dyn Test + 'a>, mut ss: SomeStruct<'a>) { ss.t = t; }
fn d<'a>(x: &'a (dyn Test + 'a)) -> &'a dyn Test { x }
fn e(x: Box<dyn Test>) -> Box<dyn Test + 'static> { x }
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
}

/// `Box::new(x)` holds what x does; a value coerced to a trait object outlives the object's lifetime (unsizing)
#[test]
fn unsizing_to_objects() {
    one_error("use std::fmt::Debug;\nfn f<'a>(x: &'a i32) -> Box<dyn Debug> {\n    Box::new(x)\n}\nfn main() {}\n", 3);
    one_error("trait Foo {}\nimpl<'a> Foo for &'a [u8] {}\nfn c(v: &[u8]) -> Box<dyn Foo> {\n    Box::new(v)\n}\nfn main() {}\n", 4);
    one_error("trait Foo {}\nimpl<'a> Foo for &'a [u8] {}\nfn d<'a, 'b>(v: &'a [u8]) -> Box<dyn Foo + 'b> {\n    let x: Box<dyn Foo + 'b> = Box::new(v);\n    x\n}\nfn main() {}\n", 4);
    one_error("fn f<'a, 'b>(x: &'a i32) -> Box<&'b i32> {\n    Box::new(x)\n}\nfn main() {}\n", 2);
    let ok = r#"
use std::fmt::Debug;
trait Foo {}
impl<'a> Foo for &'a [u8] {}
fn e<'a: 'b, 'b>(v: &'a [u8]) -> Box<dyn Foo + 'b> { Box::new(v) }
fn g<'a>(x: &'a i32) -> Box<dyn Debug + 'a> { Box::new(x) }
fn h(x: &i32) -> Box<dyn Debug> { Box::new(*x) }
fn k<'a>(x: &'a i32) -> Box<&'a i32> { Box::new(x) }
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
}

/// `&e` of a value no constant promotion makes static (a call, an operation on a local, a raw pointer comparison):
/// a temporary; a constant expression is promoted
#[test]
fn unpromoted_temporaries() {
    temp_error("fn main() {\n    let x: &'static i32 = &(5_i32.reverse_bits());\n    let _ = x;\n}\n", 2);
    temp_error("fn foo(_: &'static u32) {}\nfn id<T>(t: T) -> T { t }\nfn main() {\n    foo(&id(5u32));\n}\n", 4);
    temp_error("fn main() {\n    let n = 3;\n    let x: &'static i32 = &(n + 1);\n    let _ = x;\n}\n", 3);
    temp_error("fn main() {\n    let x: &'static bool = &(42 as *const i32 == 43 as *const i32);\n    let _ = x;\n}\n", 2);
    temp_error("fn main() {\n    let x: &'static u32 = unsafe { &std::mem::transmute(3.0f32) };\n    let _ = x;\n}\n", 2);
    let ok = r#"
const C: i32 = 4;
fn main() {
    let a: &'static i32 = &(1 + 2);
    let b: &'static i32 = &(C * 2);
    let c: &'static (i32, bool) = &(3, true);
    let d: &'static usize = &std::mem::size_of::<u64>();
    let n = 3;
    let e = &(n + 1);
    let f = &(5_i32.reverse_bits());
    println!("{} {} {:?} {} {} {}", a, b, c, d, e, f);
}
"#;
    assert_eq!(check(ok), vec![]);
}

/// A closure holds what it captures: by value, the value's regions; by reference, a borrow of the local
#[test]
fn closure_captures() {
    one_error("fn f(x: &isize) -> Box<dyn FnMut() -> isize + 'static> {\n    Box::new(move || *x)\n}\nfn main() {}\n", 2);
    one_error("fn consume<T: 'static>(_: T) {}\nfn foo<'a>(u: &'a u8) {\n    let c = move || u;\n    consume(c);\n}\nfn main() {}\n", 4);
    let d = check("fn f(x: &i32) -> Box<dyn Fn() -> i32> {\n    Box::new(|| *x)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2, "{d:?}");
    let ok = r#"
fn g<'a>(x: &'a isize) -> Box<dyn FnMut() -> isize + 'a> { Box::new(move || *x) }
fn consume<T: 'static>(_: T) {}
fn h<F: Fn() -> usize>(f: F) -> usize { f() }
fn main() {
    let v = vec![1, 2];
    let n = h(|| v.len());
    let s = String::from("a");
    consume(move || s.len());
    let w = 3;
    let c = || w + 1;
    println!("{} {}", n, c());
}
"#;
    assert_eq!(check(ok), vec![]);
}

/// Type arguments written at a call (`f::<&'a u32>()`, `x.m::<&'a u8>()`) carry the regions they name
#[test]
fn turbofish_regions() {
    one_error("fn foo<T: 'static>() {}\nfn boo<'a>() {\n    foo::<&'a u32>();\n}\nfn main() {}\n", 3);
    one_error("struct Foo;\nimpl Foo {\n    fn some_method<A: 'static>(self) {}\n}\nfn caller<'a>(_x: &isize) {\n    Foo.some_method::<&'a isize>();\n}\nfn main() {}\n", 6);
    let ok = "fn foo<T: 'static>() {}\nfn id<T>(t: T) -> T { t }\nfn boo<'a>(x: &'a u32) -> &'a u32 {\n    foo::<&'static u32>();\n    id::<&'a u32>(x)\n}\nfn main() {}\n";
    assert_eq!(check(ok), vec![]);
}

/// A borrow of a local that escapes as the returned value is reported where it is returned (E0515); a raw pointer
/// cast may change the regions of what it points to; a projection's self type implies its bounds
#[test]
fn returns_casts_projections() {
    let d = check("fn f<'a, 'b>(x: i32) -> (&'a i32, &'b i32) {\n    let y = &x;\n    (y, y)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0515"), "{d:?}");
    let d = check("fn id<T>(x: T) -> T { x }\nstruct Test;\nfn s<'a>() -> &'a Test {\n    let t = &id(Test);\n    t\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("temporary"), "{d:?}");
    let ok = r#"
struct Foo<'a> { a: &'a () }
fn extend<'a>(v: *const Foo<'a>) -> *const Foo<'static> { v as *const Foo<'static> }
trait Trait { type Type; }
impl<T> Trait for T { type Type = (); }
fn f<'a, 'b>(s: &'b str, _: <&'a &'b () as Trait>::Type) -> &'a str where &'a &'b (): Trait { s }
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
    // (the result of `f<'a, 'b: 'a>(x: &'b T) -> &'a T` may borrow x)
    let d = check("fn f<'a, 'b: 'a>(x: &'b String, _y: &'a u8) -> &'a str { x.as_str() }\nfn main() {\n    let s = String::from(\"a\");\n    let n = 1u8;\n    let r = f(&s, &n);\n    drop(s);\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("because it is borrowed"), "{d:?}");
}

/// The types an item writes are well formed: `&'a T` needs T: 'a (E0309), `&'static T` T: 'static (E0310),
/// `&'a &'b U` 'b: 'a (E0491), as declared or implied by the signature's or the impl header's types
#[test]
fn written_types_well_formed() {
    let first = |src: &str| check(src).into_iter().next().map(|d| (d.0, d.1)).unwrap_or_default();
    let d = first("struct Foo<T> {\n    foo: &'static T,\n}\nfn main() {}\n");
    assert!(d.0 == 2 && d.1.contains("E0310"), "{d:?}");
    let d = first("pub trait Foo<'a> {\n    type Bar;\n}\nimpl<'a, T> Foo<'a> for T {\n    type Bar = &'a T;\n}\nfn main() {}\n");
    assert!(d.0 == 5 && d.1.contains("E0309"), "{d:?}");
    let d = first("fn ordering4<'a, 'b, F>(a: &'a usize, b: &'b usize, x: F) where F: FnOnce(&'a &'b usize) {}\nfn main() {}\n");
    assert!(d.0 == 1 && d.1.contains("E0491"), "{d:?}");
    // (in a fn pointer's or a trait object's type)
    let d = first("struct Foo<T> {\n    x: fn() -> &'static T,\n}\nfn main() {}\n");
    assert!(d.0 == 2 && d.1.contains("E0310"), "{d:?}");
    let d = first("trait Object<T> {}\nstruct Foo<T> {\n    x: Box<dyn Object<&'static T>>,\n}\nfn main() {}\n");
    assert!(d.0 == 3 && d.1.contains("E0310"), "{d:?}");
    let ok = r#"
pub trait Foo<'a> { type Bar; }
impl<'a, T: 'a> Foo<'a> for T { type Bar = &'a T; }
pub trait Baz<'a> { type Q; }
impl<'a, T> Baz<'a> for &'a T { type Q = &'a T; }
struct Ok<T: 'static> { x: &'static T }
struct Inferred<'a, T> { x: &'a T }
trait Mine {}
fn f1<'a, 'b, F>(a: &'a &'b usize, x: F) where F: FnOnce(&'a &'b usize) {}
fn f2<'a, 'b: 'a, F: FnOnce(&'a &'b usize)>(x: F) {}
fn f3<'a, K: Mine, L: Into<&'a K>>() {}
fn f4<K, F: Fn(&K)>(x: F) {}
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
}

/// A check begun late (a `'static` annotation in a function the gate left out) knows its parameters; array literals
/// and indexed places are followed; a reborrow through a shared reference lives as long as it alone
#[test]
fn late_params_arrays_indexes() {
    one_error("struct Dog { c: usize }\nimpl Dog {\n    fn chase(&mut self) {\n        let p: &'static mut usize = &mut self.c;\n        *p += 1;\n    }\n}\nfn main() {}\n", 4);
    temp_error("static FOO: u32 = 50;\nfn main() {\n    let v: &'static [&'static u32; 1] = &[&FOO];\n    let _ = v;\n}\n", 3);
    one_error("pub struct Buffer<'a> { buf: &'a mut [u8] }\npub fn foo<'a>(buffer: &mut Buffer<'a>) {\n    buffer.buf = &mut buffer.buf[..];\n}\nfn main() {}\n", 3);
    let ok = r#"
fn read(buf: &mut &[u8]) -> u8 {
    let v = buf[0];
    *buf = &buf[1..];
    v
}
fn first<'a>(xs: &'a [u32]) -> &'a u32 { &xs[0] }
fn mid<'a>(v: &'a Vec<u8>) -> &'a [u8] { &v[1..] }
fn main() {
    let a: &'static [u32; 3] = &[1, 2, 3];
    let mut s: &[u8] = &[1, 2];
    println!("{} {} {:?}", read(&mut s), first(a), mid(&vec![1, 2]));
}
"#;
    assert_eq!(check(ok), vec![]);
}

/// Tuple struct constructors (`B(a)`), calls through trait objects, and unit structs that need dropping (not
/// promoted); a constructor of constants is promoted
#[test]
fn ctors_dyn_calls_drop_units() {
    one_error("trait Foo { fn dummy(&self) {} }\nstruct B<'a>(&'a (dyn Foo + 'a));\nfn foo<'a>(a: &dyn Foo) -> B<'a> {\n    B(a)\n}\nfn main() {}\n", 4);
    one_error("trait Foo { fn borrowed<'a>(&'a self) -> &'a (); }\nfn f<'a, 'b>(x: &'a dyn Foo) -> &'b () {\n    x.borrowed()\n}\nfn main() {}\n", 3);
    // (a method of the prelude taking `&self` returns a borrow of self)
    one_error("fn bad<'a>(s: &String) -> &'a str {\n    s.as_str()\n}\nfn main() {}\n", 2);
    assert_eq!(check("fn ok<'a>(s: &'a String) -> &'a str { s.as_str() }\nfn first<'a>(v: &'a [u8]) -> Option<&'a u8> { let mut it = v.iter(); it.next() }\nfn main() {}\n"), vec![]);
    let d = check("struct Test;\nimpl Drop for Test { fn drop(&mut self) {} }\nfn create<'a>() -> &'a Test {\n    let t = &Test;\n    return t;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0515"), "{d:?}");
    let ok = r#"
struct Generic<T>(T);
struct Struct<'a, T>(&'a T);
struct Unit;
trait Foo { fn borrowed<'a>(&'a self) -> &'a (); }
fn fine<'a>() -> Struct<'a, Generic<()>> { Struct(&Generic(())) }
fn unit<'a>() -> &'a Unit { &Unit }
fn same<'a>(x: &'a dyn Foo) -> &'a () { x.borrowed() }
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
}

/// A closure's body is checked against the signature the bound it is passed to gives it (the regions of its
/// parameters are the bound's, higher-ranked; its result's, the bound's), or against its annotations
#[test]
fn closure_bodies() {
    one_error("fn foo(_: impl FnOnce(&u32) -> &u32) {}\nfn bar() {\n    let x = 22;\n    foo(|a| &x);\n}\nfn main() {}\n", 4);
    one_error("fn with<R, F>(f: F) -> R where F: FnOnce(&isize) -> R { f(&3) }\nfn return_it<'a>() -> &'a isize {\n    with(|o| o)\n}\nfn main() {}\n", 3);
    one_error("fn foo(_: impl for<'a> FnOnce(&'a u32, &u32) -> &'a u32) {}\nfn main() {\n    foo(|a, b| b);\n}\n", 3);
    one_error("fn main() {\n    let i = &3;\n    let f = |x: &i32| -> &i32 { x };\n    let _ = f(i);\n}\n", 3);
    let ok = r#"
struct A { name: String }
fn foo(_: impl FnOnce(&u32) -> &u32) {}
fn pick(_: impl for<'a> FnOnce(&'a u32, &u32) -> &'a u32) {}
fn with<R, F>(f: F) -> R where F: FnOnce(&isize) -> R { f(&3) }
fn main() {
    foo(|a| a);
    pick(|a, _b| a);
    let n = with(|o| *o + 1);
    let v = vec![1];
    let g = || &v;
    let h = |x| x;
    let a = [A { name: "1".to_string() }, A { name: "2".to_string() }];
    let s = a.is_sorted_by_key(|a| a.name.as_str());
    println!("{} {:?} {} {}", n, g(), h(1), s);
}
"#;
    assert_eq!(check(ok), vec![]);
}

/// A signature that writes 'static is checked; a by-value `self` keeps the result's named regions; a borrow of a
/// Box's content is a loan; a local bound to a fn with lifetime parameters is that fn at its calls; an impl method
/// ties no region the trait leaves free to its header's lifetimes
#[test]
fn statics_selfs_boxes_aliases_impls() {
    let d = check("fn function(x: isize) -> &'static [isize] {\n    &[x]\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0515"), "{d:?}");
    one_error("struct Flag<'a> { desc: &'a str }\nimpl<'a> Flag<'a> {\n    fn set(self, s: &str) -> Flag<'a> {\n        Flag { desc: s }\n    }\n}\nfn main() {}\n", 4);
    let d = check("struct B { i: isize }\nstruct A<'r> { p: &'r B }\nfn make_a(p: &B) -> A { A { p } }\nfn mk<'a>() -> A<'a> {\n    let b: Box<B> = Box::new(B { i: 1 });\n    let bb: &B = &*b;\n    make_a(bb)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0515"), "{d:?}");
    one_error("use std::cell::Cell;\nfn check<'a, 'b>(_x: Cell<&'a ()>, _y: Cell<&'b ()>) where 'a: 'b {}\nfn test<'a, 'b>(x: Cell<&'a ()>, y: Cell<&'b ()>) {\n    let f = check;\n    f(x, y);\n}\nfn main() {}\n", 5);
    let d = check("struct R<'a, T>(T, &'a ());\nimpl<'a, T: 'a> Iterator for R<'a, T> {\n    type Item = &'a mut T;\n    fn next(&'a mut self) -> Option<Self::Item> { None }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0308"), "{d:?}");
    let ok = r#"
struct R<'a, T>(T, &'a T);
impl<'a, T: 'a> Iterator for R<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> { Some(self.1) }
}
struct Flag<'a> { desc: &'a str }
impl<'a> Flag<'a> { fn set(self, s: &'a str) -> Flag<'a> { Flag { desc: s } } }
fn s() -> &'static str { "a" }
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
}

/// `let _: T = e;` relates e to T; fn pointer types carry their regions (contravariant in the parameters), and so
/// make structs that hold them contravariant; a type argument's well-formedness is implied (`Option<&'s &'l ()>`);
/// `self: &Struct` elides as `&self` does
#[test]
fn wildcards_fn_pointers_typed_self() {
    one_error("struct Co<'a> { f: fn(&'a isize) }\nfn use_<'short, 'long>(c: Co<'long>, _w: Option<&'short &'long ()>) {\n    let _: Co<'short> = c;\n}\nfn main() {}\n", 3);
    one_error("use std::cell::Cell;\nstruct Foo<'a> { x: Cell<Option<&'a isize>> }\nfn use_<'short, 'long>(c: Foo<'short>, _w: Option<&'short &'long ()>) {\n    let _: Foo<'long> = c;\n}\nfn main() {}\n", 4);
    one_error("struct S {}\nimpl S {\n    fn r(self: &S, f: &u32) -> &u32 {\n        f\n    }\n}\nfn main() {}\n", 4);
    let ok = r#"
struct Contra<'a> { f: fn(&'a isize) }
struct S<'a, 'b> { f: &'a isize, g: &'b isize }
type F<'a> = fn(&mut Vec<&'a u8>) -> Option<&'static str>;
fn a<'short, 'long>(c: Contra<'short>, _w: Option<&'short &'long ()>) { let _: Contra<'long> = c; }
fn b<'short, 'long>(c: S<'long, 'short>, _w: Option<&'short &'long ()>) { let _: S<'long, 'short> = c; let _: S<'short, 'short> = c; }
fn apply<'a>(f: fn(&'a u8) -> &'a u8, x: &'a u8) -> &'a u8 { f(x) }
fn g(_: &mut Vec<&u8>) -> Option<&'static str> { None }
fn alias<'a>(v: &'a F<'a>) -> &'a F<'a> { v }
struct T {}
impl T { fn r(self: &T, _f: &u32) -> &T { self } }
fn main() { let f: F = g; let _ = alias(&f); }
"#;
    assert_eq!(check(ok), vec![]);
}

/// A type written in a body (a `let` annotation, a struct literal's) is well formed; `if` / `match` values follow
/// their branches; a Box's content is borrowed for a `&self` method called through a trait object
#[test]
fn body_wf_branches_box_receivers() {
    one_error("fn call2<'a, 'b>(_a: &'a usize, _b: &'b usize) {\n    let _z: Option<&'b &'a usize> = None;\n}\nfn main() {}\n", 2);
    one_error("use std::cell::Cell;\nstruct Foo<'a: 'b, 'b> { x: Cell<&'a u32>, y: Cell<&'b u32> }\nfn bar<'a, 'b>(x: Cell<&'a u32>, y: Cell<&'b u32>) {\n    Foo { x, y };\n}\nfn main() {}\n", 4);
    one_error("struct Inv<'a>(Option<&'a mut &'a mut ()>);\nfn mk() -> Inv<'static> { Inv(None) }\nfn unify<'a>(x: Option<Inv<'a>>, f: fn(Inv<'a>)) {\n    let bad = if x.is_some() {\n        x.unwrap()\n    } else {\n        mk()\n    };\n    f(bad);\n}\nfn main() {}\n", 7);
    let d = check("trait Foo { fn borrowed<'a>(&'a self) -> &'a (); }\nfn owned(x: Box<dyn Foo>) -> &'static () {\n    x.borrowed()\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0515"), "{d:?}");
    let ok = r#"
struct Foo<'a: 'b, 'b> { x: &'a u32, y: &'b u32 }
fn bar<'a, 'b>(x: &'a u32, y: &'b u32) -> u32 { let f = Foo { x, y }; *f.x + *f.y }
fn pick<'a>(c: bool, x: &'a u8, y: &'a u8) -> &'a u8 { let r = if c { x } else if *y > 0 { y } else { panic!() }; r }
fn m<'a>(o: Option<&'a u8>, d: &'a u8) -> &'a u8 { let r = match o { Some(v) => v, None => d }; r }
fn w<'a, 'b: 'a>(_x: &'a u8, _y: &'b u8) { let _z: Option<&'a &'b u8> = None; }
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
}

/// A check begun late that meets a local bound before it began is done again from the function's start
#[test]
fn late_check_from_the_start() {
    let d = check("fn foo<F: FnOnce() + 'static>(_p: F) {}\nfn main() {\n    let x = 3;\n    let y = &x;\n    foo(move || {\n        let _a = *y;\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    let d = check("struct S { p: &'static str }\nfn main() {\n    let p = \"F\".to_string();\n    let p: &str = &p;\n    let s: Box<S> = Box::new(S { p: p });\n    let _: &'static str = \"ok\";\n    let _ = s;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    assert_eq!(check("fn main() {\n    let v = vec![1];\n    let r = &v;\n    let s: &'static str = \"a\";\n    println!(\"{:?} {}\", r, s);\n}\n"), vec![]);
}

/// A trait's `Self: 'x` bounds hold of the types that implement it and of its objects (E0478, E0310)
#[test]
fn trait_self_outlives() {
    let first = |src: &str| check(src).into_iter().next().map(|d| (d.0, d.1)).unwrap_or_default();
    let d = first("trait Wedding<'t>: 't {}\nstruct Prince<'kiss, 'snow> {\n    child: Box<dyn Wedding<'kiss> + 'snow>,\n}\nfn main() {}\n");
    assert!(d.0 == 3 && d.1.contains("E0478"), "{d:?}");
    let d = first("pub trait Arbitrary: Sized + 'static {}\nstruct W<'a, A>(&'a A);\nimpl<'a, A: Clone> Arbitrary for W<'a, A> {}\nfn main() {}\n");
    assert!(d.0 == 3 && d.1.contains("E0478"), "{d:?}");
    // (a struct's own requirements on its arguments are not seen: `V<A>` may require `A: 'static`)
    let d = first("pub trait Arbitrary: 'static {}\nstruct V<A>(A);\nimpl<A: Clone> Arbitrary for Vec<A> {}\nfn main() {}\n");
    assert!(d.0 == 3 && d.1.contains("E0310"), "{d:?}");
    // (a lifetime used only in a bound's `Fn(..)` arguments is used)
    assert_eq!(check("struct Inv<'a> { f: Box<dyn FnOnce(&mut &'a isize) + 'static> }\nfn main() {}\n"), vec![]);
    let ok = r#"
trait Wedding<'t>: 't {}
struct Prince<'kiss, 'snow: 'kiss> { child: Box<dyn Wedding<'kiss> + 'snow> }
pub trait Arbitrary: 'static {}
struct V<A>(A);
impl<A: Clone + 'static> Arbitrary for V<A> {}
struct W<'a, A>(&'a A);
impl<'a, A: Clone> Clone for W<'a, A> { fn clone(&self) -> Self { W(self.0) } }
fn main() {}
"#;
    assert_eq!(check(ok), vec![]);
}

/// The two sides of a comparison are of one type: their invariant parts are the same
#[test]
fn comparisons() {
    one_error("fn cmp<'a, 'b>(x: *mut &'a i32, y: *mut &'b i32) -> bool {\n    x == y\n}\nfn main() {}\n", 2);
    assert_eq!(check("fn hr<'a>(f: fn(&'a i32), g: fn(&i32)) -> bool { f == g }\nfn co<'a, 'b>(x: &'a i32, y: &'b i32) -> bool { x == y }\nfn main() {}\n"), vec![]);
}

/// A receiver of the self type names its lifetimes as the impl's self type does (E0308)
#[test]
fn self_parameter_lifetimes() {
    let d = check("struct Foo<'a, 'b> { x: &'a isize, y: &'b isize }\nimpl<'a, 'b> Foo<'a, 'b> {\n    fn bar(\n        self: Foo<'b, 'a>,\n    ) {}\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0308"), "{d:?}");
    assert_eq!(check("struct Foo<'a, 'b> { x: &'a isize, y: &'b isize }\nimpl<'a, 'b> Foo<'a, 'b> {\n    fn bar(self: Foo<'a, 'b>) {}\n    fn baz(self: &Foo<'a, 'b>) {}\n    fn qux(self: Box<Self>) {}\n}\nfn main() {}\n"), vec![]);
}

/// A closure's captures hold regions of the function it is in, related in its body as that function knows them
#[test]
fn closure_captures_regions() {
    one_error("trait B {}\nimpl<F> B for F {}\nfn produce<'a, 'b: 'a>(data: &'b mut Vec<&'b u32>, value: &'a u32) -> impl B + 'b {\n    let x = move || {\n        let value: &'a u32 = value;\n        data.push(value);\n    };\n    x\n}\nfn main() {}\n", 6);
    let ok = r#"
trait B {}
impl<F> B for F {}
fn produce<'a, 'b: 'a>(data: &'a mut Vec<&'a u32>, value: &'b u32) -> impl B + 'a {
    let x = move || { let value: &'a u32 = value; data.push(value); };
    x
}
pub fn f<'a>(_: &'a i32, _: impl FnOnce(&mut &mut &'a i32)) {}
pub fn h<'a>(p: &'a i32) { f(p, |x: &mut &mut &'a i32| { let _: &mut &mut &'a i32 = x; }) }
fn main() {
    let v = vec![1u8];
    let r = &v;
    let c = || r.len();
    println!("{}", c());
}
"#;
    assert_eq!(check(ok), vec![]);
}

/// An impl's self type is well formed (required of a call of its methods); a for loop's items of a Vec, an array or
/// a slice have their elements' trees; a closure with no bound whose result is not written borrows nothing it owns
/// for its result, and is otherwise free
#[test]
fn self_wf_for_items_soft_results() {
    one_error("struct Foo<'a, 'b: 'a>(&'a &'b ());\nimpl<'a, 'b> Foo<'a, 'b> {\n    fn xmute(_a: &'b ()) -> &'a () { unreachable!() }\n}\npub fn foo<'a, 'b>(u: &'b ()) -> &'a () {\n    Foo::xmute(u)\n}\nfn main() {}\n", 6);
    let d = check("struct D<'a>(&'a ());\nfn f(v: Vec<D<'static>>) {\n    for ref mut d in v {\n        let y = ();\n        *d = D(&y);\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`y` does not live long enough"), "{d:?}");
    let ok = r#"
struct UsizeRef<'a> { a: &'a usize }
type RefTo = Box<dyn for<'r> Fn(&'r Vec<usize>) -> UsizeRef<'r>>;
fn first<'a>(v: &'a [&'a u8]) -> &'a u8 { for x in v { return *x; } &0 }
fn main() {
    let a: RefTo = Box::new(|vec: &Vec<usize>| UsizeRef { a: &vec[0] });
    let _ = a;
}
"#;
    assert_eq!(check(ok), vec![]);
}

/// E0195: an impl method's early-bound lifetimes (those with bounds) are the trait's; E0106 in extern blocks and for
/// `'_` in a field's trait object
#[test]
fn early_bound_and_extern_elision() {
    let d = check("trait Trait {\n    fn bar<'a, 'b: 'a>(x: &'a str, y: &'b str);\n}\nstruct Foo;\nimpl Trait for Foo {\n    fn bar<'a, 'b>(_x: &'a str, _y: &'b str) {}\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0195"), "{d:?}");
    let d = check("extern \"C\" {\n    pub fn g(_: &u8) -> &u8;\n    pub fn f() -> &u8;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0106"), "{d:?}");
    let d = check("use std::fmt::Debug;\nstruct Foo {\n    x: Box<dyn Debug + '_>,\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0106"), "{d:?}");
    assert_eq!(check("trait Trait { fn bar<'a, 'b: 'a>(x: &'a str, y: &'b str); }\nstruct Foo;\nimpl Trait for Foo { fn bar<'a, 'b: 'a>(_x: &'a str, _y: &'b str) {} }\nfn main() {}\n"), vec![]);
}

/// E0106 for an associated type's value that elides, and for a method with no receiver that elides as a free fn
#[test]
fn assoc_type_and_method_elision() {
    let d = check("trait MyTrait { type Output; }\nimpl MyTrait for &i32 {\n    type Output = &i32;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0106"), "{d:?}");
    let d = check("trait Foo {}\nstruct Xyz;\nimpl Xyz {\n    fn do_sth<'a>(\n        foo: &dyn Foo, _bar: &'a dyn Foo\n    ) -> &dyn Foo {\n        foo\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0106"), "{d:?}");
    assert_eq!(check("trait MyTrait { type Output; }\nimpl<'a> MyTrait for &'a i32 { type Output = &'a i32; }\nstruct X;\nimpl X { fn one(x: &u8) -> &u8 { x } }\nfn main() {}\n"), vec![]);
}

/// A closure that borrows a local and is returned: E0373 at the closure
#[test]
fn closure_returned_with_borrows() {
    let d = check("fn f() {\n    let g = |x: usize, y: usize| {\n        let f = |t: bool| if t { x } else { y };\n        return f;\n    };\n    let _ = g;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0373"), "{d:?}");
}

/// E0106 is not reported when a parameter is a struct that may hold lifetimes it does not write; it is reported at the
/// elided reference of a multi-line result type
#[test]
fn elision_with_hidden_lifetimes() {
    assert_eq!(check("struct S<'a> { x: &'a String }\nfn f(s: S) -> &String { s.x }\ntype S2<'a> = S<'a>;\nfn g(s: S2) -> &String { s.x }\nfn main() {}\n"), vec![]);
    let d = check("fn f() -> Vec<\n    &str\n> {\n    vec![]\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0106"), "{d:?}");
}

/// A struct holding a trait object is invariant in the lifetimes the object's trait arguments name
#[test]
fn invariant_through_trait_object_arguments() {
    let d = check("struct Inv<'a> {\n    f: Box<dyn FnOnce(&mut &'a isize) + 'static>,\n}\nfn longer<'r>(b: Inv<'r>) -> Inv<'static> {\n    b\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
    assert_eq!(check("struct Inv<'a> {\n    f: Box<dyn FnOnce(&mut &'a isize) + 'static>,\n}\nfn same<'r>(b: Inv<'r>) -> Inv<'r> {\n    b\n}\nfn main() {}\n"), vec![]);
}

/// A cast of a pointer to a trait object (or to a struct ending in one) may not lengthen the object's lifetime, also
/// when the target is inferred (`as _`)
#[test]
fn pointer_casts_keep_the_object_lifetime() {
    let d = check("trait Trait {}\nfn f<'a>(p: *mut (dyn Trait + 'a)) -> *mut (dyn Trait + 'static) {\n    p as _\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    let d = check("trait Trait {}\nstruct W<T: ?Sized>(T);\nfn f<'a>(p: *mut W<dyn Trait + 'a>) -> *mut W<dyn Trait + 'static> {\n    p as _\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    assert_eq!(check("trait Trait {}\nfn f<'a>(p: *mut (dyn Trait + 'static)) -> *mut (dyn Trait + 'a) {\n    p as _\n}\nfn main() {}\n"), vec![]);
}

/// `Self { .. }` builds the impl's self type, whose `'_` is a lifetime of the impl
#[test]
fn self_literal_has_the_impl_lifetimes() {
    let d = check("pub struct S<'a> {\n    f: &'a [u32],\n}\nimpl S<'_> {\n    pub fn new<'a>(f: &'a [u32]) -> S<'a> {\n        Self { f }\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    assert_eq!(check("pub struct S<'a> {\n    f: &'a [u32],\n}\nimpl<'b> S<'b> {\n    pub fn new(f: &'b [u32]) -> Self {\n        Self { f }\n    }\n}\nimpl S<'_> {\n    pub fn new2<'a>(f: &'a [u32]) -> S<'a> {\n        S { f }\n    }\n}\nfn main() {}\n"), vec![]);
}

/// `&*s` of a String or a Vec borrows the local
#[test]
fn deref_of_owned_string_and_vec() {
    let d = check("fn assert_static<T: 'static>(_t: T) {}\nfn main() {\n    let line = String::new();\n    match [&*line] {\n        [word] => assert_static(word),\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    let d = check("fn keep(_: &'static [u8]) {}\nfn main() {\n    let v: Vec<u8> = Vec::new();\n    keep(&*v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    assert_eq!(check("fn keep(_: &[u8]) {}\nfn main() {\n    let v: Vec<u8> = Vec::new();\n    keep(&*v);\n    let s = String::new();\n    let t: &str = &*s;\n    let _ = t;\n}\n"), vec![]);
}

/// The annotation of a `let` with no initializer is well formed, as is one in code after a `return`
#[test]
fn wf_of_uninitialized_lets() {
    let d = check("fn f<'a>() {\n    let x: &'static &'a ();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2, "{d:?}");
    let d = check("fn f<'a>() {\n    return;\n    let _: &'static &'a () = &&();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("fn f<'a>() {\n    let x: &'a &'static ();\n    x = &&();\n    let _ = x;\n}\nfn main() {}\n"), vec![]);
}

/// A reborrow through a parameter that must outlive one of the signature's lifetimes lives to the end of the
/// function: a later conflicting use of the place is an error
#[test]
fn reborrows_into_universal_regions_live_to_the_end() {
    let d = check("fn f<'a>(v: &'a mut Vec<u8>, keep: &mut Vec<&'a u8>) {\n    keep.push(&v[0]);\n    v.push(1);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("cannot borrow"), "{d:?}");
    let d = check("struct S<'a> {\n    r: Option<&'a mut u8>,\n}\nfn g<'a>(s: &mut S<'a>, x: &'a mut u8) {\n    s.r = Some(&mut *x);\n    *x = 2;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot assign"), "{d:?}");
    // (a reborrow for a shorter region ends where it is last used)
    assert_eq!(check("fn f<'a>(v: &'a mut Vec<u8>) {\n    let mut keep: Vec<&u8> = Vec::new();\n    keep.push(&v[0]);\n    keep.clear();\n    v.push(1);\n}\nfn main() {}\n"), vec![]);
}

/// A closure that returns a mutable borrow of what it captures by reference is `FnMut`: the borrow cannot escape
/// the call
#[test]
fn fnmut_closure_returning_a_captured_borrow() {
    let d = check("fn main() {\n    let mut x = 0;\n    let mut f = || &mut x;\n    let _ = f();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("cannot escape `FnMut`"), "{d:?}");
    let d = check("fn main() {\n    let mut v: Vec<()> = Vec::new();\n    || {\n        &mut v\n    };\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    // (a shared borrow, a closure passed where `FnOnce` is expected, one that returns a parameter)
    assert_eq!(check("fn once<R>(f: impl FnOnce() -> R) -> R {\n    f()\n}\nfn main() {\n    let x = 0;\n    let f = || &x;\n    let _ = f();\n    let mut y = 0;\n    let _ = once(|| &mut y);\n    let g = |z: &mut u8| &mut *z;\n    let mut w = 0;\n    let _ = g(&mut w);\n}\n"), vec![]);
}

/// A region the closure's bound binds flowing into a local of the function the closure is in
#[test]
fn borrowed_data_escapes_the_closure() {
    let d = check("fn with_int<F: FnOnce(&isize)>(f: F) {\n    let x = 3;\n    f(&x);\n}\nfn main() {\n    let mut x: Option<&isize> = None;\n    with_int(|y| x = Some(y));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("escapes outside of closure"), "{d:?}");
    assert_eq!(check("fn with_int<F: FnOnce(&isize)>(f: F) {\n    let x = 3;\n    f(&x);\n}\nfn main() {\n    let mut x: Option<isize> = None;\n    with_int(|y| x = Some(*y));\n    let z = 4;\n    let r = &z;\n    with_int(|y| {\n        let _ = (r, y);\n    });\n}\n"), vec![]);
}

/// A closure with no `move` borrows the copies it reads: one that outlives the block of what it reads is an error
#[test]
fn closures_borrow_the_copies_they_read() {
    let d = check("fn main() {\n    let f = {\n        let x = 0;\n        || x\n    };\n    let _ = f();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("fn main() {\n    let f = {\n        let x = 0;\n        move || x\n    };\n    let _ = f();\n    let y = 1;\n    let g = || y + 1;\n    let _ = (g(), y);\n}\n"), vec![]);
}

/// Returning a borrow of a temporary a call made (not promoted): E0515, also in a function the region check's gate
/// leaves out
#[test]
fn returned_borrow_of_a_temporary() {
    let d = check("fn id<T>(x: T) -> T {\n    x\n}\nfn f(_x: &isize) -> &isize {\n    return &id(3);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0515"), "{d:?}");
    assert_eq!(check("fn h() -> &'static str {\n    \"a\"\n}\nfn g(_x: &u8) -> &'static str {\n    h()\n}\nfn k(x: &u8) -> &u8 {\n    &*x\n}\nfn main() {}\n"), vec![]);
}

/// A closure made where nothing was expected has one region for a parameter with no type written: passed where a
/// bound takes a reference for any region, it is not general enough
#[test]
fn closure_parameter_not_general_enough() {
    let d = check("fn thing(_x: impl FnOnce(&u32)) {}\nfn main() {\n    let f = |_| ();\n    thing(f);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("not general enough"), "{d:?}");
    // (written in place, or with the parameter's type written: higher-ranked)
    assert_eq!(check("fn thing(_x: impl FnOnce(&u32)) {}\nfn main() {\n    thing(|_| ());\n    let f = |_: &u32| ();\n    thing(f);\n    let g = |x: u32| x;\n    let _ = g(1);\n}\n"), vec![]);
}

/// A fn whose result borrows from its parameter, passed where the bound's result is a type parameter: one type
/// for every region, which the fn's result is not
#[test]
fn fn_item_not_general_enough() {
    let d = check("fn all<F, T>(_f: F) where F: FnMut(&String) -> T {}\nfn id(x: &String) -> &String {\n    x\n}\nfn main() {\n    all(id);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("not general enough"), "{d:?}");
    assert_eq!(check("fn all<F, T>(_f: F) where F: FnMut(&String) -> T {}\nfn len(x: &String) -> usize {\n    x.len()\n}\nfn all2<F>(_f: F) where F: FnMut(&String) -> &String {}\nfn id(x: &String) -> &String {\n    x\n}\nfn main() {\n    all(len);\n    all2(id);\n}\n"), vec![]);
}

/// `#![deny(unused_lifetimes)]`, `#![deny(single_use_lifetimes)]`
#[test]
fn unused_and_single_use_lifetime_lints() {
    let d = check("#![deny(unused_lifetimes)]\nstruct Foo {}\nimpl<'a> Foo {}\nfn f<'b>(x: &'b u8) -> &'b u8 {\n    x\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("never used"), "{d:?}");
    let d = check("#![deny(single_use_lifetimes)]\nstruct Foo<'f> {\n    data: &'f u32,\n}\nimpl<'f> Foo<'f> {\n    fn get(&self) -> u32 {\n        *self.data\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("only used once"), "{d:?}");
    // (used twice; once in the result; once in an `impl` type; bounded)
    assert_eq!(check("#![deny(single_use_lifetimes)]\nfn a<'a>(x: &'a u32) -> &'a u32 {\n    x\n}\nfn b<'a>() -> &'a u32 {\n    &1\n}\nfn c<'a>(x: impl IntoIterator<Item = &'a i32>) {\n    for _ in x {}\n}\nfn d<'a, 'b: 'a>(_: &'a u8, _: &'b u8) {}\nfn main() {}\n"), vec![]);
}

/// E0637: `'_` as a bound, a reference with no lifetime in a where clause's bound
#[test]
fn anonymous_lifetimes_in_bounds() {
    let d = check("trait Foo<'a> {}\nimpl<'b: '_> Foo<'b> for i32 {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0637"), "{d:?}");
    let d = check("trait WithType<T> {}\nstruct Foo<T>(T);\nimpl<T> Foo<T>\nwhere\n    T: WithType<&u32>,\n{\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0637"), "{d:?}");
    assert_eq!(check("fn f<F: Fn(&u32) -> u32>(f: F) -> u32 {\n    f(&1)\n}\nfn g<'a, I: Iterator<Item = &'a u32>>(_i: I) {}\nfn main() {\n    let _ = f(|x| *x);\n}\n"), vec![]);
}

/// `for<'a> T: 'a` is `T: 'static`
#[test]
fn higher_ranked_outlives_is_static() {
    let d = check("fn foo<T>() where for<'a> T: 'a {}\nfn bar<'a>() {\n    foo::<&'a i32>();\n}\nfn main() {\n    bar();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("fn foo<T>() where for<'a> T: 'a {}\nfn main() {\n    foo::<&'static i32>();\n    foo::<u8>();\n}\n"), vec![]);
}

/// `sort_by_key` and the like take `FnMut(&T) -> K` for any region, K one type: a key borrowed from the element is an
/// error
#[test]
fn by_key_closures_cannot_return_borrows() {
    let d = check("struct P {\n    name: String,\n}\nfn main() {\n    let mut v: Vec<P> = Vec::new();\n    v.sort_by_key(|p| &p.name);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    assert_eq!(check("struct P {\n    name: String,\n    age: u32,\n}\nfn main() {\n    let mut v: Vec<P> = Vec::new();\n    v.sort_by_key(|p| p.age);\n    v.sort_by_key(|p| p.name.clone());\n    let a = v.iter().max_by_key(|&p| &p.name);\n    let mut w: Vec<&str> = vec![\"a\"];\n    w.sort_by_key(|s| *s);\n    let _ = a;\n}\n"), vec![]);
}

/// A statement's temporary borrowed by an assignment, or by a method of a local holding references, goes away at
/// the statement's end: E0716 when the borrow is used later
#[test]
fn statement_temporaries() {
    let d = check("fn id<T>(x: T) -> T {\n    x\n}\nfn main() {\n    let mut x;\n    x = &id(3);\n    let y = *x;\n    let _ = y;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0716"), "{d:?}");
    let d = check("fn id<T>(x: T) -> T {\n    x\n}\nfn main() {\n    let mut v: Vec<&i32> = Vec::new();\n    v.push(&id(3));\n    let _ = v.len();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check("fn id<T>(x: T) -> T {\n    x\n}\nfn main() {\n    let mut v: Vec<i32> = Vec::new();\n    v.push(*&id(3));\n    let x = &id(3);\n    let _ = (*x, v.len());\n}\n"), vec![]);
}

/// A borrow that must outlive a region of a local's type is held by that local (as NLL: live while the local is):
/// `fn insert(&'a mut self)` in `impl<'a> Foo<'a>`, an argument a `&mut self` method stores
#[test]
fn borrows_held_through_the_regions_of_a_type() {
    let d = check("struct Foo<'a> {\n    v: Vec<&'a str>,\n}\nimpl<'a> Foo<'a> {\n    fn insert(&'a mut self) {}\n}\nfn main() {\n    let mut foo = Foo { v: Vec::new() };\n    foo.insert();\n    foo.insert();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 10, "{d:?}");
    let d = check("struct S<'a> {\n    r: Option<&'a mut u8>,\n}\nimpl<'a> S<'a> {\n    fn set(&mut self, x: &'a mut u8) {\n        self.r = Some(x);\n    }\n    fn get(&self) -> u8 {\n        0\n    }\n}\nfn main() {\n    let mut y = 1;\n    let mut s = S { r: None };\n    s.set(&mut y);\n    y += 1;\n    let _ = s.get();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 16, "{d:?}");
    assert_eq!(check("struct S<'a> {\n    r: Option<&'a mut u8>,\n}\nimpl<'a> S<'a> {\n    fn set(&mut self, x: &'a mut u8) {\n        self.r = Some(x);\n    }\n    fn get(&self) -> u8 {\n        0\n    }\n}\nfn main() {\n    let mut y = 1;\n    let mut s = S { r: None };\n    s.set(&mut y);\n    let _ = s.get();\n    let _ = s.get();\n    y += 1;\n}\n"), vec![]);
}

/// E0261 in a body's `let` annotation, and in a trait's where clause
#[test]
fn undeclared_lifetimes_in_bodies_and_traits() {
    let d = check("fn main() {\n    let x: &'a u8 = &1;\n    let _ = x;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0261"), "{d:?}");
    let d = check("trait Tr<T>\nwhere\n    T: 'b,\n{\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0261"), "{d:?}");
    assert_eq!(check("struct S<'a>(&'a u8);\nimpl<'a> S<'a> {\n    fn f<'b>(&self, x: &'b u8) {\n        let _y: &'a u8 = self.0;\n        let _z: &'b u8 = x;\n        let _w: &'static str = \"\";\n    }\n}\ntrait Tr<'a, T: 'a> where T: 'static {}\nfn main() {}\n"), vec![]);
}

/// A closure made where nothing was expected has one region for its parameters' references in all its calls:
/// `|y| y` called with a local's borrow, then with a parameter returned, returns the local's
#[test]
fn closure_signature_regions_shared_by_its_calls() {
    let d = check("fn foo(x: &i32) -> &i32 {\n    let z = 4;\n    let f = |y| y;\n    let _k = f(&z);\n    f(x)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0515"), "{d:?}");
    assert_eq!(check("fn foo(x: &i32) -> &i32 {\n    let f = |y| y;\n    let _k = f(x);\n    f(x)\n}\nfn bar(x: &i32) -> &i32 {\n    let z = 4;\n    let f = |y: &i32| y;\n    let _k = f(&z);\n    f(x)\n}\nfn main() {}\n"), vec![]);
}

/// A temporary a `match` binds by `ref` goes away at the end of the statement; a `let`'s lives as long as the block
#[test]
fn scrutinee_temporaries_bound_by_ref() {
    let d = check("fn main() {\n    let msg;\n    match Some(\"Hello\".to_string()) {\n        Some(ref m) => {\n            msg = m;\n        }\n        None => panic!(),\n    }\n    println!(\"{}\", *msg);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check("fn main() {\n    let ref x = String::new();\n    match Some(String::new()) {\n        Some(ref m) => println!(\"{} {}\", m, x),\n        None => {}\n    }\n}\n"), vec![]);
}

/// A temporary a `let` extends (`let x = &f();`) goes away with the block's locals
#[test]
fn extended_temporaries_end_with_their_block() {
    let d = check("fn id<T>(x: T) -> T {\n    x\n}\nfn main() {\n    let r;\n    {\n        let x = &id(3);\n        r = x;\n    }\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check("fn id<T>(x: T) -> T {\n    x\n}\nfn main() {\n    let r;\n    let x = &id(3);\n    {\n        r = x;\n    }\n    println!(\"{} {}\", r, x);\n}\n"), vec![]);
}

/// A field, a type alias or an associated type naming a struct with lifetimes writes them (E0106), as many as it
/// has (E0107); a const's or a static's are 'static
#[test]
fn hidden_lifetimes_in_items() {
    let d = check("struct Bar<'a, T>(&'a T);\ntype B = Bar<u8>;\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0106"), "{d:?}");
    let d = check("struct Buzz<'a, 'b>(&'a str, &'b str);\nstruct Baz<'a> {\n    buzz: Buzz<'a>,\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0107"), "{d:?}");
    assert_eq!(check("struct Foo<'a> {\n    b: &'a [bool],\n}\nstatic F: Foo = Foo { b: &[true] };\nstruct W<'a> {\n    f: Foo<'a>,\n    g: fn(Foo) -> u8,\n}\nfn h(_f: Foo) {}\nfn main() {\n    let _ = &F;\n}\n"), vec![]);
}

/// E0195: a lifetime a type parameter's bound names is early-bound, in the trait as in the impl
#[test]
fn early_bound_through_type_parameter_bounds() {
    let d = check("trait Test<'p> {}\ntrait NoLifetime {\n    fn get<'p, T: Test<'p>>(&self) -> u8;\n}\nstruct Foo;\nimpl NoLifetime for Foo {\n    fn get<'p, T>(&self) -> u8 {\n        0\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0195"), "{d:?}");
    assert_eq!(check("trait Test<'p> {}\ntrait NoLifetime {\n    fn get<'p, T: Test<'p>>(&self) -> u8;\n}\nstruct Foo;\nimpl NoLifetime for Foo {\n    fn get<'p, T: Test<'p>>(&self) -> u8 {\n        0\n    }\n}\nfn main() {}\n"), vec![]);
}

/// A call through `dyn Fn(&'static u8)` passes arguments of the regions its bound writes; a static's regions are
/// 'static when it is assigned
#[test]
fn dyn_fn_calls_and_static_assignments() {
    let d = check("static FOO: &'static (dyn Fn(&'static u8) + Send + Sync) = &drop;\nfn main() {\n    let n = 42;\n    FOO(&n);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    let d = check("static FOO: u8 = 42;\nstatic mut BAR: &'static u8 = &FOO;\nfn set() {\n    let n = 42;\n    unsafe {\n        BAR = &n;\n    }\n}\nfn main() {\n    set();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    assert_eq!(check("static FOO: u8 = 42;\nstatic mut BAR: &'static u8 = &FOO;\nfn take(f: &dyn Fn(&u8), n: &u8) {\n    f(n);\n}\nfn main() {\n    unsafe {\n        BAR = &FOO;\n    }\n    take(&|_| {}, &1);\n}\n"), vec![]);
}

/// A pattern's path that writes lifetimes is a type annotation: the value matched outlives them
#[test]
fn pattern_paths_with_lifetimes() {
    let d = check("enum Foo<'a> {\n    Bar { field: &'a u32 },\n}\nfn main() {\n    let y = 22;\n    let foo = Foo::Bar { field: &y };\n    match foo {\n        Foo::Bar::<'static> { field: _z } => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("enum Foo<'a> {\n    Bar(&'a u32),\n}\nfn main() {\n    let y = 22;\n    let foo = Foo::Bar(&y);\n    match foo {\n        Foo::Bar(_z) => {}\n    }\n    static Z: u32 = 1;\n    let Foo::Bar::<'static>(_w) = Foo::Bar(&Z);\n}\n"), vec![]);
}

/// A call through a fn pointer passes its arguments into the pointer's parameters' regions; a fn item with the
/// type arguments written is a value of those regions
#[test]
fn fn_pointer_calls_and_fn_item_values() {
    let d = check("fn take<'a>(f: fn(&'a i32), x: &i32) {\n    f(x);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2, "{d:?}");
    let d = check("fn foo<'a>() {\n    let x = 0;\n    let f = &drop::<&'a i32>;\n    f(&x);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("fn take<'a>(f: fn(&'a i32), x: &'a i32) {\n    f(x);\n}\nfn id<T>(t: T) -> T {\n    t\n}\nfn g<'a>(a: &'a ()) {\n    let x = id::<&'a ()>;\n    let _: &'a () = x(a);\n}\nfn main() {}\n"), vec![]);
}

/// A returned iterator adapter holds the closure passed to it, and with it what the closure borrows (E0373)
#[test]
fn returned_adapter_holds_its_closure() {
    let d = check("struct L {\n    line: u32,\n}\nimpl L {\n    fn lines<'a>(self, s: &'a str) -> impl Iterator<Item = &'a str> {\n        s.split('\\n').enumerate().filter_map(move |(i, l)| if self.line as usize <= i { Some(l) } else { None })\n    }\n    fn lines2<'a>(self, s: &'a str) -> impl Iterator<Item = &'a str> {\n        s.split('\\n').enumerate().filter_map(|(i, l)| if self.line as usize <= i { Some(l) } else { None })\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("E0373"), "{d:?}");
    assert_eq!(check("struct B {\n    v: Vec<u32>,\n}\nimpl B {\n    fn find(&mut self, id: u32) -> Option<&mut u32> {\n        self.v.iter_mut().find(|a| **a == id)\n    }\n}\nfn main() {}\n"), vec![]);
}

/// `vec![&f()]`: a macro's arguments are not extended, the temporary goes away with the statement (not inside
/// `assert_eq!`, whose temporaries live as long as its statement)
#[test]
fn vec_macro_temporaries() {
    let d = check("fn id<T>(x: T) -> T {\n    x\n}\nfn main() {\n    let v = vec![&id(3)];\n    for &&x in &v {\n        let _ = x;\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check("fn one() -> i32 {\n    1\n}\nfn main() {\n    assert_eq!(vec![&one(), &2], vec![&1, &(one() + one())]);\n    let v = vec![&3];\n    let _ = v.len();\n}\n"), vec![]);
}

/// The hidden type of an `impl Trait` result of a fn with no region in its signature captures none: its regions
/// are 'static
#[test]
fn opaque_result_without_lifetimes_is_static() {
    let d = check("pub trait T {}\nstruct S<'a>(&'a ());\nimpl<'a> T for S<'a> {}\nfn foo() -> impl T {\n    let x = ();\n    S(&x)\n}\nfn main() {\n    let _ = foo();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    assert_eq!(check("use std::fmt::Display;\npub trait T {}\nstruct S<'a>(&'a ());\nimpl<'a> T for S<'a> {}\nfn foo() -> impl T {\n    S(&())\n}\nfn bar() -> impl Display {\n    \"x\"\n}\nfn main() {\n    let _ = (foo(), bar());\n}\n"), vec![]);
}

/// `Self(..)` in an impl of an instance that writes 'static (`impl A<&'static u8>`) builds that type
#[test]
fn self_constructor_of_a_static_instance() {
    let d = check("struct A<T>(T);\nimpl A<&'static u8> {\n    fn f() {\n        let x = 0;\n        Self(&x);\n    }\n}\nfn main() {\n    A::f();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
    assert_eq!(check("struct A<T>(T);\nimpl A<&'static u8> {\n    fn f() {\n        static X: u8 = 0;\n        Self(&X);\n        let y = 0;\n        let _ = A(&y);\n    }\n}\nfn main() {\n    A::f();\n}\n"), vec![]);
}

/// A call requires its parameters' types well formed (`&'a Foo<'b>`: 'b: 'a), and a local holds through the regions
/// of its type a borrow made before it is bound
#[test]
fn parameter_well_formedness_at_calls() {
    let d = check("struct Foo<'b>(Option<&'b ()>);\nimpl<'b> Foo<'b> {\n    fn xmute<'a>(&'a self, u: &'b u32) -> &'a u32 {\n        u\n    }\n}\nfn main() {\n    let f2 = Foo(None);\n    let dangling = {\n        let pointer = Box::new(42);\n        f2.xmute(&pointer)\n    };\n    println!(\"{}\", dangling);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("struct Foo<'b>(Option<&'b ()>);\nimpl<'b> Foo<'b> {\n    fn xmute<'a>(&'a self, u: &'b u32) -> &'a u32 {\n        u\n    }\n}\nfn main() {\n    let f2 = Foo(None);\n    let pointer = Box::new(42);\n    let ok = f2.xmute(&pointer);\n    println!(\"{}\", ok);\n}\n"), vec![]);
}

/// A closure made where nothing was expected that stores its parameter in what it captures: the function it is in
/// sees the store, and its calls' arguments flow there
#[test]
fn closure_stores_its_parameter_in_a_capture() {
    let d = check("fn extend<'a, 'b>(x: &mut (&'a str,), y: &'b str) {\n    let mut closure = |input| x.0 = input;\n    closure(y);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2, "{d:?}");
    let d = check("fn main() {\n    let mut v: Vec<&u8> = Vec::new();\n    {\n        let mut add = |s| v.push(s);\n        let x = 1;\n        add(&x);\n    }\n    let _ = v.len();\n}\n");
    assert!(d.len() == 1 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("fn extend<'a>(x: &mut (&'a str,), y: &'a str) {\n    let mut closure = |input| x.0 = input;\n    closure(y);\n}\nfn main() {}\n"), vec![]);
}

/// A destructuring `let` of `&f()` extends the temporary to its block: a binding into it returned is E0515
#[test]
fn destructuring_let_of_a_temporary() {
    let d = check("fn id<T>(x: T) -> T {\n    x\n}\nstruct Foo(isize);\nfn foo<'a>() -> &'a isize {\n    let &Foo(ref x) = &id(Foo(3));\n    x\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0515"), "{d:?}");
    assert_eq!(check("fn id<T>(x: T) -> T {\n    x\n}\nstruct Foo(isize);\nfn main() {\n    let &Foo(ref x) = &id(Foo(3));\n    let _ = *x;\n}\n"), vec![]);
}

/// The receiver's borrow comes before the arguments: a shared one (`v.get({ v.push(2); 0 })`) and a two-phase
/// borrow's reservation (`v.push(v.pop().unwrap())`) last while an argument that names the receiver is evaluated, and
/// a place written through `IndexMut` (`v[v.len() - 1] = 1` of a Vec) is borrowed before its index
#[test]
fn receiver_borrowed_before_the_arguments() {
    let d = check("fn main() {\n    let mut v = vec![0, 1];\n    v.get({\n        v.push(2);\n        0\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("cannot borrow `v` as mutable because it is also borrowed as immutable"), "{d:?}");
    let d = check("fn main() {\n    let mut v = vec![1, 2];\n    v.push(v.pop().unwrap());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("more than once at a time"), "{d:?}");
    let d = check("fn main() {\n    let mut v = vec![0u32; 4];\n    v[v.len() - 1] = 123;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(
        check("fn main() {\n    let mut v = vec![1usize, 2];\n    v.push(v.len());\n    let x = v.get(v.len() - 1).copied();\n    let n = v.len();\n    v[n - 1] = 3;\n    v[0] = v.len();\n    let mut a = [1, 2, 3];\n    a[a.len() - 1] = 4;\n    let mut vv = vec![vec![1]];\n    vv[0][0] = vv.len() as i32;\n    println!(\"{:?} {:?} {:?}\", x, a, vv);\n}\n"),
        vec![]
    );
}

/// A closure that calls a `&mut self` method of what it captures (`|| v.push(4)`), or writes it through an index
/// (`|| v[1] = 4`), captures it by `&mut`
#[test]
fn closure_capture_by_mut_method() {
    let d = check("fn main() {\n    let mut v: Vec<isize> = vec![1, 2, 3];\n    let r = &v[0];\n    let mut c = || {\n        v.push(4);\n    };\n    c();\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("cannot borrow `v` as mutable"), "{d:?}");
    let d = check("fn g<F: FnOnce()>(_a: &isize, f: F) {\n    f();\n}\nfn main() {\n    let mut v: Vec<isize> = vec![1, 2, 3];\n    g(&v[0], || {\n        v[1] = 4;\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    assert_eq!(
        check("fn main() {\n    let mut v: Vec<isize> = vec![1, 2, 3];\n    let mut c = || {\n        v.push(4);\n        v[0] = 2;\n    };\n    c();\n    let r = &v;\n    let n = || r.len();\n    println!(\"{:?} {}\", r, n());\n}\n"),
        vec![]
    );
}

/// A `&mut` local passed where a `&mut` is expected is reborrowed: one that must outlive a lifetime of the signature
/// lives to the function's end, so passing it again in a loop is E0499. `&mut dyn Tr` passed on to `&mut dyn Tr` may
/// shorten the object's lifetime (an unsizing coercion), and is no such reborrow
#[test]
fn implicit_reborrow_of_a_mut_argument() {
    let d = check("struct W<'a> {\n    func: fn(&'a mut u8),\n}\nimpl<'a> W<'a> {\n    fn in_loop(self, arg: &'a mut u8) {\n        loop {\n            (self.func)(arg)\n        }\n    }\n}\nfn main() {\n    let mut a = 1;\n    let w = W { func: |_| {} };\n    w.in_loop(&mut a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("more than once"), "{d:?}");
    let d = check("fn keep<'a>(v: &mut Vec<&'a mut u8>, x: &'a mut u8) {\n    for _ in 0..2 {\n        push(v, x);\n    }\n}\nfn push<'a>(v: &mut Vec<&'a mut u8>, x: &'a mut u8) {\n    v.push(x);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(
        check("fn bump(x: &mut u8) {\n    *x += 1;\n}\nfn ok(x: &mut u8) {\n    for _ in 0..2 {\n        bump(x);\n    }\n    *x += 1;\n}\nfn each(v: &[i32], f: &mut dyn FnMut(&i32)) {\n    for x in v {\n        f(x);\n    }\n}\nfn twice<'a>(v: &'a [i32], f: &mut dyn FnMut(&i32)) -> &'a i32 {\n    each(v, f);\n    each(v, f);\n    &v[0]\n}\nfn main() {\n    let mut b = 2;\n    ok(&mut b);\n    let mut n = 0;\n    twice(&[1], &mut |_| n += 1);\n}\n"),
        vec![]
    );
}

/// `ref` bindings of a value not in memory: `let ref y = a;` borrows a (E0502 below), `let ref x = 0;` binds a
/// temporary, and `let ref mut y = x;` writes x itself
#[test]
fn ref_bindings_of_values() {
    let d = check("fn bar(_x: &mut i32) {}\nfn foo(a: &mut i32) {\n    let ref y = a;\n    bar(a);\n    let v = **y;\n    let _ = v;\n}\nfn main() {\n    let mut n = 1;\n    foo(&mut n);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    let d = check("fn main() {\n    let ref my_ref @ _ = 0;\n    *my_ref = 0;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("behind a `&` reference"), "{d:?}");
    let d = check("fn main() {\n    let var = 10i32;\n    let ref string: String = var;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("mismatched types"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut x = 5;\n    let ref mut y = x;\n    *y += 1;\n    let ref z = 7;\n    match x {\n        ref mut w => *w += 1,\n    }\n    assert_eq!(x + *z, 14);\n}\n"), vec![]);
}

/// Code rustc accepts that the review of #39 found rejected: one lifetime in two inputs of a method with no
/// receiver, a getter through `&mut &'long S` borrowing for 'long, a lint in an allow list after another, a lifetime
/// named in a string literal, a receiver of an impl for a reference, an impl's type parameter named like a struct
#[test]
fn accepted_review_39() {
    assert_eq!(check("struct S;\nimpl S {\n    fn pick<'a>(x: &'a u8, _y: &'a u8) -> &u8 {\n        x\n    }\n}\nfn main() {\n    let a = 1;\n    let b = 2;\n    println!(\"{}\", S::pick(&a, &b));\n}\n"), vec![]);
    assert_eq!(check("struct S {\n    v: u8,\n}\nimpl S {\n    fn get(&self) -> &u8 {\n        &self.v\n    }\n}\nfn f<'short, 'long>(x: &'short mut &'long S) -> &'long u8 {\n    x.get()\n}\nfn main() {\n    let s = S { v: 1 };\n    let mut r = &s;\n    println!(\"{}\", f(&mut r));\n}\n"), vec![]);
    assert_eq!(check("#![deny(unused_lifetimes)]\n#[allow(dead_code, unused_lifetimes)]\nfn f<'a>() {}\nfn main() {\n    f();\n}\n"), vec![]);
    assert_eq!(check("#![deny(single_use_lifetimes)]\nfn f<'a>() {\n    let _ = \"'a\";\n}\nfn main() {\n    f();\n}\n"), vec![]);
    assert_eq!(check("struct View<'a>(&'a u8);\ntrait Read {\n    fn rd(self) -> u8;\n}\nimpl<'a> Read for &'a View<'static> {\n    fn rd(self: &'a View<'static>) -> u8 {\n        *self.0\n    }\n}\nstatic X: u8 = 1;\nfn main() {\n    let v = View(&X);\n    println!(\"{}\", (&v).rd());\n}\n"), vec![]);
    assert_eq!(check("struct View<'a>(&'a u8);\nstruct Holder<T>(T);\ntrait Tr {\n    type Out;\n}\nimpl<View> Tr for Holder<View> {\n    type Out = View;\n}\nfn main() {\n    let _h: Holder<u8> = Holder(1);\n}\n"), vec![]);
    assert_eq!(check("fn pick<'a>(x: &'a u8, _y: &'a u8) -> &u8 {\n    x\n}\nfn main() {\n    let a = 1;\n    println!(\"{}\", pick(&a, &a));\n}\n"), vec![]);
    // (a `//` in a string is no comment: the lifetime's use after it is seen)
    assert_eq!(check("#![deny(unused_lifetimes)]\nfn f<'a>() {\n    let _ = \"https://example\";\n    let _: Option<&'a u8> = None;\n}\nfn main() {\n    f();\n}\n"), vec![]);
    // (still E0106: two lifetimes, and none)
    let d = check("struct S;\nimpl S {\n    fn pick<'a, 'b>(x: &'a u8, _y: &'b u8) -> &u8 {\n        x\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].1.contains("E0106"), "{d:?}");
}

/// zustc check checks a generic fn nothing calls, with a placeholder type for each type parameter: its borrow and
/// region errors are reported; what a placeholder cannot do (a bound's method, a coercion to `dyn`) leaves the fn
/// unchecked, nothing reported
#[test]
fn uncalled_generic_fns() {
    let d = check("struct W<'a, T: 'a> {\n    func: fn(&'a mut T),\n}\nimpl<'a, T: 'a> W<'a, T> {\n    fn in_loop(self, arg: &'a mut T) {\n        loop {\n            (self.func)(arg)\n        }\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("more than once"), "{d:?}");
    let d = check("fn baz<'a, 'b, T>(x: &mut Vec<&'a T>, y: &T) {\n    x.push(y);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (an error of the borrow check's kind lowering returns is reported in a placeholder instance too)
    let d = check("use std::collections::HashMap;\nfn put<T>(m: &HashMap<u8, u8>, _t: T) {\n    m[&1] = 2;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0594"), "{d:?}");
    // (a generic fn's call of a concrete impl's method whose trait declares 'static too is followed)
    let d = check("struct S;\ntrait Tr {\n    fn take(x: &'static u8);\n}\nimpl Tr for S {\n    fn take(_: &'static u8) {}\n}\nfn f<T>(x: &u8) {\n    S::take(x);\n}\nfn main() {\n    let n = 0u8;\n    f::<u8>(&n);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9, "{d:?}");
    // (accepted: a bound's method, a Copy bound, a recursive call through a closure, a trait's impl with 'static)
    assert_eq!(
        check("trait Tr {\n    fn get(&self) -> u8;\n}\nfn a<T: Tr>(x: &T) -> u8 {\n    x.get()\n}\nfn b<T: Copy>(x: T) -> (T, T) {\n    (x, x)\n}\nfn c<F: FnMut(u8)>(n: u8, mut f: F) {\n    if n > 0 {\n        c(n - 1, |x| f(x))\n    }\n}\nfn d<'a, 'b, T>(_: &'a &'b (), v: &'b T, _: &()) -> &'a T {\n    v\n}\nfn main() {}\n"),
        vec![]
    );
}

/// A placeholder's region is a type parameter's: universal, outliving what its `T: 'a` bounds name. One that must
/// outlive another region is "the parameter type `T` may not live long enough" (E0309; E0310 for 'static)
#[test]
fn type_parameter_regions() {
    let d = check("use std::cell::Cell;\nfn outlives<'a, T>(_x: Cell<&'a usize>, _y: T)\nwhere\n    T: 'a,\n{\n}\nfn region_static<'a, T>(cell: Cell<&'a usize>, t: T) {\n    outlives(cell, t)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("the parameter type `T` may not live long enough") && d[0].1.contains("E0309"), "{d:?}");
    assert_eq!(check("use std::cell::Cell;\nfn outlives<'a, T>(_x: Cell<&'a usize>, _y: T)\nwhere\n    T: 'a,\n{\n}\nfn bounded<'a, T: 'a>(cell: Cell<&'a usize>, t: T) {\n    outlives(cell, t)\n}\nfn main() {}\n"), vec![]);
}

/// What a bound's trait requires of its `Self` holds of a placeholder (`trait Tr: 'static`, `trait Q<'a>: 'a`, through
/// supertraits); a raw pointer to a placeholder cast to one to a trait object is an unsizing coercion (review of #42)
#[test]
fn placeholder_trait_outlives() {
    assert_eq!(check("trait Tr: 'static {}\nfn boxed<T: Tr>(v: T) -> Box<dyn Tr + 'static> {\n    Box::new(v)\n}\ntrait Q<'a>: 'a {}\nfn q<'a, T: Q<'a>>(v: T) -> Box<dyn Q<'a> + 'a> {\n    Box::new(v)\n}\ntrait Base: 'static {}\ntrait Sub: Base {}\nfn s<T: Sub>(v: T) -> Box<dyn Sub + 'static> {\n    Box::new(v)\n}\nfn main() {}\n"), vec![]);
    let d = check("trait Tr {}\nfn p<T: Tr>(v: *const T) -> *const (dyn Tr + 'static) {\n    v as *const (dyn Tr + 'static)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0310"), "{d:?}");
    assert_eq!(check("trait Tr {}\nfn p<T: Tr + 'static>(v: *const T) -> *const (dyn Tr + 'static) {\n    v as *const (dyn Tr + 'static)\n}\nfn main() {}\n"), vec![]);
}

/// A placeholder's region is the same wherever its type is: a type argument written (`f::<T>(x)`), an annotation
/// (`let _: &'static T`), not only a value's type. E0311 when the region it must outlive has no name
#[test]
fn placeholder_type_arguments() {
    let w = "fn with_restriction<'b, T: 'b>(x: &'b ()) -> &'b () {\n    x\n}\n";
    let d = check(&format!("fn no_restriction<T>(x: &()) -> &() {{\n    with_restriction::<T>(x)\n}}\n{w}fn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0311"), "{d:?}");
    let d = check("fn test<T>(_: &u8) {\n    let _: &'static T;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0310"), "{d:?}");
    assert_eq!(check(&format!("fn bounded<'b, T: 'b>(x: &'b ()) -> &'b () {{\n    with_restriction::<T>(x)\n}}\nfn local<T>(x: &()) {{\n    with_restriction::<T>(x);\n}}\n{w}fn main() {{}}\n")), vec![]);
}

/// A placeholder bounded by a trait: coerced to `dyn Tr + 'static` (E0310), cast to it; a fn whose body calls the
/// bound's methods is not checked (`dyn Tr + '_` in a parameter is an elided lifetime, not the default)
#[test]
fn placeholder_bounds() {
    let d = check("trait X {\n    fn foo(&self) {}\n}\nfn p1<T>(v: T) -> Box<dyn X + 'static>\nwhere\n    T: X,\n{\n    Box::new(v)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("E0310"), "{d:?}");
    let d = check("trait S {\n    fn get(&self) -> isize;\n}\nfn m<A: S>(v: A) -> Box<dyn S + 'static> {\n    Box::new(v) as Box<dyn S + 'static>\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0310"), "{d:?}");
    assert_eq!(
        check("trait X {\n    fn foo(&self) {}\n}\nfn p1<'a, T: X + 'a>(v: T) -> Box<dyn X + 'a> {\n    Box::new(v)\n}\ntrait G {\n    fn get(&self) -> usize;\n}\nstruct H<'a> {\n    c: &'a usize,\n}\nimpl<'a> G for H<'a> {\n    fn get(&self) -> usize {\n        *self.c\n    }\n}\nfn get_v(gc: Box<dyn G + '_>) -> usize {\n    gc.get()\n}\nfn main() {\n    let c = 22;\n    let h = H { c: &c };\n    println!(\"{}\", get_v(Box::new(h) as Box<dyn G>));\n}\n"),
        vec![]
    );
}

/// The borrow check's mutability: a `ref` binding is not `mut` (E0596 for `&mut x` of it), an immutable static is not
/// assigned (E0594), a slice by range is a loan of the elements (E0596 behind `&`, and a conflict as `&x[i]` is)
#[test]
fn mutability_of_bindings_statics_slices() {
    let d = check("fn main() {\n    let op = Some(2);\n    match op {\n        Some(ref v) => {\n            let _a = &mut v;\n        }\n        None => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("not declared as mutable"), "{d:?}");
    // (two or more such borrows: one error, at the declaration, as rustc reports them)
    let d = check("fn main() {\n    let v = Vec::new();\n    v.push(0);\n    v.push(1);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("not declared as mutable"), "{d:?}");
    let d = check("fn main() {\n    let v: Vec<u8> = Vec::new();\n    v.push(0);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    let d = check("static FOO: isize = 5;\nfn main() {\n    FOO = 6;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("immutable static item `FOO`"), "{d:?}");
    let d = check("fn main() {\n    let x: &[isize] = &[1, 2, 3];\n    let _ = &mut x[0..2];\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("behind a `&` reference"), "{d:?}");
    let d = check("fn main() {\n    let mut v = vec![1, 2, 3];\n    let a = &mut v[0..2];\n    let b = &v[1..];\n    a[0] = 1;\n    println!(\"{:?}\", b);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    assert_eq!(
        check("static mut N: i32 = 0;\nstatic K: i32 = 1;\nfn main() {\n    let op = Some(2);\n    if let Some(ref mut v) = Some(op) {\n        *v = None;\n    }\n    unsafe {\n        N = K;\n    }\n    let mut v = vec![1, 2, 3];\n    let a = &mut v[0..2];\n    a[0] = 1;\n    let b = &v[1..];\n    println!(\"{:?}\", b);\n}\n"),
        vec![]
    );
}

/// A closure that mutates what it captures is `FnMut`: a call of it by name borrows it mutably (E0596 when the binding
/// is not `mut`); one that holds a `&mut` of a capture is not `Copy` (E0382 when moved twice). One that holds a value
/// that is not `Copy` may be `FnOnce`, called by value
#[test]
fn fnmut_closures() {
    let d = check("fn main() {\n    let mut my_var = false;\n    let callback = || {\n        my_var = true;\n    };\n    callback();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("not declared as mutable"), "{d:?}");
    let d = check("fn main() {\n    let mut a = 5;\n    let hello = || {\n        a += 1;\n    };\n    let _b = hello;\n    let _c = hello;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("use of moved value: `hello`"), "{d:?}");
    assert_eq!(
        check("fn main() {\n    let mut a = 5;\n    let mut inc = || {\n        a += 1;\n    };\n    inc();\n    inc();\n    let b = 1;\n    let read = || b + 1;\n    let r1 = read;\n    let r2 = read;\n    let mut foo = Some(Box::new(8));\n    let f = move || {\n        if let Some(x) = foo {\n            foo = Some(x);\n        }\n        println!(\"{:?}\", foo.unwrap());\n    };\n    f();\n    println!(\"{} {}\", r1(), r2());\n}\n"),
        vec![]
    );
}

/// An array matched by value: each element is a place of its own (`let [_, _, x] = a;` moves a[2], not the others)
#[test]
fn array_pattern_moves() {
    let arr = "fn array() -> [(String, String); 3] {\n    Default::default()\n}\n";
    let d = check(&format!("{arr}fn f() {{\n    let a = array();\n    let [_, _, _x] = a;\n    let [.., _y] = a;\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("use of moved value: `a[2]`"), "{d:?}");
    let d = check(&format!("{arr}fn f() {{\n    let a = array();\n    let [_x, _, _] = a;\n    let [_y @ .., _, _] = a;\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
    assert_eq!(check(&format!("{arr}fn f() {{\n    let a = array();\n    let [_x, _, _] = a;\n    let [.., _y] = a;\n    let [_, (_, _z), _] = a;\n}}\nfn main() {{}}\n")), vec![]);
}

/// `for x in v` of a Vec / an array by value takes it (E0382 after); `&mut x;` in a closure is a `&mut` capture (FnMut);
/// a closure whose body moves a capture out is FnOnce, and a call of it by name takes it (E0382 when called again)
#[test]
fn for_moves_and_fnonce() {
    let d = check("fn main() {\n    let v = vec![String::new()];\n    for l in v {\n        let _ = l;\n    }\n    let _w = v;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("use of moved value: `v`"), "{d:?}");
    let d = check("fn main() {\n    let mut my_var = false;\n    let callback = || {\n        &mut my_var;\n    };\n    callback();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("not declared as mutable"), "{d:?}");
    let d = check("fn main() {\n    let counter: Vec<i32> = Vec::new();\n    let tick = || std::mem::drop(counter);\n    tick();\n    tick();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("use of moved value: `tick`"), "{d:?}");
    assert_eq!(
        check("fn main() {\n    let mut w = vec![1];\n    for x in w.iter() {\n        let _ = x;\n    }\n    w.push(1);\n    for x in &w {\n        let _ = x;\n    }\n    w.push(2);\n    let a = [1, 2];\n    for x in a {\n        let _ = x;\n    }\n    let _b = a;\n    let s = String::from(\"x\");\n    let show = move || println!(\"{}\", s);\n    show();\n    show();\n}\n"),
        vec![]
    );
}

/// A closure passed where a bound wants `Fn` may not mutate what it captures: E0594 for an assignment, E0596 for a
/// `&mut` or a `&mut self` method (in a closure inside it too)
#[test]
fn fn_bound_closures() {
    let d = check("pub fn bar<F: Fn()>(_f: F) {}\npub fn foo() {\n    let mut x = 0;\n    bar(move || x = 1);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("captured variable in a `Fn` closure"), "{d:?}");
    let d = check("fn call_it<F>(f: F) where F: Fn() {\n    f();\n}\nstruct A;\nimpl A {\n    fn gen_mut(&mut self) {}\n}\nfn main() {\n    let mut x = A;\n    call_it(|| {\n        call_it(|| x.gen_mut());\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("cannot borrow `x` as mutable"), "{d:?}");
    assert_eq!(check("pub fn bar<F: FnMut()>(mut f: F) {\n    f();\n}\npub fn baz<F: Fn() -> i32>(f: F) -> i32 {\n    f()\n}\nfn main() {\n    let mut x = 0;\n    bar(|| x += 1);\n    let y = 2;\n    baz(|| y + 1);\n}\n"), vec![]);
}

/// Review of #43: an index reads an array pattern's moved element; a move closure that only mutates a capture that is
/// not `Copy` is `FnMut`; a qualified immutable static; a placeholder of another type parameter keeps its bound's
/// lifetimes; a named closure passed where `Fn` is wanted (E0525); `for x in v.iter().copied()` does not take v
#[test]
fn review_of_43() {
    let d = check("fn main() {\n    let a = [String::new(), String::new(), String::new()];\n    let [_, _, _s] = a;\n    let _t = &a[2];\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    let d = check("fn main() {\n    let mut s = String::new();\n    let append = move || s.push_str(\"x\");\n    append();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("not declared as mutable"), "{d:?}");
    let d = check("mod m {\n    pub static N: i32 = 0;\n}\nfn main() {\n    m::N = 1;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("immutable static item `m::N`"), "{d:?}");
    let d = check("trait Get<'a> {\n    fn get(&self) -> &'a u8;\n    fn bad<'b, G: Get<'b>>(&self, g: G) -> &'a u8 {\n        g.get()\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`'b` must outlive `'a`"), "{d:?}");
    let d = check("fn wants_fn<F: Fn()>(_f: F) {}\nfn main() {\n    let mut x = 0;\n    let c = || x += 1;\n    wants_fn(c);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0525"), "{d:?}");
    assert_eq!(
        check("fn wants_mut<F: FnMut()>(mut f: F) {\n    f();\n}\nfn main() {\n    let v = vec![1, 2];\n    for x in v.iter().copied() {\n        let _ = x;\n    }\n    let _n = v.len();\n    let mut x = 0;\n    let c = || x += 1;\n    wants_mut(c);\n    let mut foo = Some(Box::new(8));\n    let f = move || {\n        if let Some(b) = foo {\n            foo = Some(b);\n        }\n        println!(\"{:?}\", foo.unwrap());\n    };\n    f();\n}\n"),
        vec![]
    );
}

/// Review of #43 (2): an `FnMut` closure called through a `&` (E0596) or a `&mut` (accepted); `a[1] = x` does not
/// reinitialize a[0] moved by a pattern; `for x in v.iter()` borrows v for the loop
#[test]
fn review_of_43_2() {
    let d = check("fn main() {\n    let mut x = 0;\n    let c = || x += 1;\n    let r = &c;\n    r();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("behind a `&` reference"), "{d:?}");
    let d = check("fn main() {\n    let mut a = [String::new(), String::new()];\n    let [_x, _y] = a;\n    a[1] = String::new();\n    let _z = &a[0];\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("moved"), "{d:?}");
    let d = check("fn main() {\n    let mut v = vec![1];\n    for x in v.iter() {\n        v.push(*x);\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("also borrowed as immutable"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut x = 0;\n    let mut c = || x += 1;\n    let r = &mut c;\n    r();\n    r();\n    let mut v = vec![1];\n    for x in v.iter_mut() {\n        *x += 1;\n    }\n    v.push(2);\n}\n"), vec![]);
}

/// Review of #43 (3): an `FnMut` closure called through `&&mut` is behind a `&` (E0596), through `&mut &mut` it is not;
/// a long `rest @ ..` moves its elements as one place (a[1..19]), which a[0] is not
#[test]
fn review_of_43_3() {
    let d = check("fn main() {\n    let mut x = 0;\n    let mut c = || x += 1;\n    let m = &mut c;\n    let r = &m;\n    r();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("behind a `&` reference"), "{d:?}");
    let d = check("fn main() {\n    let a: [String; 20] = Default::default();\n    let [_f, _rest @ ..] = a;\n    let _g = &a[5];\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("moved"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut x = 0;\n    let mut c = || x += 1;\n    let mut m = &mut c;\n    let r = &mut m;\n    r();\n    let a: [String; 20] = Default::default();\n    let [_f, _rest @ .., _l] = a;\n}\n"), vec![]);
}

/// A builtin method's result (`v.iter()` of `&[T]`): a placeholder's region in it is its own, not the receiver's
/// (`v.iter().count()` in a generic fn nothing calls was "`'_` must outlive `T#`")
#[test]
fn builtin_results_of_placeholders() {
    assert_eq!(
        check("fn d<T>(v: &[T]) -> usize {\n    v.iter().filter(|_| true).count()\n}\nfn e<T>(v: &[T]) -> Option<&T> {\n    v.iter().next()\n}\nfn f<T>(v: &[T]) -> usize {\n    v.iter().map(|_| 1).count()\n}\nfn main() {}\n"),
        vec![]
    );
}

/// A constant condition is not folded for the borrow check (`false || { i = 5; true }` may leave i uninitialized,
/// E0381); a chain of one operator is one control flow, so `true && { z = 3; true } && z == 3` reads z only after it is
/// set
#[test]
fn constant_conditions_and_chains() {
    let d = check("fn main() {\n    let i: isize;\n    println!(\"{}\", false || { i = 5; true });\n    println!(\"{}\", i);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("isn't initialized"), "{d:?}");
    assert_eq!(check("fn main() {\n    let z;\n    if true && { z = 3; true } && z == 3 {}\n    let y;\n    if false || { y = 3; false } || y == 3 {}\n}\n"), vec![]);
}

/// A receiver a call made, borrowed by a `&self` method whose result borrows it, in a `let`'s initializer: a temporary
/// freed at the end of the statement (E0716 when the result is used after). Not when the result does not borrow it
#[test]
fn receiver_temporaries() {
    let a = "struct A {\n    v: Vec<u8>,\n}\nimpl A {\n    fn get(&self) -> &A {\n        self\n    }\n    fn len(&self) -> usize {\n        self.v.len()\n    }\n    fn name(&self) -> &'static str {\n        \"a\"\n    }\n}\nfn mk() -> A {\n    A { v: vec![1] }\n}\n";
    let d = check(&format!("{a}fn main() {{\n    let x = mk().get();\n    println!(\"{{}}\", x.len());\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 19 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check(&format!("{a}fn main() {{\n    let n = mk().len();\n    let s = mk().name();\n    let k = mk().get().len();\n    println!(\"{{}} {{}} {{}}\", n, s, k);\n}}\n")), vec![]);
}

/// A type alias written without its lifetimes in a signature (`b: A` with `type A<'a> = &'a isize`): its regions are
/// elided, a fresh one each, as `&isize`'s
#[test]
fn alias_with_elided_lifetimes() {
    let d = check("type A<'a> = &'a isize;\nstruct C<'a> {\n    f: A<'a>,\n}\nimpl<'a> C<'a> {\n    fn set_bad(&mut self, b: A) {\n        self.f = b;\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("type A<'a> = &'a isize;\nstruct C<'a> {\n    f: A<'a>,\n}\nimpl<'a> C<'a> {\n    fn set_ok(&mut self, b: A<'a>) {\n        self.f = b;\n    }\n    fn get(&self) -> A {\n        self.f\n    }\n}\nfn main() {}\n"), vec![]);
}

/// A method of the prelude's types whose result holds references holds what its receiver holds: returning
/// `it.next()` or `raw.iter().map(..).collect()` of a local's iterator is E0515, where the chain starts
#[test]
fn prelude_results_hold_receivers() {
    let d = check("fn d<'a>() -> Option<&'a String> {\n    let raw: Vec<String> = Vec::new();\n    let mut it = raw.iter();\n    it.next()\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("cannot return a reference to a local variable"), "{d:?}");
    let d = check("fn r<'a>() -> Vec<&'a str> {\n    let raw: Vec<String> = vec![String::new()];\n    raw\n        .iter()\n        .map(|l| l.trim())\n        .collect()\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("fn ok(s: &str) -> Option<&str> {\n    let parts: Vec<&str> = s.split(',').collect();\n    parts.first().copied()\n}\nfn n() -> usize {\n    let raw: Vec<String> = Vec::new();\n    raw.iter().count()\n}\nfn main() {}\n"), vec![]);
}

/// E0195 for the methods of an impl of a type that is not a struct (`impl<'a> Foo<'a> for &'a isize`): the method's
/// early-bound lifetimes are the trait's
#[test]
fn e0195_for_impls_of_references() {
    let d = check("pub trait Foo<'a> {\n    fn no_bound<'b>(self, b: &'b u8);\n}\nimpl<'a> Foo<'a> for &'a isize {\n    fn no_bound<'b: 'a>(self, b: &'b u8) {}\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].1.contains("E0195"), "{d:?}");
    assert_eq!(check("pub trait Foo<'a> {\n    fn no_bound<'b>(self, b: &'b u8);\n    fn has_bound<'b: 'a>(self, b: &'b u8);\n}\nimpl<'a> Foo<'a> for &'a isize {\n    fn no_bound<'b>(self, b: &'b u8) {}\n    fn has_bound<'b: 'a>(self, b: &'b u8) {}\n}\nfn main() {}\n"), vec![]);
}

/// A capture held by value (a `move` closure's) of a local not declared `mut`: assigning it is E0594, at the
/// assignment; assigning through a `&mut` it holds (`t.0.0 = x`) or reading it is not
#[test]
fn move_closure_assigns_immutable_capture() {
    let d = check("fn main() {\n    let x = 0;\n    (move || {\n        x = 1;\n    })()\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0594"), "{d:?}");
    assert_eq!(check("struct S(String);\nstruct T<'a>(&'a mut S);\nfn main() {\n    let mut s = S(String::new());\n    let t = T(&mut s);\n    let mut c = move || {\n        t.0.0 = String::from(\"new\");\n    };\n    c();\n    let y = 1;\n    let g = move || y + 1;\n    g();\n    let mut z = 0;\n    let mut h = move || z += 1;\n    h();\n}\n"), vec![]);
}

/// A type parameter bounded by `Fn` / `FnMut` / `FnOnce` in a generic fn nothing calls: its calls are checked against
/// the bound (`FnMut` borrows it mutably: E0596 when not `mut`; `FnOnce` takes it: E0382 when called twice; `Fn`
/// reads it: E0502 under a `&mut` of it)
#[test]
fn placeholder_fn_calls() {
    let d = check("fn b<F: FnMut(isize, isize) -> isize>(f: F) {\n    f(1, 2);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("not declared as mutable"), "{d:?}");
    let d = check("fn c<F: FnOnce(isize, isize) -> isize>(f: F) {\n    f(1, 2);\n    f(1, 2);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("use of moved value: `f`"), "{d:?}");
    let d = check("fn a<F: Fn(isize, isize) -> isize>(mut f: F) {\n    let g = &mut f;\n    f(1, 2);\n    use_mut(g);\n}\nfn use_mut<T>(_: &mut T) {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("fn a<F: Fn(isize, isize) -> isize>(f: F) -> isize {\n    f(1, 2) + f(3, 4)\n}\nfn b<F: FnMut(isize) -> isize>(mut f: F) -> isize {\n    f(1) + f(2)\n}\nfn c<F: FnOnce() -> String>(f: F) -> String {\n    f()\n}\nfn d<T, F: Fn(&T) -> bool>(v: &[T], f: F) -> usize {\n    v.iter().filter(|x| f(x)).count()\n}\nfn main() {}\n"), vec![]);
}

/// `&*Box::new(1)` borrows what a temporary owns: stored in a map that outlives the statement, E0716; a `let` extends
/// it (`let r = &*Box::new(1);`)
#[test]
fn borrow_of_a_temporarys_contents() {
    let d = check("use std::collections::HashMap;\nfn main() {\n    let mut m: HashMap<usize, &usize> = HashMap::new();\n    m.insert(42, &*Box::new(1));\n    println!(\"{:?}\", m.len());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check("fn main() {\n    let r: &usize = &*Box::new(1);\n    println!(\"{}\", r);\n}\n"), vec![]);
}

/// Review follow-ups of part 7: what rustc accepts (an alias that fixes `'static`, `map` with a closure that does not
/// use its item, a shadowed capture, a receiver a call returns as a reference, a method named `__ref_m` itself) and
/// what it rejects (E0195 for lifetimes early-bound in one and late-bound in the other, E0276 for a stricter bound,
/// `FnMut` through `&F`)
#[test]
fn part7_review_follow_ups() {
    assert_eq!(check("type Static = &'static u8;\nfn keep(x: Static) -> &'static u8 {\n    x\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("fn ok() -> Vec<&'static str> {\n    let raw = vec![String::new()];\n    raw.iter().map(|_| \"ok\").collect()\n}\nfn main() {}\n"), vec![]);
    let d = check("fn bad() -> Vec<&'static str> {\n    let raw = vec![String::new()];\n    raw.iter().map(|s| s.as_str()).collect()\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("fn main() {\n    let x = 0;\n    let mut c = move || {\n        let mut x = x;\n        x += 1;\n        println!(\"{x}\");\n    };\n    c();\n}\n"), vec![]);
    assert_eq!(check("struct A {\n    s: String,\n}\nimpl A {\n    fn get(&self) -> &String {\n        &self.s\n    }\n}\nfn id(a: &A) -> &A {\n    a\n}\nfn main() {\n    let a = A { s: String::new() };\n    let x = id(&a).get();\n    println!(\"{}\", x.len());\n}\n"), vec![]);
    assert_eq!(check("trait Tr {\n    fn foo<'a: 'b, 'b>(x: &'a u8, y: &'b u8);\n    fn __ref_foo();\n}\nimpl Tr for i32 {\n    fn foo<'a: 'b, 'b>(x: &'a u8, y: &'b u8) {}\n    fn __ref_foo() {}\n}\nfn main() {}\n"), vec![]);
    // (the impl's bounds less strict than the trait's: accepted)
    assert_eq!(check("trait Tr {\n    fn f<'a: 'b, 'b: 'a>(x: &'a u8, y: &'b u8);\n}\nimpl Tr for i32 {\n    fn f<'a: 'b, 'b>(x: &'a u8, y: &'b u8) {}\n}\nfn main() {}\n"), vec![]);
    let d = check("trait Tr {\n    fn f<'a, 'b: 'static>(x: &'a u8, y: &'b u8);\n}\nimpl Tr for i32 {\n    fn f<'a: 'static, 'b>(x: &'a u8, y: &'b u8) {}\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0195"), "{d:?}");
    let d = check("trait Tr {\n    fn f<'a: 'b, 'b>(x: &'a u8, y: &'b u8);\n}\nimpl Tr for i32 {\n    fn f<'a, 'b: 'a>(x: &'a u8, y: &'b u8) {}\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0276"), "{d:?}");
    let d = check("fn bad<F: FnMut()>(f: &F) {\n    f();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("behind a `&` reference"), "{d:?}");
}

/// What is stored through a `&mut` a local holds (`*q = &y` with `q = &mut p`) is held by what it points to: p used
/// after y is gone is E0597. Not when p is not used after, nor through a `&mut` of a local that outlives y
#[test]
fn store_through_mut_ref() {
    let d = check("fn deref(_: &i32) {}\nfn main() {\n    let x = 44;\n    let mut p = &x;\n    {\n        let y = 22;\n        let q = &mut p;\n        *q = &y;\n    }\n    deref(p);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("fn deref(_: &i32) {}\nfn main() {\n    let x = 44;\n    let mut p = &x;\n    {\n        let y = 22;\n        let q = &mut p;\n        *q = &y;\n        deref(p);\n    }\n    let z = 1;\n    p = &z;\n    deref(p);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let y = 22;\n    let x = 44;\n    let mut p = &x;\n    let q = &mut p;\n    *q = &y;\n    println!(\"{}\", p);\n}\n"), vec![]);
}

/// A `&mut` local passed for a type parameter not known yet (`fn g<T>(x: T)`) is moved, not reborrowed (E0382 when
/// used after); one passed where the parameter is a `&mut` is reborrowed
#[test]
fn mut_ref_moved_into_type_parameter() {
    let d = check("struct X(u32);\nimpl X {\n    fn f(&mut self) {\n        generic(self);\n        self.0 += 1;\n    }\n}\nfn generic<T>(_x: T) {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("moved"), "{d:?}");
    assert_eq!(check("struct X(u32);\nimpl X {\n    fn f(&mut self) {\n        by_ref(self);\n        self.0 += 1;\n    }\n}\nfn by_ref(_x: &mut X) {}\nfn main() {}\n"), vec![]);
}

/// Two fields of a union are the same memory: `&mut u.s.a` and `&u.z.c` conflict (E0502); fields of the same field
/// do not
#[test]
fn union_fields_overlap() {
    let src = |second: &str| format!("#[derive(Clone, Copy)]\nstruct S {{\n    a: u8,\n    b: u8,\n}}\n#[derive(Clone, Copy)]\nstruct Z {{\n    c: u8,\n    d: u8,\n}}\nunion U {{\n    s: S,\n    z: Z,\n}}\nfn main() {{\n    unsafe {{\n        let mut u = U {{ s: S {{ a: 1, b: 2 }} }};\n        let m = &mut u.s.a;\n        let n = &{second};\n        println!(\"{{}} {{}}\", m, n);\n    }}\n}}\n");
    let d = check(&src("u.z.c"));
    assert!(d.len() == 1 && d[0].0 == 19, "{d:?}");
    assert_eq!(check(&src("u.s.b")), vec![]);
}

/// Review follow-ups of part 8: a write to one union field while another is borrowed is reported (it panicked); a store
/// through a field's `&mut` is not followed into the local's other loans; E0509 for a projected scrutinee and nested
/// patterns; a `&mut` field moved into a type parameter; a static moved out by a module path (E0507)
#[test]
fn part8_review_follow_ups() {
    // (a write to `u.b.x` while `u.a` is borrowed: the same memory)
    let d = check("#[derive(Clone, Copy)]\nstruct P { x: u32, y: u32 }\nunion U { a: u32, b: P }\nfn main() {\n    let mut u = U { a: 1 };\n    let r = unsafe { &u.a };\n    unsafe { u.b.x = 2; }\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("borrowed"), "{d:?}");
    // (a union's field written: the union is, its field moved out before too)
    assert_eq!(check("struct A;\nstruct B;\nunion U {\n    a: std::mem::ManuallyDrop<A>,\n    b: std::mem::ManuallyDrop<B>,\n}\nfn main() {\n    unsafe {\n        let mut u = U { a: std::mem::ManuallyDrop::new(A) };\n        let _a = u.a;\n        u.b = std::mem::ManuallyDrop::new(B);\n        let _a = u.a;\n    }\n}\n"), vec![]);
    // (`*q.0 = &y` with q = (&mut p, &mut z): z is not changed)
    assert_eq!(check("fn sink(_: &u32) {}\nfn main() {\n    let x = 0;\n    let mut p = &x;\n    let mut z = &x;\n    {\n        let y = 1;\n        let q = (&mut p, &mut z);\n        *q.0 = &y;\n        sink(p);\n    }\n    sink(z);\n}\n"), vec![]);
    let drop_d = "struct D { f: Option<String> }\nimpl Drop for D { fn drop(&mut self) {} }\n";
    // (`match d.f { Some(x) => .. }`: x is moved out of d)
    let d = check(&format!("{drop_d}fn main() {{\n    let d = D {{ f: Some(String::new()) }};\n    match d.f {{ Some(x) => drop(x), None => {{}} }}\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0509"), "{d:?}");
    assert_eq!(check(&format!("{drop_d}fn main() {{\n    let d = D {{ f: Some(String::new()) }};\n    match d.f {{ Some(ref x) => println!(\"{{}}\", x), None => {{}} }}\n}}\n")), vec![]);
    // (`let D { f: Some(x) } = d`: deeper than d's own field)
    let d = check(&format!("{drop_d}fn main() {{\n    let d = D {{ f: Some(String::new()) }};\n    let D {{ f: Some(x) }} = d else {{ return }};\n    drop(x);\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0509"), "{d:?}");
    // (a `&mut` field passed for a type parameter is moved)
    let d = check("fn consume<T>(_: T) {}\nfn main() {\n    let mut a = 1;\n    let s = (&mut a,);\n    consume(s.0);\n    *s.0 = 2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("moved"), "{d:?}");
    // (a static by its module's path)
    let d = check("mod m {\n    pub static BAR: String = String::new();\n}\nfn take(_: String) {}\nfn main() {\n    take(m::BAR);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0507"), "{d:?}");
    assert_eq!(check("mod m {\n    pub static BAR: String = String::new();\n}\nfn main() {\n    println!(\"{}\", m::BAR.len());\n}\n"), vec![]);
    // (`*q = &mut y` with q = &mut p, p = &mut *z: p changes, not z, which q holds a loan of through p)
    assert_eq!(check("fn main() {\n    let mut z0 = 0;\n    let z = &mut z0;\n    let mut w = 5;\n    let mut p = &mut w;\n    p = &mut *z;\n    {\n        let mut y = 1;\n        let q = &mut p;\n        *q = &mut y;\n        **q += 1;\n    }\n    *z += 1;\n}\n"), vec![]);
    // (nor what z points to when z is a parameter: `*q = &mut y` does not store y behind it)
    assert_eq!(check("fn f(z: &mut i32) {\n    let mut p = &mut *z;\n    {\n        let mut y = 1;\n        let q = &mut p;\n        *q = &mut y;\n        **q += 1;\n    }\n    *z += 1;\n}\nfn main() {}\n"), vec![]);
    let d = check("fn g(z: &mut &i32) {\n    let y = 1;\n    let q = &mut *z;\n    *q = &y;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    // (a reborrow `q = &mut *p` points to what p points to: `*q = &y` stores into r)
    let d = check("fn sink(_: &i32) {}\nfn main() {\n    let x = 1;\n    let mut r = &x;\n    {\n        let y = 2;\n        let p = &mut r;\n        let q = &mut *p;\n        *q = &y;\n    }\n    sink(r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9, "{d:?}");
    // (but `*q = &mut &y`: the `&mut` is invariant, so z's region is y's)
    let d = check("fn sink(_: &u32) {}\nfn main() {\n    let z0 = 0;\n    let mut z = &z0;\n    let mut p = &mut z;\n    {\n        let y = 1;\n        let q = &mut p;\n        *q = &mut &y;\n    }\n    sink(z);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9, "{d:?}");
    // (E0509 after a `..`, of an array's element, of a field through a box, of a `&mut` field into a type parameter)
    let d = check("struct D(i32, String);\nimpl Drop for D {\n    fn drop(&mut self) {}\n}\nfn main() {\n    let d = D(1, String::new());\n    let D(.., s) = d;\n    drop(s);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0509"), "{d:?}");
    let d = check("struct D {\n    f: [String; 1],\n}\nimpl Drop for D {\n    fn drop(&mut self) {}\n}\nfn main() {\n    let d = D { f: [String::new()] };\n    let D { f: [s] } = d;\n    drop(s);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("E0509"), "{d:?}");
    let d = check("struct D {\n    f: String,\n}\nimpl Drop for D {\n    fn drop(&mut self) {}\n}\nfn main() {\n    let b = Box::new(D { f: String::new() });\n    let s = b.f;\n    drop(s);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("E0509"), "{d:?}");
    let d = check("struct D<'a> {\n    r: &'a mut i32,\n}\nimpl<'a> Drop for D<'a> {\n    fn drop(&mut self) {}\n}\nfn consume<T>(_: T) {}\nfn main() {\n    let mut x = 1;\n    let d = D { r: &mut x };\n    consume(d.r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("E0509"), "{d:?}");
    // (a `ref` binding and a `Copy` part are fine)
    assert_eq!(check("struct D(i32, String);\nimpl Drop for D {\n    fn drop(&mut self) {}\n}\nfn main() {\n    let d = D(1, String::new());\n    let D(n, ref s) = d;\n    let D(.., ref t) = d;\n    println!(\"{} {} {}\", n, s, t);\n}\n"), vec![]);
}

/// E0582: a lifetime a `for<'a>` binder declares that only the bound's result or an associated type's binding names
#[test]
fn e0582_unconstrained_binder() {
    let d = check("fn bar<F>(_t: F)\nwhere\n    F: for<'a> Fn() -> Option<&'a i32>,\n{\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0582"), "{d:?}");
    let d = check("fn baz<F>(_t: F)\nwhere\n    F: for<'a> Iterator<Item = &'a i32>,\n{\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0582"), "{d:?}");
    assert_eq!(check("fn ok<F, T, U>(_t: F)\nwhere\n    F: for<'a> Fn(&'a u8) -> &'a u8,\n    for<'a> &'a T: IntoIterator<Item = &'a U>,\n{\n}\nfn main() {}\n"), vec![]);
    // (a generic associated type's binding names the binder's lifetime in its own arguments: an input)
    assert_eq!(check("trait S {\n    type Item<'a> where Self: 'a;\n}\nfn f<T>() where T: for<'a> S<Item<'a> = &'a i32> {}\nfn main() {}\n"), vec![]);
}

/// A move out of a static (E0507); a constant is a new value at each use
#[test]
fn move_out_of_static() {
    let d = check("struct Foo {\n    foo: String,\n}\nstatic BAR: Foo = Foo { foo: String::new() };\nfn test(_f: Foo) {}\nfn main() {\n    test(BAR);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0507"), "{d:?}");
    assert_eq!(check("struct Foo {\n    foo: u8,\n}\nconst BAR: Foo = Foo { foo: 1 };\nfn test(_f: Foo) {}\nfn main() {\n    test(BAR);\n    println!(\"{}\", BAR.foo);\n}\n"), vec![]);
}

/// A tuple / array / struct literal of a runtime value (`(v,)` of a local) is a temporary, not promoted to a static as
/// `&(1,)` is: borrowed past the statement it is E0716. A `let` extends it
#[test]
fn runtime_literal_temporaries() {
    // (a block, an `if`, a `match` in a literal: the value it ends with; constant ones are promoted)
    let first = "fn first(x: &(i32,)) -> &i32 {\n    &x.0\n}\n";
    assert_eq!(check(&format!("{first}fn main() {{\n    let r = first(&({{ 1 }},));\n    let k = first(&(if true {{ 1 }} else {{ 2 }},));\n    println!(\"{{}} {{}}\", r, k);\n}}\n")), vec![]);
    let d = check(&format!("{first}fn main() {{\n    let c = true;\n    let q = first(&(if c {{ 1 }} else {{ 2 }},));\n    println!(\"{{}}\", q);\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0716"), "{d:?}");
    // (a struct update reads its base)
    let d = check("struct S {\n    a: i32,\n    b: i32,\n}\nfn get(s: &S) -> &i32 {\n    &s.b\n}\nfn main() {\n    let s = S { a: 1, b: 2 };\n    let r = get(&S { a: 3, ..s });\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("E0716"), "{d:?}");
    let d = check("fn gimme(x: &(u32,)) -> &u32 {\n    &x.0\n}\nfn main() {\n    let v = 22;\n    let x = gimme(&(v,));\n    println!(\"{:?}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check("fn gimme(x: &(u32,)) -> &u32 {\n    &x.0\n}\nfn main() {\n    let v = 22;\n    let a = gimme(&(1,));\n    let r = &(v,);\n    let b = gimme(r);\n    println!(\"{} {} {}\", a, b, gimme(&(v,)));\n}\n"), vec![]);
}

/// A block passed as an argument passes what its tail borrows: its temporaries are the call's (E0716). `&{ .. }` is a
/// temporary. `let _ = z;` keeps z live (rustc's fake read)
#[test]
fn block_tail_temporaries() {
    let gimme = "fn gimme(x: &(u32,)) -> &u32 {\n    &x.0\n}\n";
    let d = check(&format!("{gimme}fn main() {{\n    let x = gimme({{ let v = 22; &(v,) }});\n    println!(\"{{:?}}\", x);\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0716"), "{d:?}");
    let d = check(&format!("{gimme}fn main() {{\n    let z = gimme(&{{ let v = 4; (v,) }});\n    let _ = z;\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0716"), "{d:?}");
    let d = check("fn mk() -> String {\n    String::from(\"a\")\n}\nfn main() {\n    let x: &str = { mk().as_str() };\n    println!(\"{}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0716"), "{d:?}");
    // (the `let` extends a block tail's borrow to its own block; a value used within the statement is fine)
    assert_eq!(check(&format!("fn mk() -> String {{\n    String::from(\"a\")\n}}\n{gimme}fn main() {{\n    let n = {{ mk().as_str() }}.len();\n    let s = {{ let v = 1; &(v,) }};\n    let q = *gimme({{ let v = 5; &(v,) }});\n    println!(\"{{}} {{:?}} {{}}\", n, s, q);\n}}\n")), vec![]);
    // (a fake read keeps x live: the write to y while it is borrowed conflicts)
    let d = check("fn main() {\n    let mut y = 1;\n    let r = &y;\n    y = 2;\n    let _ = r;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    // (but it is no use of a moved value)
    assert_eq!(check("fn main() {\n    let x = String::new();\n    drop(x);\n    let _ = x;\n}\n"), vec![]);
}

/// A trait's parameters are invariant: a callee's bound `G: Get<T>` met by the caller's `G: Get<&'max i32>` makes T's
/// region 'max
#[test]
fn trait_bound_regions_invariant() {
    let tr = "trait Get<T> {\n    fn get(&self) -> T;\n}\nfn impls_get<G: Get<T>, T>() {}\n";
    let d = check(&format!("{tr}fn f<'min, 'max: 'min, G: Get<&'max i32>>() {{\n    impls_get::<G, &'min i32>()\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check(&format!("{tr}fn f<'a, G: Get<&'a i32>>() {{\n    impls_get::<G, &'a i32>()\n}}\nfn g<'a, 'b, G: Get<&'a i32>>() where 'a: 'b, 'b: 'a {{\n    impls_get::<G, &'b i32>()\n}}\nfn main() {{}}\n")), vec![]);
}

/// E0510: a match guard must not assign or mutably borrow the place the match tests (or one it is part of, or what it
/// points to); a part of it is fine, as is a match that tests nothing
#[test]
fn match_guard_mutation() {
    let d = check("fn main() {\n    let mut n = 3;\n    match n {\n        0 => {}\n        _ if { n += 1; true } => {}\n        _ => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0510"), "{d:?}");
    let d = check("fn main() {\n    let mut x = &mut Some(&2);\n    match x {\n        &mut None => {}\n        &mut Some(&_) if { (|| { *x = None; })(); false } => {}\n        _ => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0510"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut s = (1, Some(2));\n    match s {\n        (_, Some(_)) if { s.0 = 5; true } => {}\n        _ => {}\n    }\n    let mut v = vec![1];\n    match v.len() {\n        0 if { v.push(2); true } => {}\n        _ => {}\n    }\n    println!(\"{:?} {:?}\", s, v);\n}\n"), vec![]);
}

/// E0713: a borrow through a `&mut` field of a value whose type implements `Drop`, returned: the value's drop may use
/// the field when the function returns
#[test]
fn borrow_in_use_when_destructor_runs() {
    let s = "struct Scribble<'a>(&'a mut u32);\nimpl<'a> Drop for Scribble<'a> {\n    fn drop(&mut self) {\n        *self.0 = 42;\n    }\n}\n";
    let d = check(&format!("{s}fn f<'a>(s: Scribble<'a>) -> &'a mut u32 {{\n    &mut *s.0\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("E0713"), "{d:?}");
    let d = check(&format!("{s}fn f<'a>(s: Box<Scribble<'a>>) -> &'a u32 {{\n    let r: &'a u32 = &*(*s).0;\n    &*r\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("E0713"), "{d:?}");
    // (not through a `&mut Scribble`, nor of a type without `Drop`, nor a borrow not returned)
    assert_eq!(check(&format!("{s}struct Plain<'a>(&'a mut u32);\nfn f<'a>(s: &'a mut Scribble) -> &'a mut u32 {{\n    &mut *s.0\n}}\nfn g<'a>(s: Plain<'a>) -> &'a mut u32 {{\n    &mut *s.0\n}}\nfn h(s: Scribble) -> u32 {{\n    let r = &mut *s.0;\n    *r\n}}\nfn main() {{}}\n")), vec![]);
}

/// E0507: a closure written where a bound wants `Fn` / `FnMut` is that, so moving out what it captures by value is an
/// error at the move; `FnOnce` takes it
#[test]
fn closure_moves_capture_out() {
    let d = check("fn take_mut(_f: impl FnMut()) {}\nfn main() {\n    let val = String::new();\n    take_mut(|| {\n        let _s: String = val;\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0507"), "{d:?}");
    let d = check("fn take_fn<F: Fn()>(_f: F) {}\nfn main() {\n    let v = String::new();\n    take_fn(move || {\n        drop(v);\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`Fn` closure"), "{d:?}");
    assert_eq!(check("fn take_once(_f: impl FnOnce()) {}\nfn take_mut(_f: impl FnMut()) {}\nfn main() {\n    let a = String::new();\n    take_once(|| {\n        let _x: String = a;\n    });\n    let c = 5;\n    take_mut(move || {\n        let _z = c;\n    });\n    let e = vec![1];\n    take_mut(move || drop(e.clone()));\n}\n"), vec![]);
}

/// `for n in v` with v: &mut Vec<T> moves v: `IntoIterator::into_iter` takes a type parameter, so no reborrow
#[test]
fn for_loop_moves_mut_ref() {
    let d = check("fn f(v: &mut Vec<i32>) {\n    for n in v {\n        *n += 1;\n    }\n    for n in v {\n        *n += 1;\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("moved"), "{d:?}");
    assert_eq!(check("fn f(v: &mut Vec<i32>) -> usize {\n    for n in v.iter_mut() {\n        *n += 1;\n    }\n    for n in &mut *v {\n        *n += 1;\n    }\n    v.len()\n}\nfn g(v: &Vec<i32>) {\n    for _ in v {}\n    for _ in v {}\n}\nfn main() {}\n"), vec![]);
}

/// E0507: a match guard sees the arm's bindings by shared reference, so a closure in it must not move one out
#[test]
fn guard_closure_moves_binding() {
    let d = check("fn main() {\n    match Some(&4) {\n        None => {}\n        ref mut foo if {\n            (|| { let mut bar = foo; bar.take() })();\n            false\n        } => {}\n        Some(s) => std::process::exit(*s),\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("in pattern guard (E0507)"), "{d:?}");
    assert_eq!(check("fn main() {\n    let v = Some(String::new());\n    match v {\n        Some(ref s) if (|| s.len() > 0)() => {}\n        Some(s) if s.len() > 1 => drop(s),\n        _ => {}\n    }\n}\n"), vec![]);
}

/// A call through a type parameter's `Fn` bound: the arguments flow into the regions the bound's parameters write, as
/// for a `dyn Fn`
#[test]
fn fn_bound_call_regions() {
    let d = check("fn f<'a>(g: impl FnOnce(&'static i32), x: &'a i32) {\n    g(x);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("fn f<'a>(g: impl FnOnce(fn(&'a i32)), x: fn(&'static i32)) {\n    g(x);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2, "{d:?}");
    assert_eq!(check("fn f<'a, 'b: 'a, G: FnOnce(&'a i32)>(g: G, x: &'b i32) {\n    g(x);\n}\nfn h<G: Fn(&i32)>(g: G) {\n    let v = 1;\n    g(&v);\n}\nfn main() {}\n"), vec![]);
}

/// A copy proves its type `Copy`: with `impl Copy for Foo<'static>`, what a copied Foo borrows is 'static
/// (nll/do-not-ignore-lifetime-bounds-in-copy.rs)
#[test]
fn static_copy_impl() {
    let pre = "#[derive(Clone)] struct Foo<'a>(&'a u32);\nimpl Copy for Foo<'static> {}\n";
    let d = check(&format!("{pre}fn main() {{\n    let s = 2;\n    let a = Foo(&s);\n    drop(a);\n    drop(a);\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`s` does not live long enough"), "{d:?}");
    let d = check(&format!("{pre}fn main() {{\n    let s = 2;\n    let a = (Foo(&s),);\n    drop(a.0);\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
    // (a 'static loan, a promoted constant, an impl for every lifetime, a tuple that also holds another reference)
    let ok = "#[derive(Clone)] struct Bar<'a>(&'a u32);\nimpl<'a> Copy for Bar<'a> {}\nstatic N: u32 = 3;\nfn main() {\n    let s = 2;\n    let a = Foo(&N);\n    drop(a);\n    drop(a);\n    let b = Bar(&s);\n    drop(b);\n    drop(b);\n    let c = Foo(&7);\n    let e = (c, &s);\n    let d = e;\n    drop(d);\n}\n";
    assert_eq!(check(&format!("{pre}{ok}")), vec![]);
}

/// A type alias written without its lifetimes that stands for a Box (`Box<B>` with `type B<'a> = Box<&'a isize>`):
/// each of its regions is elided (regions/regions-infer-paramd-indirect.rs)
#[test]
fn alias_of_box_elided() {
    let pre = "type A<'a> = &'a isize;\ntype B<'a> = Box<A<'a>>;\nstruct C<'a> { f: Box<B<'a>> }\n";
    let d = check(&format!("{pre}impl<'a> C<'a> {{\n    fn set_f_bad(&mut self, b: Box<B>) {{\n        self.f = b;\n    }}\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let ok = "impl<'a> C<'a> {\n    fn set_f_ok(&mut self, b: Box<B<'a>>) { self.f = b; }\n    fn get(&self) -> &B { &self.f }\n}\nfn g(b: Box<B>) -> isize { **b + 1 }\nfn main() {}\n";
    assert_eq!(check(&format!("{pre}{ok}")), vec![]);
}

/// An associated const of an impl for its type's 'static instance (`impl A<'static> { const IC }`): `A::<'a>::IC`
/// requires 'a to be 'static (nll/user-annotations/inherent-associated-constants.rs, nll/issue-58299.rs)
#[test]
fn static_self_const() {
    let pre = "struct A<'a>(&'a ());\nimpl A<'static> {\n    const IC: i32 = 10;\n}\n";
    let d = check(&format!("{pre}fn f<'a>(_x: &'a i32) -> i32 {{\n    A::<'a>::IC\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check(&format!("{pre}fn f<'a>(_x: &'a i32) -> i32 {{\n    A::<'static>::IC + A::IC\n}}\nfn main() {{}}\n")), vec![]);

}

/// From the review of part 9 (#51): guards protect the places the patterns test, and the deref prefixes of where
/// bindings come from; `S { .. }` tests nothing (E0510)
#[test]
fn part9_review_guards() {
    let d = check("fn main() {\n    let mut pair = (Some(0), 0);\n    match pair {\n        (None, _) => (),\n        (Some(_), _) if { pair.0 = None; false } => (),\n        (Some(_), _) => (),\n    }\n    let _ = pair;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`pair.0`"), "{d:?}");
    let d = check("fn main() {\n    let mut ptr: &i32 = &1;\n    let x = match *ptr {\n        value if { ptr = &2; true } => value,\n        _ => 0,\n    };\n    let _ = (x, ptr);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0510"), "{d:?}");
    let d = check("struct S { n: u32, m: u32 }\nfn main() {\n    let mut s = S { n: 0, m: 0 };\n    match s {\n        S { n: 0, .. } if { s.m = 1; false } => (),\n        S { n: 1, .. } if { s.n = 3; false } => (),\n        _ => (),\n    }\n    let _ = s.n;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("`s.n`"), "{d:?}");
    assert_eq!(check("struct S { n: u32 }\nfn main() {\n    let mut s = S { n: 0 };\n    match s {\n        S { .. } if { s = S { n: 1 }; false } => (),\n        _ => (),\n    }\n    let mut pair = (Some(0), 0);\n    match pair {\n        (None, _) => (),\n        (Some(_), _) if { pair.1 = 5; false } => (),\n        (Some(_), _) => (),\n    }\n    let _ = (s.n, pair);\n}\n"), vec![]);
}

/// From the review of part 9 (#51): E0713 through a `Drop` value in a field, not past a shared deref; E0507 for a
/// nested `move` closure, not for a name the closure binds again; a receiver reached through `Deref` is borrowed
/// while the arguments are evaluated; a callee's `'static` in a trait bound
#[test]
fn part9_review_follow_ups() {
    let s = "struct Scribble<'a>(&'a mut u32);\nimpl<'a> Drop for Scribble<'a> {\n    fn drop(&mut self) {\n        *self.0 = 42;\n    }\n}\n";
    let d = check(&format!("{s}struct Wrapper<'a>(Scribble<'a>);\nfn f<'a>(w: Wrapper<'a>) -> &'a mut u32 {{\n    &mut *(w.0).0\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("E0713"), "{d:?}");
    assert_eq!(check("struct S<'a> { data: &'a mut &'static u32 }\nimpl<'a> Drop for S<'a> {\n    fn drop(&mut self) {}\n}\nfn f<'a>(s: S<'a>) -> &'static u32 {\n    &**s.data\n}\nfn main() {}\n"), vec![]);
    let d = check("fn take_mut(_: impl FnMut()) {}\nfn main() {\n    let v = String::new();\n    take_mut(|| {\n        let _inner = move || drop(v);\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0507"), "{d:?}");
    assert_eq!(check("fn take_fn(_: impl Fn()) {}\nfn main() {\n    let val = String::new();\n    take_fn(move || {\n        let _len = val.len();\n        let val = String::new();\n        drop(val);\n    });\n}\n"), vec![]);
    // (a move of the capture before a `let` binds its name again is still one; the names an arm, a loop, an `if let`,
    // a nested closure's parameter or a block's `let` bind are not the capture there)
    let d = check("fn take_fn(_: impl Fn()) {}\nfn main() {\n    let val = String::new();\n    take_fn(move || {\n        drop(val);\n        let val = String::new();\n        drop(val);\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0507"), "{d:?}");
    let d = check("fn take_fn(_: impl Fn()) {}\nfn main() {\n    let val = String::new();\n    take_fn(move || {\n        if true {\n            let val = String::new();\n            drop(val);\n        }\n        drop(val);\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("E0507"), "{d:?}");
    assert_eq!(check("fn take_fn(_: impl Fn()) {}\nfn main() {\n    let val = String::new();\n    take_fn(move || match Some(String::new()) {\n        Some(val) => drop(val),\n        None => (),\n    });\n    let val = String::new();\n    take_fn(move || {\n        let f = |val: String| drop(val);\n        f(String::new());\n        let _ = val.len();\n    });\n    let val = String::new();\n    take_fn(move || {\n        if let Some(val) = Some(String::new()) {\n            drop(val)\n        }\n        let _ = val.len();\n    });\n    let val = String::new();\n    take_fn(move || {\n        for val in vec![String::new()] {\n            drop(val);\n        }\n        let _ = val.len();\n    });\n}\n"), vec![]);
    // (a `let` that destructures the capture moves only the parts bound by value that are not Copy)
    assert_eq!(check("struct P {\n    n: u32,\n    s: String,\n}\nfn take_fn(_: impl Fn()) {}\nfn main() {\n    let pair = (1, String::new());\n    take_fn(move || {\n        let (n, _) = pair;\n        let _ = n;\n    });\n    let p = P { n: 1, s: String::new() };\n    take_fn(move || {\n        let P { n, .. } = p;\n        let _ = n;\n    });\n}\n"), vec![]);
    let d = check("fn take_fn(_: impl Fn()) {}\nfn main() {\n    let pair = (1, String::new());\n    take_fn(move || {\n        let (_, s) = pair;\n        drop(s);\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0507"), "{d:?}");
    // (so a named closure that copies out of it and mutates another capture is FnMut only, not maybe FnOnce)
    let d = check("fn take_fn<F: Fn()>(_: F) {}\nfn main() {\n    let pair = (1, String::new());\n    let mut x = 0;\n    let c = move || {\n        let (n, _) = pair;\n        x += n;\n    };\n    take_fn(c);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0525") && d[0].1.contains("only implements `FnMut`"), "{d:?}");
    // (a closure that holds a capture by value holds all of the variable: of one that needs dropping, using a part
    // that is not `Copy` moves all of it, so a part used after is a use of a moved value. Only `Copy` parts used: the
    // variable stays (the closure's copy is emptied of the rest). A variable that needs no dropping: the parts)
    let d = check("fn main() {\n    let q = (String::new(), String::new());\n    let d = move || {\n        let (ref a, _) = q;\n        let _ = a.len();\n    };\n    drop(q.1);\n    d();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("use of moved value"), "{d:?}");
    assert_eq!(check("struct P {\n    n: u32,\n    s: String,\n}\nstruct Q {\n    a: u32,\n    b: u32,\n}\nfn main() {\n    let p = P { n: 1, s: String::new() };\n    let c = move || p.n;\n    drop(p.s);\n    let _ = c();\n    let q = Q { a: 1, b: 2 };\n    let d = move || q.a;\n    let _ = q.b;\n    let _ = d();\n}\n"), vec![]);
    // (a nested closure without `move` whose body moves the capture out takes it by value too; one that borrows it
    // does not)
    let d = check("fn take_mut(_: impl FnMut()) {}\nfn main() {\n    let v = String::new();\n    take_mut(|| {\n        let _inner = || drop(v);\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0507"), "{d:?}");
    assert_eq!(check("fn take_mut(_: impl FnMut()) {}\nfn main() {\n    let v = String::new();\n    take_mut(|| {\n        let inner = || v.len();\n        let _ = inner();\n    });\n}\n"), vec![]);
    let d = check("use std::ops::Deref;\nstruct T;\nimpl T {\n    fn read(&self, _: ()) {}\n}\nstruct W(T);\nimpl Deref for W {\n    type Target = T;\n    fn deref(&self) -> &T {\n        &self.0\n    }\n}\nfn main() {\n    let w = W(T);\n    w.read(drop(w));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 15 && d[0].1.contains("because it is borrowed"), "{d:?}");
    let tr = "trait Get<'a> {\n    fn get(&self) -> &'a i32;\n}\nfn need_static<G: Get<'static>>(_: &G) {}\n";
    let d = check(&format!("{tr}fn f<'a, G: Get<'a>>(g: &G) {{\n    need_static::<G>(g);\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check(&format!("{tr}fn f<G: Get<'static>>(g: &G) {{\n    need_static::<G>(g);\n}}\nfn main() {{}}\n")), vec![]);
    // (an fn in a fn body: E0503 at its own lines, the position the message names too)
    let d = check("fn main() {\n    fn helper() {\n        let mut x = 1;\n        let r = &mut x;\n        let y = x;\n        *r = 2;\n        let _ = y;\n    }\n    helper();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("because it was mutably borrowed (at 4:"), "{d:?}");
    let d = check("struct S {\n    f: u8,\n}\nfn main() {\n    fn helper() {\n        let mut s = S { f: 1 };\n        drop(s);\n        s.f = 2;\n    }\n    helper();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("assign to part of moved value") && d[0].1.contains("(moved at 7:"), "{d:?}");
}

/// `break` / `continue` take the locals of the scopes they leave out of scope (E0597), a `Drop` value's too (E0713,
/// also at a scope's end); `self` is mutable only as `mut self` (E0596)
#[test]
fn scope_exits_and_self() {
    let d = check("fn main() {\n    let r;\n    loop {\n        let x = 1;\n        r = &x;\n        break;\n    }\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let mut r = &0;\n    for i in 0..3 {\n        let x = i;\n        if i == 1 {\n            r = &x;\n            continue;\n        }\n    }\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    let s = "struct Scribble<'a>(&'a mut u32);\nimpl Drop for Scribble<'_> {\n    fn drop(&mut self) {\n        *self.0 = 42;\n    }\n}\n";
    let d = check(&format!("{s}fn main() {{\n    let mut x = 1;\n    let r;\n    {{\n        let s = Scribble(&mut x);\n        r = &mut *s.0;\n    }}\n    *r = 2;\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 12 && d[0].1.contains("E0713"), "{d:?}");
    let d = check("struct S;\nimpl S {\n    fn bar(&mut self) {}\n    fn imm(self) {\n        (&mut self).bar();\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`self` is not declared as mutable"), "{d:?}");
    // (a borrow that ends before the jump, a `Drop` value's borrow that ends in its scope, `mut self`)
    assert_eq!(check(&format!("{s}struct T(u8);\nimpl T {{\n    fn bar(&mut self) {{}}\n    fn a(mut self) -> u8 {{\n        self.bar();\n        self.0 = 1;\n        self.0\n    }}\n    fn b(&mut self) {{\n        self.bar();\n        let r = &mut *self;\n        r.bar();\n    }}\n}}\nfn main() {{\n    let mut v = vec![];\n    for i in 0..3 {{\n        let x = i;\n        let r = &x;\n        if *r == 1 {{\n            break;\n        }}\n        v.push(*r);\n    }}\n    let mut y = 1;\n    {{\n        let s = Scribble(&mut y);\n        let r = &mut *s.0;\n        *r = 3;\n    }}\n    println!(\"{{:?}}\", v);\n}}\n")), vec![]);
}

/// A closure that binds a part of a capture by `ref mut` borrows it mutably: it is `FnMut`, not `Copy`
/// (closures/2229_closure_analysis/diagnostics/closure-origin-*-diagnostics.rs)
#[test]
fn closure_ref_mut_pattern_capture() {
    let d = check("fn main() {\n    let mut p = (1, 2);\n    let c = || {\n        let (ref mut x, _) = p;\n        *x += 1;\n    };\n    let _b = c;\n    let _a = c;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("use of moved value: `c`"), "{d:?}");
    assert_eq!(check("fn main() {\n    let p = (1, 2);\n    let c = || {\n        let (ref x, _) = p;\n        let _ = *x;\n    };\n    let _b = c;\n    let _a = c;\n    let mut q = (1, 2);\n    let mut d = || {\n        let (ref mut x, _) = q;\n        *x += 1;\n    };\n    d();\n    d();\n}\n"), vec![]);
}

/// E0713 through an implicit reborrow: a `&mut` field of a `Drop` value passed where a `&mut` is expected
/// (`v.push(c.raw)` is `v.push(&mut *c.raw)`), the loan stored where it outlives the value, a local or a parameter. The
/// region check is turned on for it in a fn of one region. A reborrow that ends with the call is fine
#[test]
fn drop_field_implicit_reborrow() {
    let d = check("struct C<'a> {\n    raw: &'a mut i32,\n}\nimpl<'a> Drop for C<'a> {\n    fn drop(&mut self) {}\n}\nfn put<'a>(v: &mut Vec<&'a mut i32>, x: &'a mut i32) {\n    v.push(x)\n}\nfn f<'a>(v: &mut Vec<&'a mut i32>, x: &'a mut i32) {\n    let c = C { raw: x };\n    v.push(c.raw);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 12 && d[0].1.contains("E0713"), "{d:?}");
    let d = check("struct C<'a> {\n    raw: &'a mut i32,\n}\nimpl<'a> Drop for C<'a> {\n    fn drop(&mut self) {}\n}\nfn put<'a>(v: &mut Vec<&'a mut i32>, x: &'a mut i32) {\n    v.push(x)\n}\nfn f<'a>(v: &mut Vec<&'a mut i32>, c: C<'a>) {\n    put(v, c.raw);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("E0713"), "{d:?}");
    assert_eq!(check("struct C<'a> {\n    raw: &'a mut i32,\n}\nimpl<'a> Drop for C<'a> {\n    fn drop(&mut self) {}\n}\nfn put<'a>(v: &mut Vec<&'a mut i32>, x: &'a mut i32) {\n    v.push(x)\n}\nfn bump(x: &mut i32) {\n    *x += 1;\n}\nfn f(c: C) {\n    bump(c.raw);\n    bump(c.raw);\n}\nfn g<'a>(v: &mut Vec<C<'a>>, c: C<'a>) {\n    v.push(c)\n}\nfn main() {}\n"), vec![]);
}

/// After a guard that failed, the next arm tests the scrutinee again: what the guard moved out of it is used (E0382,
/// at the match; a binding of all of it, at the arm). A `&mut` captured by value by a `let` in the closure is moved;
/// passed as an argument it may be reborrowed (`drop::<&mut U>(x)` twice is fine)
#[test]
fn guard_moves_and_captured_mut_refs() {
    let d = check("fn main() {\n    let a = Some(String::new());\n    let b = match a {\n        Some(_) if {\n            drop(a);\n            false\n        } => None,\n        x => x,\n    };\n    drop(b);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("use of moved value: `a`"), "{d:?}");
    let d = check("fn main() {\n    let b = &mut true;\n    match b {\n        &mut false => {}\n        _ if {\n            (|| {\n                let bar = b;\n                *bar = false;\n            })();\n            false\n        } => {}\n        &mut true => {}\n        _ => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("use of moved value"), "{d:?}");
    assert_eq!(check("struct U;\nfn main() {\n    let mut u = U;\n    let x = &mut u;\n    let c1 = || drop::<&mut U>(x);\n    c1();\n    let c2 = || drop::<&mut U>(x);\n    c2();\n}\n"), vec![]);
}

/// A struct pattern follows its parts by name (`let S { x: ax, .. } = a;` moves a.x, E0505 while a is borrowed); a
/// pattern of a struct that is no enum does not read the struct itself, and a reference bound in an arm flows out of
/// the match (E0499 for the second `ref mut` of y.0, not for y.1)
#[test]
fn struct_pattern_parts() {
    let d = check("struct S {\n    x: Box<i32>,\n    y: Box<i32>,\n}\nfn g(_: &S) {}\nfn main() {\n    let a = S { x: Box::new(1), y: Box::new(2) };\n    let pb = &a;\n    let S { x: ax, .. } = a;\n    g(pb);\n    drop(ax);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("cannot move out of `a.x` because it is borrowed"), "{d:?}");
    let d = check("struct Y(usize, usize);\nfn main() {\n    let mut y = Y(1, 2);\n    let a = match y {\n        Y(ref mut a, _) => a,\n    };\n    let b = match y {\n        Y(_, ref mut b) => b,\n    };\n    let c = match y {\n        Y(ref mut c, _) => c,\n    };\n    *a += 1;\n    *b += 1;\n    *c += 1;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("more than once"), "{d:?}");
}

/// An assignment drops the old value: a `Drop` that may see what it borrows uses its loans there (E0506). A move out
/// of what an overloaded `Deref` gives (an `Rc`'s field) is E0507; a copy or a borrow of it is fine
#[test]
fn assignment_drop_and_deref_moves() {
    let d = check("struct W<'a>(&'a mut bool);\nimpl<'a> Drop for W<'a> {\n    fn drop(&mut self) {}\n}\nfn main() {\n    let mut base = true;\n    let mut w = W(&mut base);\n    for _ in 0..2 {\n        base = false;\n        w = W(&mut base);\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("cannot assign to `base` because it is borrowed"), "{d:?}");
    let d = check("use std::rc::Rc;\nstruct Bar {\n    field: Vec<i32>,\n    n: u32,\n}\nfn main() {\n    let x = Rc::new(Bar { field: vec![], n: 1 });\n    let n = x.n;\n    let l = x.field.len();\n    let f = x.field;\n    drop((n, l, f));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("cannot move out of an `Rc`"), "{d:?}");
}

/// The fn's tail is a match: what an arm returns (a binding, or a constructor of one: `Some(r)`) is returned while the
/// arm's bindings are in scope, so a `ref mut` binding through a `Drop` value's `&mut` field is E0713
#[test]
fn match_tail_returns_arm_binding() {
    let d = check("enum DropOption<T> {\n    Some(T),\n    None,\n}\nimpl<T> Drop for DropOption<T> {\n    fn drop(&mut self) {}\n}\nfn f(opt: DropOption<&mut i32>) -> Option<&mut i32> {\n    match opt {\n        DropOption::Some(&mut ref mut r) => Some(r),\n        DropOption::None => None,\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("E0713"), "{d:?}");
    assert_eq!(check("fn f(opt: Option<(&mut i32, String)>) -> Option<&mut i32> {\n    match opt {\n        Some((&mut ref mut r, _)) => Some(r),\n        None => None,\n    }\n}\nfn main() {}\n"), vec![]);
}

/// From the review of #51: a `let` that destructures an enum variant of a capture moves only the parts it binds by
/// value whose types are not `Copy`, by the variant's fields (`let E::V(n, _) = e;` with n an integer copies)
#[test]
fn variant_destructuring_moves_by_type() {
    assert_eq!(check("enum E {\n    V(i32, String),\n}\nenum F {\n    W { a: u8, s: String },\n}\nfn take_fn(_: impl Fn()) {}\nfn main() {\n    let e = E::V(1, String::new());\n    take_fn(move || {\n        let E::V(n, _) = e;\n        let _ = n;\n    });\n    let f = F::W { a: 1, s: String::new() };\n    take_fn(move || {\n        let F::W { a, .. } = f;\n        let _ = a;\n    });\n}\n"), vec![]);
    let d = check("enum E {\n    V(i32, String),\n}\nfn take_fn(_: impl Fn()) {}\nfn main() {\n    let e = E::V(1, String::new());\n    take_fn(move || {\n        let E::V(_, s) = e;\n        drop(s);\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("E0507"), "{d:?}");
}

/// The scrutinee of an `if let` or a `match` in a closure for an `Fn` bound comes with the patterns: one that binds a
/// part of a capture by value that is not `Copy` moves it (E0507, at the scrutinee); `ref` and `Copy` parts do not
#[test]
fn closure_match_scrutinee_moves() {
    let d = check("enum E {\n    V(i32, String),\n}\nfn take_fn(_: impl Fn()) {}\nfn main() {\n    let e = E::V(1, String::new());\n    take_fn(move || {\n        if let E::V(_, s) = e {\n            drop(s);\n        }\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("E0507"), "{d:?}");
    let d = check("fn take_fn(_: impl Fn()) {}\nfn main() {\n    let o = Some(String::new());\n    take_fn(move || match o {\n        Some(s) => drop(s),\n        None => {}\n    });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0507"), "{d:?}");
    assert_eq!(check("fn take_fn(_: impl Fn()) {}\nfn main() {\n    let o = Some(String::new());\n    take_fn(move || match o {\n        Some(ref s) => println!(\"{}\", s),\n        None => {}\n    });\n    let p = Some((1, String::new()));\n    take_fn(move || {\n        if let Some((n, _)) = p {\n            let _ = n;\n        }\n    });\n}\n"), vec![]);
}

/// From the review of #53: a `Copy` impl with a `'static` slot and others does not make every copy `'static`; a
/// parenthesized capture moved by a `let` is moved; a block's result that keeps a borrow through a dropped `Drop`
/// value's `&mut` field is E0713; a `ref mut` pattern in a closure borrows only the part it binds; a move through a
/// `&Rc` is E0507; a guard's `let` that binds an arm's name again is not the binding
#[test]
fn part10_review() {
    assert_eq!(check("#[derive(Clone)]\nstruct Foo<'a, 'b>(&'a u32, &'b u32);\nimpl<'b> Copy for Foo<'static, 'b> {}\nstatic S: u32 = 1;\nfn main() {\n    let local = 2;\n    let f = Foo(&S, &local);\n    let g = f;\n    let h = f;\n    println!(\"{} {}\", g.1, h.1);\n}\n"), vec![]);
    let d = check("fn main() {\n    let mut x = 1;\n    let b = &mut x;\n    let c = || { let bar = (b); *bar = 2; };\n    c();\n    *b = 3;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("moved"), "{d:?}");
    let d = check("struct Scribble<'a>(&'a mut u32);\nimpl<'a> Drop for Scribble<'a> { fn drop(&mut self) { *self.0 = 42; } }\nfn main() {\n    let mut x = 1;\n    let r = {\n        let s = Scribble(&mut x);\n        &mut *s.0\n    };\n    *r = 2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0713"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut p = (1u32, 2u32);\n    let r = &p.1;\n    let mut c = || {\n        let (ref mut x, _) = p;\n        *x += p.1;\n    };\n    c();\n    println!(\"{}\", r);\n}\n"), vec![]);
    let d = check("use std::rc::Rc;\nstruct S { value: String, n: u32 }\nfn main() {\n    let rc = Rc::new(S { value: String::new(), n: 1 });\n    let r = &rc;\n    let n = r.n;\n    let v = r.value;\n    drop((n, v));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot move out of an `Rc`"), "{d:?}");
    assert_eq!(check("fn main() {\n    match Some(String::from(\"arm\")) {\n        Some(foo) if {\n            let foo = String::from(\"local\");\n            (|| { let _owned = foo; true })()\n        } => (),\n        _ => (),\n    }\n}\n"), vec![]);
}

/// A block's tail's temporary a method is called on lives until the end of the enclosing statement, after the block's
/// locals: one whose `Drop` may see a local it borrows is E0597 (with `;`, it is dropped first). `a;` in a closure
/// moves a (a closure made in a loop is then E0382). A guard sees a `ref mut` binding as shared (E0596 / E0594)
#[test]
fn tail_temps_stmt_moves_guard_ref_mut() {
    let d = check("#[derive(Debug)]\nstruct D<T: std::fmt::Debug>(T);\nimpl<T: std::fmt::Debug> Drop for D<T> {\n    fn drop(&mut self) {}\n}\nimpl<T: std::fmt::Debug> D<T> {\n    fn end(&self) {}\n}\nfn f() {\n    let t = D(Box::new(1));\n    D(&t).end()\n}\nfn g() {\n    let t = D(Box::new(1));\n    D(&t).end();\n}\nfn main() {\n    f();\n    g();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("does not live long enough"), "{d:?}");
    let d = check("struct NotCopy;\nfn main() {\n    let mut a = NotCopy;\n    loop {\n        || {\n            &mut a;\n            a;\n        };\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("use of moved value: `a`"), "{d:?}");
    assert_eq!(check("fn main() {\n    let n = 5;\n    let s = String::new();\n    for _ in 0..2 {\n        let c = || {\n            n;\n            s.len();\n        };\n        c();\n    }\n}\n"), vec![]);
    let d = check("fn main() {\n    let mut x = Some(1);\n    match x {\n        Some(ref mut r) if {\n            let q = &mut *r;\n            *q == 1\n        } => {}\n        Some(ref mut r) if *r == 1 => {\n            *r = 3;\n        }\n        _ => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0596"), "{d:?}");
}

/// From the second review of #53: `impl Copy for Foo<&'static u32>` (a path written without lifetimes) needs a copy's
/// borrows `'static`; a `ref mut` pattern's part that is not followed is borrowed mutably; a `move` closure in a guard
/// that names an arm's binding moves it (E0507)
#[test]
fn part10_review_2() {
    let d = check("#[derive(Clone)]\nstruct Foo<T>(T);\nimpl Copy for Foo<&'static u32> {}\nfn main() {\n    let local = 2;\n    let f = Foo(&local);\n    let g = f;\n    let h = f;\n    drop((g, h));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("does not live long enough"), "{d:?}");
    let d = check("struct S { value: u32 }\nfn main() {\n    let mut p = (0u32, S { value: 1 });\n    let r = &p.1.value;\n    let mut c = || {\n        let (ref mut x, S { value: ref mut y }) = p;\n        *x += 1;\n        *y += 1;\n    };\n    c();\n    println!(\"{}\", r);\n}\n");
    // (the struct pattern's field is followed (part 32): `p.1.value`, as rustc 1.97 says in edition 2021)
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot borrow `p.1.value` as mutable"), "{d:?}");
    let d = check("fn main() {\n    match Some(String::from(\"arm\")) {\n        Some(foo) if { let c = move || { drop(foo.len()); false }; c() } => (),\n        _ => (),\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0507"), "{d:?}");
}

/// A `&mut` bound by value by a pattern is moved out (no reborrow): in a loop it is E0382 the second time. A receiver
/// written as a borrow (`(&mut x).with(..)`) stays borrowed while the arguments that name x are evaluated (E0499)
#[test]
fn pattern_mut_ref_moves_and_written_receivers() {
    let d = check("fn main() {\n    let mut x = 42_i32;\n    let opt = Some(&mut x);\n    for _ in 0..5 {\n        if let Some(_x) = opt {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("moved value"), "{d:?}");
    assert_eq!(check("struct Node { v: i32, next: Option<Box<Node>> }\nfn main() {\n    let mut x = 1;\n    let mut opt = Some(&mut x);\n    if let Some(r) = opt.as_mut() { **r += 1; }\n    if let Some(ref mut r) = opt { **r += 1; }\n    if let Some(r) = opt { *r += 1; }\n    let mut head = Node { v: 1, next: Some(Box::new(Node { v: 2, next: None })) };\n    let mut cur = &mut head;\n    while let Some(ref mut n) = cur.next { n.v += 1; break; }\n    let mut cur2 = Some(&mut head);\n    while let Some(n) = cur2 { n.v += 1; cur2 = match n.next { Some(ref mut b) => Some(&mut **b), None => None }; }\n    println!(\"{}\", x);\n}\n"), vec![]);
    let d = check("struct X(u32);\nimpl X {\n    pub fn with<F>(&self, blk: F) where F: FnOnce(&u32) { blk(&self.0) }\n    pub fn with_mut<F>(&mut self, blk: F) where F: FnOnce(&u32) { blk(&self.0) }\n}\nfn main() {\n    let mut x = X(1);\n    (&mut x).with(|v| { x = X(*v); });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("more than once"), "{d:?}");
    assert_eq!(check("struct X(u32);\nimpl X { fn get(&self, n: u32) -> u32 { self.0 + n } }\nfn main() {\n    let mut x = X(1);\n    let a = (&x).get(x.0);\n    let b = (&mut x).get(2);\n    let y = &x;\n    let c = (&x).get(y.0);\n    println!(\"{} {} {}\", a, b, c);\n}\n"), vec![]);
}

/// What a returned value borrows is followed through a cast and `Box::new` (`Box::new(B(&*v)) as Box<dyn X>`: E0515)
#[test]
fn returned_borrow_through_cast_and_box() {
    let d = check("trait X { }\nstruct B<'a>(&'a u32);\nimpl<'a> X for B<'a> {}\nfn f(v: Box<u32>) -> Box<dyn X + 'static> {\n    Box::new(B(&*v)) as Box<dyn X>\n}\nfn h(v: Box<u32>) -> Box<dyn X + 'static> {\n    Box::new(B(&*v))\n}\nfn main() {}\n");
    assert!(d.len() == 2 && d[0].0 == 5 && d[1].0 == 8 && d[0].1.contains("cannot return a reference"), "{d:?}");
}

/// `&mut *x` in a closure through a reference it captures is a unique borrow of `*x`: E0596 through a `&`, and two
/// such closures alive at once conflict; one after the other is fine
#[test]
fn closure_mut_through_captured_ref() {
    let d = check("fn set(x: &mut isize) {\n    *x = 4;\n}\nfn a(x: &isize) {\n    let mut c1 = || set(&mut *x);\n    c1();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("behind a `&` reference"), "{d:?}");
    let d = check("fn set(x: &mut isize) {\n    *x = 4;\n}\nfn b(x: &mut isize) {\n    let mut c1 = || set(&mut *x);\n    let mut c2 = || set(&mut *x);\n    c2();\n    c1();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("more than once"), "{d:?}");
    assert_eq!(check("fn set(x: &mut isize) {\n    *x = 4;\n}\nfn ok(x: &mut isize) {\n    let mut c1 = || set(&mut *x);\n    c1();\n    let mut c2 = || set(&mut *x);\n    c2();\n    *x += 1;\n}\nfn main() {}\n"), vec![]);
}

/// From the third review of #53: a reference pattern (`let &mut n = foo;`) matches the referent, so it does not move a
/// guard's `ref mut` binding; a part the capture analysis does not follow is borrowed mutably only if it binds by
/// `ref mut` (`S { value: ref y }` stays shared)
#[test]
fn part10_review_3() {
    assert_eq!(check("fn main() {\n    match Some(7u32) {\n        Some(ref mut foo) if (|| {\n            let &mut n = foo;\n            n == 7\n        })() => {}\n        _ => {}\n    }\n}\n"), vec![]);
    assert_eq!(check("struct S { value: u32 }\nfn main() {\n    let mut p = (0u32, S { value: 1 });\n    let r = &p.1.value;\n    let mut c = || {\n        let (ref mut x, S { value: ref y }) = p;\n        *x += *y;\n    };\n    c();\n    println!(\"{}\", r);\n}\n"), vec![]);
    let d = check("fn main() {\n    match Some(String::new()) {\n        Some(ref mut foo) if (|| { let bar = foo; bar.len() == 0 })() => {}\n        _ => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0507"), "{d:?}");
}

/// From the fourth review of #53: a `&mut` bound by value under a reference pattern (`let &mut bar = foo;` with foo a
/// `&mut &mut u32`) moves it out (E0507); a `Copy` referent (`let &mut n = foo;` with a `&mut u32`) does not
#[test]
fn part10_review_4() {
    let d = check("fn main() {\n    let mut x = 7u32;\n    match Some(&mut x) {\n        Some(ref mut foo) if (|| {\n            let &mut bar = foo;\n            *bar == 7\n        })() => {}\n        _ => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0507"), "{d:?}");
}

/// A capture held by reference is the place the environment points to: the closure's body borrows and assigns it as
/// its own (`let p = &this.x; &mut this.x;` in the body conflicts, borrowck/borrowck-closures-unique-imm.rs); what the
/// body may move or write through it is checked where the closure is made
#[test]
fn closure_body_by_ref_capture() {
    let d = check("struct Foo { x: isize }\nfn main() {\n    let mut this = &mut Foo { x: 1 };\n    let mut r = || {\n        let p = &this.x;\n        let q = &mut this.x;\n        *q += *p;\n    };\n    r();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot borrow `this.x` as mutable because it is also borrowed as immutable"), "{d:?}");
    assert_eq!(check("struct Foo { x: isize, y: isize }\nfn main() {\n    let mut this = Foo { x: 1, y: 2 };\n    let mut r = || {\n        let p = &this.x;\n        let q = &mut this.y;\n        *q += *p;\n    };\n    r();\n    println!(\"{}\", this.y);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let mut v = vec![1];\n    let mut r = || {\n        let p = &mut v;\n        p.push(2);\n        v.push(3);\n    };\n    r();\n    println!(\"{:?}\", v);\n}\n"), vec![]);
}

/// A constructor's path that writes lifetimes gives the value's type (nll/user-annotations/adt-*.rs): a tuple
/// variant's, a struct-like variant's, a unit variant's (`SomeOtherVariant::<Cell<&'static u32>>`, through an
/// invariant `Cell`), a tuple struct's and a struct literal's (which zustc did not accept with a turbofish). A pattern's
/// struct path with a turbofish is resolved (pattern_substs_on_*_struct.rs). A trait's const through a qualified path
/// (`<A<'a> as Tr>::IC`) requires what its impl's self type writes
#[test]
fn written_ctor_lifetimes() {
    let pre = "use std::cell::Cell;\nenum E<T> { V(T), W { t: T }, U }\nstruct P<T>(T);\nstruct S<T> { t: T }\nfn combine<T>(_: T, _: T) {}\n";
    let at = |body: &str| check(&format!("{pre}{body}\nfn main() {{}}\n"));
    for (body, line) in [
        ("fn a() {\n    let c = 66;\n    E::V::<&'static u32>(&c);\n}", 8),
        ("fn a<'a>(_d: &'a u32) {\n    let c = 66;\n    E::W::<&'a u32> { t: &c };\n}", 8),
        ("fn a() {\n    let c = 66;\n    combine(E::V(Cell::new(&c)), E::U::<Cell<&'static u32>>);\n}", 8),
        ("fn a() {\n    let c = 66;\n    P::<&'static u32>(&c);\n}", 8),
        ("fn a<'a>(_d: &'a u32) {\n    let c = 66;\n    S::<&'a u32> { t: &c };\n}", 8),
    ] {
        let d = at(body);
        assert!(d.len() == 1 && d[0].0 == line && d[0].1.contains("does not live long enough"), "{body}: {d:?}");
    }
    for body in [
        "fn a<'a>(c: &'a u32) {\n    E::V::<&'a u32>(c);\n    E::W::<&'a u32> { t: c };\n    combine(E::V(Cell::new(c)), E::U::<Cell<&'a u32>>);\n    P::<&'a u32>(c);\n    S::<&'a u32> { t: c };\n}",
        "fn a() {\n    let c = 66;\n    E::V::<&u32>(&c);\n    P::<&u32>(&c);\n    S::<&u32> { t: &c };\n    let f = P::<&u32>;\n    f(&c);\n}",
    ] {
        assert_eq!(at(body), vec![], "{body}");
    }
    let d = check("struct Foo<'a> { field: &'a u32 }\nfn main() {\n    let y = 22;\n    let foo = Foo { field: &y };\n    let Foo::<'static> { field: _z } = foo;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    let d = check("struct T<'a>(&'a u32);\nfn main() {\n    let z = 23;\n    let t = T(&z);\n    match t {\n        T::<'static>(_w) => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    let d = check("struct A<'a>(&'a ());\ntrait Tr { const IC: i32; }\nimpl Tr for A<'static> { const IC: i32 = 10; }\nfn f<'a>() -> i32 { <A<'a> as Tr>::IC }\nfn g() -> i32 { <A<'static> as Tr>::IC }\nfn main() { println!(\"{}\", f() + g()); }\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("'a` must outlive `'static"), "{d:?}");
}

/// The lifetimes a path call writes on its type are the impl's (nll/user-annotations/method-ufcs-inherent-*.rs):
/// `A::<'a>::new(&v, 22)` and `<A<'a>>::new(&v, 22)` with `impl<'a> A<'a> { fn new(x: &'a u32, ..) }` require v to
/// outlive 'a, in a fn of one region too; a call in the arguments does not take them
#[test]
fn path_call_self_lifetimes() {
    let pre = "struct A<'a> { x: &'a u32 }\nimpl<'a> A<'a> {\n    fn new<T>(x: &'a u32, _y: T) -> Self { Self { x } }\n    fn id(x: &u32) -> u32 { *x }\n}\n";
    let at = |body: &str| check(&format!("{pre}{body}\nfn main() {{}}\n"));
    for body in [
        "fn f<'a>() {\n    let v = 22;\n    let _x = A::<'a>::new(&v, 22);\n}",
        "fn f<'a>() {\n    let v = 22;\n    let _x = <A<'a>>::new(&v, 22);\n}",
        "fn f() {\n    let v = 22;\n    let _x = A::<'static>::new(&v, A::id(&3));\n}",
    ] {
        let d = at(body);
        assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("does not live long enough"), "{body}: {d:?}");
    }
    assert_eq!(at("fn f<'a>(c: &'a u32) -> A<'a> {\n    let v = 22;\n    let _n = A::<'a>::id(&v);\n    A::<'a>::new(c, A::<'a>::id(&v))\n}"), vec![]);
}

/// A trait object's method borrows the receiver by `&self` / `&mut self`, and a result that borrows from self holds
/// the loan (borrowck/borrowck-object-lifetime.rs, borrowck-borrow-mut-object-twice.rs). A blanket impl's method
/// (`impl<T> Fake for T`) is found for `()`, a reference to a number or `()`, and a reference to a `Box`: Self is the
/// referent, and the reference is the receiver
#[test]
fn dyn_receiver_loans_and_blanket_receivers() {
    let pre = "trait Fake { fn use_mut(&mut self) {} fn use_ref(&self) {} }\nimpl<T> Fake for T {}\ntrait Foo { fn borrowed(&self) -> &(); fn mut_borrowed(&mut self) -> &(); }\n";
    let at = |body: &str| check(&format!("{pre}{body}\nfn main() {{}}\n"));
    let d = at("fn f(x: &mut dyn Foo) {\n    let y = x.borrowed();\n    let _z = x.mut_borrowed();\n    y.use_ref();\n}");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot borrow `*x` as mutable"), "{d:?}");
    let d = at("fn f(mut x: Box<dyn Foo>) {\n    let y = x.borrowed();\n    let _z = &mut x;\n    y.use_ref();\n}");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    assert_eq!(at("fn f(x: &dyn Foo, b: Box<dyn Foo>) {\n    let y = x.borrowed();\n    let z = x.borrowed();\n    z.use_ref();\n    y.use_ref();\n    let w = b.borrowed();\n    let v = &b;\n    v.use_ref();\n    w.use_ref();\n}"), vec![]);
    let d = at("fn f() {\n    let mut void = ();\n    let first = &mut void;\n    let second = &mut void;\n    first.use_mut();\n    second.use_mut();\n}");
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
    assert_eq!(at("fn f() {\n    let mut n = 3u8;\n    let r = &mut n;\n    r.use_mut();\n    ().use_ref();\n    let u = &();\n    u.use_ref();\n}"), vec![]);
}

/// A struct whose field nests two of its lifetimes (`p: &'a mut &'b i32`) relates the borrows it is built from: in a
/// fn the gate leaves out (no region in its signature), the region check is on from its start
/// (nll/capture-ref-in-struct.rs). A blanket impl's method on `()` is `()`'s only when the impl's bounds hold of it
#[test]
fn nested_lifetime_struct_gate_and_blanket_bounds() {
    let src = "struct S<'a, 'b: 'a> {\n    p: &'a mut &'b i32,\n    y: &'b i32,\n}\nimpl<'a, 'b> S<'a, 'b> {\n    fn invoke(self) {\n        *self.p = self.y;\n    }\n}\nfn deref(_: &i32) {}\nfn test() {\n    let x = 44;\n    let mut p = &x;\n    {\n        let y = 22;\n        let s = S { p: &mut p, y: &y };\n        s.invoke();\n    }\n    deref(p);\n}\nfn main() {}\n";
    let d = check(src);
    assert!(d.len() == 1 && d[0].0 == 16 && d[0].1.contains("`y` does not live long enough"), "{d:?}");
    assert_eq!(check(&src.replace("let y = 22;", "let y = &x;").replace("y: &y", "y")), vec![]);
    let d = check("trait Tr { fn go(&self) -> u8 { 1 } }\ntrait Other {}\nimpl<T: ?Sized> Tr for T where T: Other {}\nfn main() {\n    let _x = ().go();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("no method `go`"), "{d:?}");
}

/// What a `Deref` with no `DerefMut` reaches (`Rc`, a `Ref` guard) is shared: writing it, borrowing it mutably, or
/// calling a `&mut self` method of it is E0594 / E0596 (borrowck/borrowck-borrow-overloaded-*.rs). A `MutexGuard`,
/// a `RefMut` and a `RefCell` through an `Rc` are written as before
#[test]
fn shared_deref_writes() {
    let pre = "use std::rc::Rc;\nstruct Point { x: isize, y: isize }\nimpl Point {\n    fn set(&mut self, x: isize) { self.x = x; }\n    fn get(&self) -> isize { self.x + self.y }\n}\n";
    for (body, msg) in [
        ("fn f(x: Rc<isize>) {\n    let _p = &mut *x;\n}", "cannot borrow data in an `Rc` as mutable"),
        ("fn f(x: &Rc<isize>) {\n    **x = 3;\n}", "cannot assign to data in an `Rc`"),
        ("fn f(x: Rc<Point>) {\n    let _p = &mut x.y;\n}", "cannot borrow data in an `Rc` as mutable"),
        ("fn f(mut x: Rc<Point>) {\n    x.y = 3;\n}", "cannot assign to data in an `Rc`"),
        ("fn f(x: Rc<Point>) {\n    x.set(1);\n}", "cannot borrow data in an `Rc` as mutable"),
        ("fn f(x: Rc<Vec<u8>>) {\n    x.push(1);\n}", "cannot borrow data in an `Rc` as mutable"),
    ] {
        let d = check(&format!("{pre}{body}\nfn main() {{}}\n"));
        assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains(msg), "{body}: {d:?}");
    }
    assert_eq!(check(&format!("{pre}fn f(x: Rc<Point>) -> isize {{\n    let a = &x.y;\n    *a + x.get() + (*x).x\n}}\nfn main() {{}}\n")), vec![]);
    assert_eq!(check("use std::cell::RefCell;\nuse std::rc::Rc;\nuse std::sync::Mutex;\nfn main() {\n    let m = Mutex::new(vec![1]);\n    let mut g = m.lock().unwrap();\n    g.push(2);\n    *g = vec![3];\n    drop(g);\n    let r = Rc::new(RefCell::new(String::new()));\n    r.borrow_mut().push('a');\n    let mut b = r.borrow_mut();\n    *b = String::from(\"c\");\n}\n"), vec![]);
}

/// A closure that passes a `&mut` it captures where a `&mut` is expected (`|| foo(x)`, `|| s.put(x)`) borrows `*x`
/// uniquely: it is `FnMut`, so calling it through a binding that is not `mut` is E0596
/// (closures/issue-81700-mut-borrow.rs). Where a `&` is expected, the reborrow is shared and the closure is `Fn`
#[test]
fn closure_unique_reborrow_is_fn_mut() {
    let pre = "fn foo(x: &mut u32) { *x += 1; }\nfn look(x: &u32) -> u32 { *x }\nstruct S;\nimpl S {\n    fn put(&self, x: &mut u32) { *x += 1; }\n    fn see(&self, x: &u32) -> u32 { *x }\n}\n";
    for body in ["fn f(x: &mut u32) {\n    let bar = || foo(x);\n    bar();\n}", "fn f(x: &mut u32, s: &S) {\n    let bar = || s.put(x);\n    bar();\n}", "fn f(x: &mut u32) {\n    let bar = || foo(&mut *x);\n    bar();\n}"] {
        let d = check(&format!("{pre}{body}\nfn main() {{}}\n"));
        assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("cannot borrow `bar` as mutable"), "{body}: {d:?}");
    }
    assert_eq!(check(&format!("{pre}fn f(x: &mut u32, s: &S) -> u32 {{\n    let bar = || look(x) + s.see(x);\n    let n = bar();\n    let mut baz = || foo(x);\n    baz();\n    baz();\n    n\n}}\nfn main() {{}}\n")), vec![]);
    // (the closure borrows `*x`: shared for `look(x)`, uniquely for `foo(x)`; E0500 / E0502 against it)
    for body in ["fn f(x: &mut u32) -> u32 {\n    let bar = || look(x);\n    let mut baz = || foo(x);\n    baz();\n    bar()\n}", "fn f(x: &mut u32) -> u32 {\n    let bar = || look(x);\n    let m = &mut *x;\n    *m += 1;\n    bar()\n}"] {
        let d = check(&format!("{pre}{body}\nfn main() {{}}\n"));
        assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("cannot borrow `*x` as mutable"), "{body}: {d:?}");
    }
}

/// From the review of #54: a pattern's turbofish gives the matched value's instance (`let P::<u16>(x, _) =
/// P(300, 1);` makes x a u16) and is checked against it (`S::<u16>(_)` on an `S<u8>`); a struct literal's written
/// arguments are checked against the expected instance, `_` aside; a union's `&mut` field is not `Copy`; the
/// enum's arguments written beside the variant's are E0109; a method call's turbofish and a turbofish on several
/// lines turn the region check on; a blanket impl's bounds hold transitively; a by-value `self` keeps a `&mut`
/// receiver; a guard's closure's parameters are its own; a struct is not taken for a closure by its id
#[test]
fn part11_review() {
    // (reported where the value is built, line 3; rustc reports it at the pattern)
    let d = check("struct S<T>(T);\nfn main() {\n    match S(1u8) {\n        S::<u16>(_) => {}\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("mismatched types"), "{d:?}");
    let d = check("struct S<T>(T);\nfn f(s: S<u8>) {\n    match s {\n        S::<u16>(_) => {}\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("mismatched types"), "{d:?}");
    assert_eq!(check("struct S<T>(T);\nfn main() {\n    match S(1u8) {\n        S::<u8>(_) => {}\n    }\n}\n"), vec![]);
    let d = check("struct S<A, B> { a: A, b: B }\nfn main() {\n    let s: S<u8, u8> = S::<u16, _> { a: 1, b: 2 };\n    let _ = (s.a, s.b);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("expected `S<u8, u8>`, found `S<u16, _>`"), "{d:?}");
    assert_eq!(check("struct S<A, B> { a: A, b: B }\nfn main() {\n    let s: S<u8, u8> = S::<u8, _> { a: 1, b: 2 };\n    let _ = (s.a, s.b);\n}\n"), vec![]);
    let d = check("union U<T: Copy> { a: T }\nfn main() {\n    let mut n = 1u8;\n    let _u = U { a: &mut n };\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`&mut u8: Copy`"), "{d:?}");
    let d = check("enum E<'a, T> { V(&'a u8, T) }\nfn main() {\n    let local = 1u8;\n    let _e = E::<'static>::V::<u8>(&local, 0u8);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0109"), "{d:?}");
    let d = check("struct Q;\nimpl Q {\n    fn take<T>(&self, _t: T) {}\n}\nfn f() {\n    let c = 1u32;\n    Q.take::<&'static u32>(&c);\n}\nfn main() { f() }\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("`c` does not live long enough"), "{d:?}");
    let d = check("use std::cell::Cell;\nenum E<T> { V(T), U }\nfn combine<T>(_: T, _: T) {}\nfn f() {\n    let c = 66;\n    combine(E::V(Cell::new(&c)), E::U::<\n        Cell<&'static u32>,\n    >);\n}\nfn main() { f() }\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("`c` does not live long enough"), "{d:?}");
    let d = check("trait Marker {}\ntrait Other {}\nimpl<T: Marker> Other for T {}\ntrait Tr { fn go(&self) {} }\nimpl<T: Other> Tr for T {}\nfn main() {\n    ().go();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("no method `go`"), "{d:?}");
    assert_eq!(check("trait Id { fn id(self) -> Self where Self: Sized { self } }\nimpl<T> Id for T {}\nfn main() {\n    let mut n = 3u8;\n    let r = &mut n;\n    let r2: &mut u8 = r.id();\n    *r2 += 1;\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let mut y = 1;\n    match Some(5) {\n        Some(ref mut r) if (|r: &mut i32| { *r = 2; true })(&mut y) => { *r += 1; }\n        _ => {}\n    }\n}\n"), vec![]);
}

/// A generic fn whose instances all give a `Copy` type for a parameter its bounds do not make `Copy`
/// (`dup(1u32)` of `fn dup<T>(x: T) -> (T, T) { (x, x) }`) is checked with placeholders too: moving a T twice is
/// E0382, as rustc checks the generic body. One with a `T: Copy` bound is not
#[test]
fn generic_moves_behind_copy_instances() {
    let d = check("fn dup<T>(x: T) -> (T, T) {\n    (x, x)\n}\nfn main() {\n    let _ = dup(1u32);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("use of moved value: `x`"), "{d:?}");
    assert_eq!(check("fn dup<T: Copy>(x: T) -> (T, T) {\n    (x, x)\n}\nfn main() {\n    let _ = dup(1u32);\n}\n"), vec![]);
}

/// From the second review of #54: a blanket impl's bound holds by an impl for the type itself (`impl Other for ()`
/// beside `impl<T: Marker> Other for T`); a reference receiver of a blanket method is reborrowed, and a result that
/// borrows from self (`fn get(&mut self) -> &mut Self`, `fn id(self) -> Self`) holds the loan (E0506)
#[test]
fn part11_review_2() {
    assert_eq!(check("trait Marker {}\ntrait Other {}\nimpl<T: Marker> Other for T {}\nimpl Other for () {}\ntrait Tr { fn go(&self) {} }\nimpl<T: Other> Tr for T {}\nfn main() {\n    ().go();\n}\n"), vec![]);
    for (tr, call) in [("trait Fake { fn get(&mut self) -> &mut Self { self } }", "r.get()"), ("trait Fake { fn id(self) -> Self where Self: Sized { self } }", "r.id()")] {
        let src = format!("{tr}\nimpl<T> Fake for T {{}}\nfn main() {{\n    let mut n = 3u8;\n    let r = &mut n;\n    let r2: &mut u8 = {call};\n    *r = 5;\n    *r2 += 1;\n}}\n");
        let d = check(&src);
        assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot assign to `*r` because it is borrowed"), "{call}: {d:?}");
        assert_eq!(check(&src.replace("    *r = 5;\n    *r2 += 1;\n", "    *r2 += 1;\n    *r = 5;\n")), vec![], "{call}");
    }
}

/// From the third and fourth reviews of #54: a blanket method's by-value `self` gives its loan to a result that is
/// Self only (`fn pick<U>(self, v: U) -> U` does not hold `*r`); a `&mut` receiver's loan lasts while arguments that
/// name it are evaluated (`r.touch({ *r = 5; })`, E0506); a trait object's `&mut self` call is two-phase
/// (`x.set(x.get())` with `get` returning a `&u8` is E0502, with a `u8` it is not); the edition is in the check memo's
/// fingerprint (a session that changes it checks the bodies again). From the follow-ups of the second review: a chain
/// of blanket bounds longer than the depth followed holds only by an impl for the type itself (six traits deep from
/// `Marker`, `().go()` is E0599 without `impl Marker for ()`), and a pattern's turbofish with too many arguments is E0107
#[test]
fn part11_review_3() {
    let deep = "trait Marker {}\ntrait A {}\ntrait B {}\ntrait C {}\ntrait D {}\ntrait E {}\nimpl<T: Marker> A for T {}\nimpl<T: A> B for T {}\nimpl<T: B> C for T {}\nimpl<T: C> D for T {}\nimpl<T: D> E for T {}\ntrait Tr {\n    fn go(&self) {}\n}\nimpl<T: E> Tr for T {}\nfn main() {\n    ().go();\n}\n";
    let d = check(deep);
    assert!(d.len() == 1 && d[0].0 == 17 && d[0].1.contains("go"), "{d:?}");
    assert_eq!(check(&deep.replace("trait Marker {}\n", "trait Marker {}\nimpl Marker for () {}\n")), vec![]);
    let d = check("struct S<T>(T);\nfn f(s: S<u8>) -> u8 {\n    match s {\n        S::<u8, u16>(x) => x,\n    }\n}\nfn main() {\n    println!(\"{}\", f(S(1)));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0107"), "{d:?}");
    assert_eq!(check("trait P { fn pick<U>(self, value: U) -> U where Self: Sized { value } }\nimpl<T> P for T {}\nfn main() {\n    let mut n = 3u8;\n    let r = &mut n;\n    let k = 7u8;\n    let v = r.pick(&k);\n    *r = 5;\n    println!(\"{}\", v);\n}\n"), vec![]);
    let d = check("trait Tc { fn touch(&mut self, _u: ()) {} }\nimpl<T> Tc for T {}\nfn main() {\n    let mut n = 3u8;\n    let r = &mut n;\n    r.touch({ *r = 5; });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot assign to `*r` because it is borrowed"), "{d:?}");
    let d = check("trait T {\n    fn get(&self) -> &u8;\n    fn set(&mut self, v: &u8);\n}\nfn f(x: &mut dyn T) {\n    x.set(x.get());\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot borrow `*x` as mutable because it is also borrowed as immutable"), "{d:?}");
    assert_eq!(check("trait T {\n    fn get(&self) -> u8;\n    fn set(&mut self, v: u8);\n}\nfn f(x: &mut dyn T) {\n    x.set(x.get());\n}\nfn main() {}\n"), vec![]);
    let src = "struct D<T>(T);\nimpl<T> Drop for D<T> {\n    fn drop(&mut self) {}\n}\nimpl<T> D<T> {\n    fn end(&self) {}\n}\nfn f() {\n    let t = D(Box::new(1));\n    D(&t).end()\n}\nfn main() {\n    f();\n}\n";
    // (a successful check under 2024 is not reused under 2021)
    let mut s = Session::new(Options { jobs: 1, edition: 2024, ..Options::default() });
    assert!(s.check(src).is_empty());
    s.opts.edition = 2021;
    assert_eq!(s.check(src).len(), 1);
    s.opts.edition = 2024;
    assert!(s.check(src).is_empty());
}

/// A destructuring `let` with a type annotation binds from a value of the annotation's type: the bindings have its
/// lifetimes (nll/issue-55401.rs: `let (ref y, _z): (&'a u32, u32) = (&22, 44); *y` as a `&'static u32`)
#[test]
fn let_pattern_annotation_lifetimes() {
    let d = check("fn f<'a>(_x: &'a u32) -> &'static u32 {\n    let (y, _z): (&'a u32, u32) = (&22, 44);\n    y\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("fn f<'a>(_x: &'a u32) -> &'static u32 {\n    let (ref y, _z): (&'a u32, u32) = (&22, 44);\n    *y\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    let d = check("fn m<'a, 'b>(p: (&'a u32, &'b u32)) -> &'a u32 {\n    let (_, b): (&u32, &'b u32) = p;\n    b\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("`'b` must outlive `'a`"), "{d:?}");
    assert_eq!(check("struct S<'a> {\n    x: &'a u32,\n}\nfn g<'a>(x: &'a u32) -> &'a u32 {\n    let (y, _): (&'a u32, u32) = (x, 1);\n    y\n}\nfn k<'a>(s: S<'a>) -> &'a u32 {\n    let S { x }: S<'_> = s;\n    x\n}\nfn m<'a, 'b>(p: (&'a u32, &'b u32)) -> &'a u32 {\n    let (a, _): (&'a u32, &u32) = p;\n    a\n}\nfn q<'a>(x: &'a mut u32) -> u32 {\n    let (ref mut y, z): (&'a mut u32, u32) = (x, 3);\n    **y += z;\n    **y\n}\nfn main() {\n    let mut w = 4;\n    println!(\"{} {} {} {}\", g(&1), k(S { x: &7 }), m((&1, &2)), q(&mut w));\n}\n"), vec![]);
}

/// A closure made where a trait object of a closure trait is expected is checked with that trait's signature, as with
/// a bound (borrowck/regions-escape-unboxed-closure.rs: `with_int(&mut |y| x = Some(y))` for a
/// `&mut dyn FnMut(&isize)` is E0521)
#[test]
fn closure_dyn_fn_signature() {
    let d = check("fn with_int(_f: &mut dyn FnMut(&isize)) {}\nfn main() {\n    let mut x: Option<&isize> = None;\n    with_int(&mut |y| x = Some(y));\n    let _ = x;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("borrowed data escapes outside of closure"), "{d:?}");
    let d = check("fn main() {\n    let mut x: Option<&isize> = None;\n    let f: &mut dyn FnMut(&isize) = &mut |y| x = Some(y);\n    f(&1);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("borrowed data escapes outside of closure"), "{d:?}");
    assert_eq!(check("fn g<'a>(out: &mut Vec<&'a u32>, a: &'a u32) {\n    let f: &mut dyn FnMut(&'a u32) = &mut |y| out.push(y);\n    f(a);\n}\nfn main() {\n    let mut count = 0;\n    {\n        let f: &mut dyn FnMut(&str) = &mut |s| count += s.len();\n        f(\"abc\");\n    }\n    let k: &dyn Fn(&u32) -> &u32 = &|x| x;\n    let first: &dyn Fn(&Vec<u32>) -> Option<&u32> = &|v| v.first();\n    let v = vec![4u32, 5];\n    let mut all = vec![];\n    g(&mut all, &v[0]);\n    println!(\"{} {} {:?} {}\", count, k(&7), first(&v), all.len());\n}\n"), vec![]);
}

/// An impl's method whose regions make it less general than its trait's declaration: the declaration's regions are
/// placeholders, the method's own lifetimes variables (lifetimes/lifetime-mismatch-between-trait-and-impl.rs,
/// trait-impl-mismatch-elided-lifetime-issue-65866.rs)
#[test]
fn impl_method_regions_against_the_trait() {
    let d = check("trait Foo {\n    fn foo<'a>(x: &i32, y: &'a i32) -> &'a i32;\n}\nimpl Foo for () {\n    fn foo<'a>(x: &'a i32, y: &'a i32) -> &'a i32 {\n        if x > y { x } else { y }\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`impl` item signature doesn't match `trait` item signature"), "{d:?}");
    let d = check("struct Foo;\nstruct Re<'a> {\n    _data: &'a u16,\n}\ntrait Bar {\n    fn bar(&self, r: &mut Re);\n}\nimpl Bar for Foo {\n    fn bar<'a, 'b>(&'a self, _r: &'b mut Re<'a>) {}\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 9, "{d:?}");
    assert_eq!(check("trait B {\n    fn g<'a>(&self, x: &'a u32) -> &'a u32;\n}\ntrait C {\n    fn h<'a>(x: &'a u32, y: &'a u32) -> &'a u32;\n}\ntrait A {\n    fn f(&self) -> &u32;\n}\nstruct S(u32);\nstruct L<'a>(&'a u32);\nimpl B for S {\n    fn g<'b>(&self, x: &'b u32) -> &'b u32 {\n        x\n    }\n}\nimpl C for S {\n    fn h<'a, 'b>(x: &'a u32, _y: &'b u32) -> &'a u32 {\n        x\n    }\n}\nimpl<'a> A for L<'a> {\n    fn f(&self) -> &u32 {\n        self.0\n    }\n}\nfn main() {\n    let n = 5;\n    println!(\"{} {} {}\", S(1).g(&n), S::h(&n, &n), L(&n).f());\n}\n"), vec![]);
}

/// A closure called holds what it borrows until the call, after its arguments (regions/
/// region-bound-on-closure-outlives-call.rs: `(|x| f(x))(call_rec(f))` is E0505; `c(take(v))` with c capturing v)
#[test]
fn closure_call_holds_its_captures_through_the_arguments() {
    let d = check("fn call_rec<F>(mut f: F) -> usize\nwhere\n    F: FnMut(usize) -> usize,\n{\n    (|x| f(x))(call_rec(f))\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot move out of `f` because it is borrowed"), "{d:?}");
    let d = check("fn take(v: Vec<u32>) -> usize {\n    v.len()\n}\nfn main() {\n    let v = vec![1];\n    let c = |x: usize| v.len() + x;\n    let _ = c(take(v));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot move out of `v` because it is borrowed"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut v = vec![1u32];\n    let mut c = |x| v.push(x);\n    c(1);\n    c(2);\n    let w = vec![5u32, 6];\n    let d = |x: usize| w.len() + x;\n    let a = d(w.len());\n    let s = String::from(\"hi\");\n    let e = move |k: usize| s.len() + k;\n    let t = e(e(1));\n    println!(\"{:?} {} {}\", v, a, t);\n}\n"), vec![]);
}

/// An `unsafe fn` of a blanket impl called on `()`, a number, a `&str` or a reference to one, and one of an impl for a
/// number, is E0133 outside `unsafe` (the receivers dispatched before the method call's own check; from the summary
/// review of #54)
#[test]
fn blanket_and_number_unsafe_methods() {
    let d = check("trait T {\n    unsafe fn f(&self) -> u32 {\n        1\n    }\n}\nimpl<X> T for X {}\nfn main() {\n    let r = &5u32;\n    let a = r.f();\n    println!(\"{}\", a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("E0133"), "{d:?}");
    let d = check("trait T {\n    unsafe fn f(&self) -> u32 {\n        1\n    }\n}\nimpl<X> T for X {}\nfn main() {\n    let a = ().f();\n    println!(\"{}\", a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("E0133"), "{d:?}");
    let d = check("trait P {\n    unsafe fn g(&self) -> u32;\n}\nimpl P for u8 {\n    unsafe fn g(&self) -> u32 {\n        2\n    }\n}\nfn main() {\n    let c = 3u8.g();\n    println!(\"{}\", c);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("E0133"), "{d:?}");
    assert_eq!(check("trait T {\n    unsafe fn f(&self) -> u32 {\n        1\n    }\n}\nimpl<X> T for X {}\ntrait P {\n    unsafe fn g(&self) -> u32;\n}\nimpl P for u8 {\n    unsafe fn g(&self) -> u32 {\n        2\n    }\n}\nfn main() {\n    let r = &5u32;\n    let x = unsafe { ().f() + r.f() + 3u8.f() + \"a\".f() + 4u8.g() };\n    println!(\"{}\", x);\n}\n"), vec![]);
}

/// A closure's captures held by value outlive the body's call: what they borrow of the body's own escapes it
/// (borrowck/borrowed-data-escapes-closure-148392.rs: `move || { b = Some(&a); }`, rustc's E0521)
#[test]
fn closure_value_captures_outlive_the_call() {
    let d = check("fn main() {\n    let a = 0;\n    let mut b = None;\n    move || {\n        b = Some(&a);\n    };\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`a` does not live long enough"), "{d:?}");
    assert_eq!(check("fn main() {\n    let a = 0;\n    let mut b: Option<&i32> = None;\n    let mut c = || {\n        b = Some(&a);\n    };\n    c();\n    println!(\"{:?}\", b);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let s = 5;\n    let mut acc: Vec<&i32> = vec![];\n    let r = &s;\n    let mut c = move || {\n        acc.push(r);\n        acc.len()\n    };\n    let t = String::from(\"ab\");\n    let e = move || {\n        let x = &t;\n        x.len()\n    };\n    println!(\"{} {}\", c(), e());\n}\n"), vec![]);
}

/// A closure whose kind nothing decides that returns a closure mutating what it captures by reference: the inner
/// closure's `&mut` of the capture is through the outer's `&mut self` (nll/issue-40510-3.rs,
/// closures/nested-closure-escape-borrow.rs)
#[test]
fn closure_returning_a_closure_that_mutates_its_capture() {
    let d = check("fn main() {\n    let mut x: Vec<()> = Vec::new();\n    || {\n        || {\n            x.push(())\n        }\n    };\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("captured variable cannot escape `FnMut` closure body"), "{d:?}");
    let d = check("fn main() {\n    let mut x = 0;\n    || {\n        || {\n            let _y = &mut x;\n        }\n    };\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    assert_eq!(check("fn once<F: FnOnce() -> G, G: FnMut()>(f: F) -> G {\n    f()\n}\nfn main() {\n    let v = vec![1u32];\n    let f = || {\n        || v.len()\n    };\n    let n = f()();\n    let mut c = 0;\n    let mut g = || {\n        c += 1;\n        c\n    };\n    let k = g();\n    let mut w = vec![1u32];\n    let mut h = once(|| || w.push(2));\n    h();\n    println!(\"{} {} {:?}\", n, k, w);\n}\n"), vec![]);
}

/// A `move` closure holds what its captures held by value hold: returned from a closure that borrows its own capture
/// for it, the borrow escapes (borrowck/do-not-suggest-adding-move-when-closure-is-already-marked-as-move.rs)
#[test]
fn move_closure_holds_what_its_value_captures_hold() {
    let d = check("fn main() {\n    let mut vec: Vec<i32> = Vec::new();\n    let _closure = move || {\n        vec.clear();\n        let mut iter = vec.iter();\n        move || iter.next()\n    };\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    assert_eq!(check("fn f<'a>(s: &'a str) -> impl Fn() -> usize + 'a {\n    move || s.len()\n}\nfn main() {\n    let v = vec![1u32, 2];\n    let r = &v;\n    let g = move || r.len();\n    println!(\"{} {}\", f(\"ab\")(), g());\n}\n"), vec![]);
}

/// `(&x).clone()` with x: T and no `Clone` bound is the reference's copy: it borrows x (borrowck/
/// borrowck-return-variable-on-stack-via-clone.rs, E0515)
#[test]
fn reference_clone_of_an_unbounded_param() {
    let d = check("fn leak<'a, T>(x: T) -> &'a T {\n    (&x).clone()\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("`x`"), "{d:?}");
    // (an instance's `x.clone()` with T a `Clone` type is that type's clone, not the reference's: not followed)
    assert_eq!(check("fn dup<T: Clone>(x: &T) -> T {\n    x.clone()\n}\nfn main() {\n    let s = String::from(\"a\");\n    println!(\"{}\", dup(&s));\n}\n"), vec![]);
}

/// From the review of #64: a session's second check keeps a closure body's kind (its parent taken from the record);
/// a `Deref` whose target is the type itself is not followed forever; a destructuring `let` writing `'static` turns
/// the check on, and one with holes (`(&'static u8, _)`) is related element by element; a closure called holds a
/// borrow of itself through its arguments (`c(drop(c))`, E0505); an impl's `&'static` input elides its result
#[test]
fn part12_review() {
    let src = "fn main() {\n    let mut x: Vec<()> = Vec::new();\n    || {\n        || {\n            x.push(())\n        }\n    };\n}\n";
    let mut s = Session::new(Options { jobs: 1, ..Options::default() });
    assert_eq!(s.check(src).len(), 1);
    assert_eq!(s.check(&format!("{src}fn other() {{}}\n")).len(), 1);
    assert_eq!(s.check(&format!("{src}fn other() {{\n    let _ = 1;\n}}\n")).len(), 1);
    assert_eq!(s.check(src).len(), 1);
    let d = check("use std::ops::Deref;\nstruct S {\n    v: u32,\n}\nimpl Deref for S {\n    type Target = S;\n    fn deref(&self) -> &S {\n        self\n    }\n}\nfn main() {\n    let s = S { v: 1 };\n    let c = || s.missing;\n    let _ = c;\n}\n");
    assert!(!d.is_empty(), "{d:?}");
    let d = check("fn main() {\n    let x = 5;\n    let (a, _): (&'static i32, u8) = (&x, 0);\n    println!(\"{}\", a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    let d = check("fn f(r: &u8) -> u8 {\n    let (x, _): (&'static u8, _) = (r, 0u8);\n    *x\n}\nfn main() {\n    let v = 1;\n    println!(\"{}\", f(&v));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let s = String::from(\"a\");\n    let c = move |_: ()| s.len();\n    let _ = c(drop(c));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("cannot move out of `c` because it is borrowed"), "{d:?}");
    let d = check("trait Tr {\n    fn f(x: &u8) -> &u8;\n}\nstruct S;\nimpl Tr for S {\n    fn f(x: &'static u8) -> &u8 {\n        x\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("`impl` item signature doesn't match"), "{d:?}");
    assert_eq!(check("trait Tr {\n    fn f(x: &'static u8) -> &u8;\n}\nstruct S;\nimpl Tr for S {\n    fn f(x: &'static u8) -> &u8 {\n        x\n    }\n}\nfn main() {\n    let mut n = 0;\n    let mut f = |x: u32| {\n        n += x;\n        x + 1\n    };\n    let a = f(1);\n    let s = String::from(\"ab\");\n    let e = move |k: usize| s.len() + k;\n    let (p, _): (&'static str, _) = (\"x\", 1u8);\n    println!(\"{} {} {} {}\", a, e(e(1)), n, p);\n}\n"), vec![]);
}

/// From the review of #54 (a follow-up): a chain of blanket impls is followed past eight impls (as far as rustc's
/// recursion limit), so `().go()` through ten of them is accepted when `()` has the first trait and rejected when it
/// does not; a cycle of blanket impls does not hold (E0599 in rustc), and is not followed forever
#[test]
fn blanket_chain_past_eight() {
    let chain = "trait Marker {}\ntrait A {}\ntrait B {}\ntrait C {}\ntrait D {}\ntrait E {}\ntrait F {}\ntrait G {}\ntrait H {}\ntrait I {}\nimpl<T: Marker> A for T {}\nimpl<T: A> B for T {}\nimpl<T: B> C for T {}\nimpl<T: C> D for T {}\nimpl<T: D> E for T {}\nimpl<T: E> F for T {}\nimpl<T: F> G for T {}\nimpl<T: G> H for T {}\nimpl<T: H> I for T {}\ntrait Tr {\n    fn go(&self) -> u32 {\n        7\n    }\n}\nimpl<T: I> Tr for T {}\n";
    assert_eq!(check(&format!("{chain}impl Marker for () {{}}\nfn main() {{\n    println!(\"{{}}\", ().go());\n}}\n")), vec![]);
    let d = check(&format!("{chain}fn main() {{\n    println!(\"{{}}\", ().go());\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 27 && d[0].1.contains("no method `go`"), "{d:?}");
    let d = check("trait A {}\ntrait B {}\nimpl<T: B> A for T {}\nimpl<T: A> B for T {}\ntrait Tr {\n    fn go(&self) -> u32 {\n        7\n    }\n}\nimpl<T: A> Tr for T {}\nfn main() {\n    println!(\"{}\", ().go());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 12 && d[0].1.contains("no method `go`"), "{d:?}");
}

/// From the second review of #64: a write through a chain of 17 `Deref`s with no `DerefMut` is E0594 (the borrow
/// check follows such a chain as far as rustc's recursion limit, not 16 steps), and a read through it is accepted; a
/// destructuring `let` whose annotation writes `'static` turns the check on before its initializer is lowered, so a
/// call in it (`(id(r), 0)`) is related to the annotation
#[test]
fn part12_review_2() {
    let chain = "use std::ops::Deref;\nstruct S0 {\n    field: u32,\n}\nstruct S1(S0);\nimpl Deref for S1 {\n    type Target = S0;\n    fn deref(&self) -> &S0 {\n        &self.0\n    }\n}\nstruct S2(S1);\nimpl Deref for S2 {\n    type Target = S1;\n    fn deref(&self) -> &S1 {\n        &self.0\n    }\n}\nstruct S3(S2);\nimpl Deref for S3 {\n    type Target = S2;\n    fn deref(&self) -> &S2 {\n        &self.0\n    }\n}\nstruct S4(S3);\nimpl Deref for S4 {\n    type Target = S3;\n    fn deref(&self) -> &S3 {\n        &self.0\n    }\n}\nstruct S5(S4);\nimpl Deref for S5 {\n    type Target = S4;\n    fn deref(&self) -> &S4 {\n        &self.0\n    }\n}\nstruct S6(S5);\nimpl Deref for S6 {\n    type Target = S5;\n    fn deref(&self) -> &S5 {\n        &self.0\n    }\n}\nstruct S7(S6);\nimpl Deref for S7 {\n    type Target = S6;\n    fn deref(&self) -> &S6 {\n        &self.0\n    }\n}\nstruct S8(S7);\nimpl Deref for S8 {\n    type Target = S7;\n    fn deref(&self) -> &S7 {\n        &self.0\n    }\n}\nstruct S9(S8);\nimpl Deref for S9 {\n    type Target = S8;\n    fn deref(&self) -> &S8 {\n        &self.0\n    }\n}\nstruct S10(S9);\nimpl Deref for S10 {\n    type Target = S9;\n    fn deref(&self) -> &S9 {\n        &self.0\n    }\n}\nstruct S11(S10);\nimpl Deref for S11 {\n    type Target = S10;\n    fn deref(&self) -> &S10 {\n        &self.0\n    }\n}\nstruct S12(S11);\nimpl Deref for S12 {\n    type Target = S11;\n    fn deref(&self) -> &S11 {\n        &self.0\n    }\n}\nstruct S13(S12);\nimpl Deref for S13 {\n    type Target = S12;\n    fn deref(&self) -> &S12 {\n        &self.0\n    }\n}\nstruct S14(S13);\nimpl Deref for S14 {\n    type Target = S13;\n    fn deref(&self) -> &S13 {\n        &self.0\n    }\n}\nstruct S15(S14);\nimpl Deref for S15 {\n    type Target = S14;\n    fn deref(&self) -> &S14 {\n        &self.0\n    }\n}\nstruct S16(S15);\nimpl Deref for S16 {\n    type Target = S15;\n    fn deref(&self) -> &S15 {\n        &self.0\n    }\n}\nstruct S17(S16);\nimpl Deref for S17 {\n    type Target = S16;\n    fn deref(&self) -> &S16 {\n        &self.0\n    }\n}\n";
    let d = check(&(chain.to_string() + "fn main() {\n    let mut x = S17(S16(S15(S14(S13(S12(S11(S10(S9(S8(S7(S6(S5(S4(S3(S2(S1(S0 { field: 0 })))))))))))))))));\n    x.field = 1;\n}\n"));
    assert!(d.len() == 1 && d[0].1.contains("cannot assign to data in dereference of `S1` (E0594)"), "{d:?}");
    assert_eq!(check(&(chain.to_string() + "fn main() {\n    let x = S17(S16(S15(S14(S13(S12(S11(S10(S9(S8(S7(S6(S5(S4(S3(S2(S1(S0 { field: 0 })))))))))))))))));\n    println!(\"{}\", x.field);\n}\n")), vec![]);
    let id = "fn id(x: &u8) -> &u8 {\n    x\n}\n";
    let d = check(&(id.to_string() + "fn f(r: &u8) {\n    let (_x, _): (&'static u8, u8) = (id(r), 0);\n}\nfn main() {\n    let v = 1;\n    f(&v);\n}\n"));
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check(&(id.to_string() + "static X: u8 = 1;\nfn main() {\n    let (p, _): (&'static u8, u8) = (id(&X), 0);\n    println!(\"{}\", p);\n}\n")), vec![]);
}

/// From the third review of #64: a closure's captures held by value escape at each of its exits, an explicit `return`
/// and a `?` too (E0521 in rustc), not only at the body's end; a `Deref` chain that does not reach the field within
/// rustc's recursion limit (128) is E0055, so the borrow check, which follows a place's derefs as far, does not drop
/// a place the program keeps (a write through 129 `Deref`s), and a chain of 128 is followed
#[test]
fn part12_review_3() {
    let d = check("fn main() {\n    let a = 1;\n    let mut b: Option<&i32> = None;\n    let mut c = move || {\n        b = Some(&a);\n        return;\n    };\n    c();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`a` does not live long enough"), "{d:?}");
    let d = check("fn get() -> Option<u8> {\n    None\n}\nfn main() {\n    let a = 1;\n    let mut b: Option<&i32> = None;\n    let mut c = move || -> Option<u8> {\n        b = Some(&a);\n        let v = get()?;\n        loop {\n            let _ = v;\n        }\n    };\n    let _ = c();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("`a` does not live long enough"), "{d:?}");
    assert_eq!(check("fn main() {\n    let a = 1;\n    let mut c = move |k: bool| -> i32 {\n        let r = &a;\n        if k {\n            return *r;\n        }\n        *r + 1\n    };\n    println!(\"{}\", c(true));\n}\n"), vec![]);
    // (S_n derefs to S_{n-1}; S0 has the field)
    let chain = |n: usize, body: &str| {
        let mut s = String::from("use std::ops::Deref;\nstruct S0 {\n    field: u32,\n}\n");
        let mut v = String::from("S0 { field: 0 }");
        for i in 1..=n {
            s += &format!("struct S{i}(S{});\nimpl Deref for S{i} {{\n    type Target = S{};\n    fn deref(&self) -> &S{} {{\n        &self.0\n    }}\n}}\n", i - 1, i - 1, i - 1);
            v = format!("S{i}({v})");
        }
        s + &body.replace("VALUE", &v)
    };
    let d = check(&chain(129, "fn main() {\n    let mut x = VALUE;\n    x.field = 1;\n}\n"));
    assert!(d.len() == 1 && d[0].1.contains("reached the recursion limit while auto-dereferencing `S129` (E0055)"), "{d:?}");
    let d = check(&chain(128, "fn main() {\n    let mut x = VALUE;\n    x.field = 1;\n}\n"));
    assert!(d.len() == 1 && d[0].1.contains("cannot assign to data in dereference of `S1` (E0594)"), "{d:?}");
    assert_eq!(check(&chain(128, "fn main() {\n    let x = VALUE;\n    println!(\"{}\", x.field);\n}\n")), vec![]);
    // (through a generic target, `W<T>: Deref<Target = T>`: as far, then E0055; a shorter chain's write is E0594)
    let generic = |n: usize, body: &str| {
        let mut v = String::from("S0 { field: 0 }");
        for _ in 0..n {
            v = format!("W({v})");
        }
        "use std::ops::Deref;\nstruct S0 {\n    field: u32,\n}\nstruct W<T>(T);\nimpl<T> Deref for W<T> {\n    type Target = T;\n    fn deref(&self) -> &T {\n        &self.0\n    }\n}\n".to_string() + &body.replace("VALUE", &v)
    };
    let d = check(&generic(129, "fn main() {\n    let mut x = VALUE;\n    x.field = 1;\n}\n"));
    assert!(d.len() == 1 && d[0].1.contains("reached the recursion limit while auto-dereferencing `W<"), "{d:?}");
    let d = check(&generic(20, "fn main() {\n    let mut x = VALUE;\n    x.field = 1;\n}\n"));
    assert!(d.len() == 1 && d[0].1.contains("cannot assign to data in dereference of `W` (E0594)"), "{d:?}");
    assert_eq!(check(&generic(20, "fn main() {\n    let x = VALUE;\n    println!(\"{}\", x.field);\n}\n")), vec![]);
    // (a target that is a reference, `Target = &'static S`: followed through it, as part of the chain; a cycle so is
    // E0055, not lowered again and again until the stack overflows)
    let rr = "use std::ops::Deref;\nstruct S0 {\n    field: u32,\n}\nstatic Z: S0 = S0 { field: 3 };\nstatic RZ: &S0 = &Z;\nstruct W;\nimpl Deref for W {\n    type Target = &'static S0;\n    fn deref(&self) -> &&'static S0 {\n        &RZ\n    }\n}\n";
    assert_eq!(check(&format!("{rr}fn main() {{\n    let w = W;\n    println!(\"{{}}\", w.field);\n}}\n")), vec![]);
    let d = check("use std::ops::Deref;\nstruct S;\nstatic SS: S = S;\nstatic RS: &S = &SS;\nimpl Deref for S {\n    type Target = &'static S;\n    fn deref(&self) -> &&'static S {\n        &RS\n    }\n}\nfn main() {\n    let s = S;\n    println!(\"{}\", s.missing);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 13 && d[0].1.contains("reached the recursion limit while auto-dereferencing `S` (E0055)"), "{d:?}");
    // (a cycle through two types: type inference followed it until the stack overflowed)
    let d = check("use std::ops::Deref;\nstruct A {\n    x: u32,\n}\nstruct B {\n    y: u32,\n}\nstatic BB: B = B { y: 2 };\nstatic AA: A = A { x: 2 };\nimpl Deref for A {\n    type Target = B;\n    fn deref(&self) -> &B {\n        &BB\n    }\n}\nimpl Deref for B {\n    type Target = A;\n    fn deref(&self) -> &A {\n        &AA\n    }\n}\nfn main() {\n    let a = A { x: 1 };\n    println!(\"{}\", a.missing);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 24 && d[0].1.contains("reached the recursion limit while auto-dereferencing `A` (E0055)"), "{d:?}");
}

/// An impl's method against its trait through the header's arguments: `To` of `fn get_relation(&self) -> To` is the
/// header's `&ProofReader`, a lifetime of the impl that `&self` is not known to outlive (traits/param-without-
/// lifetime-constraint.rs); a header's `&'static` is not compared, and an impl's `&'static` input is less general
/// (E0308 in rustc). A struct in a fn's signature with another number of lifetime arguments than it has is E0107
#[test]
fn part13_header_and_arity() {
    let rel = "struct Article {\n    proof_reader: ProofReader,\n}\nstruct ProofReader {\n    name: String,\n}\npub trait HaveRelationship<To> {\n    fn get_relation(&self) -> To;\n}\n";
    let d = check(&format!("{rel}impl HaveRelationship<&ProofReader> for Article {{\n    fn get_relation(&self) -> &ProofReader {{\n        &self.proof_reader\n    }}\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("`impl` item signature doesn't match `trait` item signature"), "{d:?}");
    assert_eq!(check(&format!("{rel}impl HaveRelationship<&'static ProofReader> for Article {{\n    fn get_relation(&self) -> &'static ProofReader {{\n        static P: ProofReader = ProofReader {{ name: String::new() }};\n        &P\n    }}\n}}\nfn main() {{\n    let a = Article {{ proof_reader: ProofReader {{ name: String::new() }} }};\n    println!(\"{{}}\", a.get_relation().name.len() + a.proof_reader.name.len());\n}}\n")), vec![]);
    let take = "trait Take<T> {\n    fn take(&self, t: T) -> usize;\n}\nstruct S;\n";
    assert_eq!(check(&format!("{take}impl Take<&u8> for S {{\n    fn take(&self, t: &u8) -> usize {{\n        *t as usize\n    }}\n}}\nfn main() {{\n    let x = 3;\n    println!(\"{{}}\", S.take(&x));\n}}\n")), vec![]);
    let d = check(&format!("{take}impl Take<&u8> for S {{\n    fn take(&self, t: &'static u8) -> usize {{\n        *t as usize\n    }}\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("`impl` item signature doesn't match"), "{d:?}");
    let d = check("struct Foo<'c, 'd>(&'c (), &'d ());\nstruct Bar;\nimpl<'a> Bar {\n    fn boom(&self, foo: &mut Foo<'_, '_, 'a>) -> Option<&'a ()> {\n        let _ = foo;\n        None\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("struct takes 2 lifetime arguments but 3 lifetime arguments were supplied (E0107)"), "{d:?}");
    assert_eq!(check("struct Foo<'c, 'd>(&'c (), &'d ());\nfn f<'a>(x: &Foo<'a, 'a>) -> Foo<'a, 'a> {\n    Foo(x.0, x.1)\n}\nfn g(x: Foo) -> u8 {\n    let _ = x;\n    1\n}\nfn main() {\n    let v = ();\n    let a = Foo(&v, &v);\n    let _ = f(&a);\n    let _ = g(Foo(&v, &v));\n}\n"), vec![]);
}

/// `let _ = s;` in a closure reads nothing of s, so the closure does not capture s for it (edition 2021): a `move`
/// closure leaves s usable after it, and a `Box<dyn Fn()>` of it is `'static`. A typo there is still E0425, and a
/// `let _x = r;` captures r
#[test]
fn part13_wild_let_in_closure() {
    assert_eq!(check("fn main() {\n    let s = String::from(\"a\");\n    let c = move || {\n        let _ = s;\n    };\n    c();\n    println!(\"{}\", s);\n}\n"), vec![]);
    assert_eq!(check("fn keep(_b: Box<dyn Fn()>) {}\nfn main() {\n    let y = 3;\n    let r = &y;\n    keep(Box::new(move || {\n        let _ = r;\n    }));\n}\n"), vec![]);
    assert_eq!(check("struct P {\n    a: String,\n    b: u8,\n}\nfn main() {\n    let p = P { a: String::from(\"x\"), b: 1 };\n    let c = move || {\n        let _ = p.a;\n        p.b\n    };\n    println!(\"{} {}\", c(), p.a);\n}\n"), vec![]);
    let d = check("fn main() {\n    let c = || {\n        let _ = typo;\n    };\n    c();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("cannot find value `typo`"), "{d:?}");
    let d = check("fn keep(_b: Box<dyn Fn()>) {}\nfn main() {\n    let y = 3;\n    let r = &y;\n    keep(Box::new(move || {\n        let _x = r;\n    }));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`y` does not live long enough"), "{d:?}");
}

/// A block's tail's temporary whose `Drop` may see what it holds, holding a loan through another value (`id(rows)`,
/// `rows.wrap()`: what rows held) is dropped after the block's locals: E0597 (nll/issue-54556-stephaneyfx.rs). A
/// constructor's call with an argument known when it runs (`&Some(s)`, `&W(v)`) is a temporary, not promoted: E0716
#[test]
fn part13_tail_and_ctor_temps() {
    let rows = "pub struct Statement;\npub struct Rows<'stmt>(&'stmt Statement);\nimpl<'stmt> Drop for Rows<'stmt> {\n    fn drop(&mut self) {}\n}\nimpl<'stmt> Rows<'stmt> {\n    fn wrap(self) -> Rows<'stmt> {\n        self\n    }\n    fn len(&self) -> usize {\n        0\n    }\n}\nfn id<T>(t: T) -> T {\n    t\n}\n";
    for tail in ["id(rows).len()", "rows.wrap().len()"] {
        let d = check(&format!("{rows}fn get() -> usize {{\n    let stmt = Statement;\n    let rows = Rows(&stmt);\n    {tail}\n}}\nfn main() {{\n    println!(\"{{}}\", get());\n}}\n"));
        assert!(d.len() == 1 && d[0].0 == 19 && d[0].1.contains("`stmt` does not live long enough"), "{tail}: {d:?}");
    }
    // (the tail's value is not a temporary that outlives the locals: `let n = ..; n`)
    assert_eq!(check(&format!("{rows}fn get() -> usize {{\n    let stmt = Statement;\n    let rows = Rows(&stmt);\n    let n = id(rows).len();\n    n\n}}\nfn main() {{\n    println!(\"{{}}\", get());\n}}\n")), vec![]);
    let d = check("fn f(x: &Option<String>) -> &Option<String> {\n    x\n}\nfn main() {\n    let s = String::new();\n    let r = f(&Some(s));\n    println!(\"{:?}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("temporary value dropped while borrowed"), "{d:?}");
    assert_eq!(check("struct W(u32);\nfn f(x: &W) -> &W {\n    x\n}\nfn g(x: &Option<u32>) -> &Option<u32> {\n    x\n}\nfn main() {\n    let r = f(&W(3));\n    let o = g(&Some(4));\n    let v = 5;\n    let k = &Some(v);\n    println!(\"{} {:?} {:?}\", r.0, o, k);\n}\n"), vec![]);
}

/// `#![deny(mismatched_lifetime_syntaxes)]`: an output that writes an input's lifetime another way (`&'a u8` in,
/// `&u8` out; `&u8` in, `S` out) is an error, at the item's line; the same syntax both ways is not. Denied on one
/// item only, the others are not checked. `use<'_>` names the one input lifetime, not a missing one
#[test]
fn part13_mismatched_lifetime_syntaxes() {
    let d = check("#![deny(mismatched_lifetime_syntaxes)]\nfn f<'a>(x: &'a u8) -> &u8 {\n    x\n}\nfn main() {\n    let v = 1;\n    let _ = f(&v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("eliding a lifetime that's named elsewhere is confusing"), "{d:?}");
    let d = check("#![deny(mismatched_lifetime_syntaxes)]\nstruct S<'a>(&'a u8);\nfn g(x: &u8) -> S {\n    S(x)\n}\nfn main() {\n    let v = 1;\n    let _ = g(&v).0;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("hiding a lifetime that's elided elsewhere is confusing"), "{d:?}");
    assert_eq!(check("#![deny(mismatched_lifetime_syntaxes)]\nstruct S<'a>(&'a u8);\nfn f<'a>(x: &'a u8) -> &'a u8 {\n    x\n}\nfn h(x: &u8) -> &u8 {\n    x\n}\nfn k(x: &u8) -> S<'_> {\n    S(x)\n}\nfn s<'a>(_x: &'a u8) -> &'static u8 {\n    &1\n}\nfn main() {\n    let v = 1;\n    let _ = (f(&v), h(&v), k(&v).0, s(&v));\n}\n"), vec![]);
    let d = check("fn f<'a>(x: &'a u8) -> &u8 {\n    x\n}\n#[deny(mismatched_lifetime_syntaxes)]\nfn g<'a>(x: &'a u8) -> &u8 {\n    x\n}\nfn main() {\n    let v = 1;\n    let _ = (f(&v), g(&v));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("eliding a lifetime that's named elsewhere is confusing"), "{d:?}");
    assert_eq!(check("fn f<'a>(x: &'a u8) -> &u8 {\n    x\n}\nfn main() {\n    let v = 1;\n    let _ = f(&v);\n}\n"), vec![]);
    assert_eq!(check("fn e<'a>(v: &'a u8) -> impl FnOnce() + use<'_> {\n    move || {\n        let _ = v;\n    }\n}\nfn main() {\n    let x = 1;\n    e(&x)();\n}\n"), vec![]);
}

/// A closure's `let p: &'static mut usize = &mut self.food;`: the function is checked from the closure on (as for its
/// own `'static` annotation), so what the closure reaches through `self` is borrowed for less than `'static`; a
/// `'static` value there is fine
#[test]
fn part13_static_let_in_closure() {
    let dog = "struct Dog {\n    food: usize,\n}\nimpl Dog {\n    pub fn chase(&mut self) {\n        let _f = || {\n            let p: &'static mut usize = &mut self.food;\n            *p = 3;\n        };\n    }\n}\nfn main() {}\n";
    let d = check(dog);
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("fn f(x: &usize) {\n    let c = || {\n        let p: &'static usize = x;\n        let _ = *p;\n    };\n    c();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("fn f(x: &usize) {\n    let c = || {\n        let p: &'static str = \"lit\";\n        let q: &'static [u8] = &[1, 2];\n        p.len() + q.len() + *x\n    };\n    println!(\"{}\", c());\n}\nfn main() {\n    f(&1);\n}\n"), vec![]);
}

/// Where it is reported: a call that requires `T: 'static` of a function the check left out until there, with no
/// argument (`s::<T>()`), at the call, not at the function's first line; a closure's capture written through a `&`
/// (`z.0.0.0 = ..` with `z.0.0: &(String, String)`), at the closure, as rustc does
#[test]
fn part13_report_lines() {
    let d = check("fn s<T: 'static>() {}\nfn test<T>() {\n    let _ = 1;\n    s::<T>();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("the parameter type `T` may not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let mut x = (String::new(), String::new());\n    let mut y = (&x, 1);\n    let z = (&mut y, 2);\n    let mut c = || {\n        z.0.0.0 = String::from(\"a\");\n    };\n    c();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("is behind a `&` reference"), "{d:?}");
}

/// A `&mut` local: `let b = a;` moves it (the type inference's type is no annotation), so a later use of a is E0382;
/// `let b: &mut T = a;` reborrows `*a`, so a use of `*a` while b lives is E0506. A method's `&mut self` is taken
/// at the call, while the arguments are live: `v.ext(&v)` is E0502 (two-phase borrows)
#[test]
fn part13_mut_ref_moves_and_two_phase() {
    let d = check("fn f(s: &mut String) {\n    let alias = s;\n    let _n = s.len();\n    alias.push('a');\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("use of moved value: `s`"), "{d:?}");
    let d = check("fn f(s: &mut String) {\n    let alias: &mut String = s;\n    *s = String::new();\n    alias.push('a');\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("cannot assign to `*s` because it is borrowed"), "{d:?}");
    let d = check("struct S(Vec<i32>);\nimpl S {\n    fn ext(&mut self, _b: &S) {}\n}\nfn main() {\n    let mut v = S(vec![0]);\n    v.ext(&v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot borrow `v` as mutable because it is also borrowed as immutable"), "{d:?}");
    let ok = "struct S(Vec<i32>);\nimpl S {\n    fn ext(&mut self, _b: &S) {}\n    fn add(&mut self, n: usize) {\n        self.0.push(n as i32);\n    }\n    fn len(&self) -> usize {\n        self.0.len()\n    }\n}\nfn walk<'a>(mut cur: &'a mut Vec<i32>, other: &'a mut Vec<i32>) {\n    let r: &mut Vec<i32> = cur;\n    r.push(1);\n    cur.push(2);\n    let t: &mut Vec<i32> = other;\n    t.push(3);\n    cur = t;\n    cur.push(4);\n}\nfn moves(a: &mut Vec<u8>) {\n    let b = a;\n    b.push(1);\n    let c = b;\n    c.push(2);\n}\nfn main() {\n    let mut v = S(vec![]);\n    let w = S(vec![1]);\n    v.ext(&w);\n    v.add(v.len());\n    let mut a = vec![1];\n    let mut b = vec![2];\n    walk(&mut a, &mut b);\n    let mut x = vec![1, 2];\n    let y = &mut x;\n    let z: &mut Vec<i32> = y;\n    z.push(3);\n    y.push(4);\n    let mut u = vec![];\n    moves(&mut u);\n    println!(\"{} {:?} {:?} {:?}\", v.len(), a, x, u);\n}\n";
    assert_eq!(check(ok), vec![]);
}

/// `v.extend(&v)` / `v.extend(shared)` with `shared = &v`: the iterator the argument becomes holds a loan of v when
/// v's `&mut` is taken (E0502, borrowck/two-phase-reservation-sharing-interference-2.rs). A closure that calls a
/// captured `FnMut` closure borrows it mutably, so is `FnMut` itself: neither may be called through a binding not
/// declared `mut` (unboxed-closures/unboxed-closures-infer-fnmut-calling-fnmut-no-mut.rs)
#[test]
fn part13_extend_and_fnmut_calls() {
    let d = check("fn main() {\n    let mut v = vec![0, 1, 2];\n    let shared = &v;\n    v.extend(shared);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("cannot borrow `v` as mutable because it is also borrowed as immutable"), "{d:?}");
    let d = check("fn main() {\n    let mut v = vec![0, 1, 2];\n    v.extend(&v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("cannot borrow `v` as mutable"), "{d:?}");
    let d = check("fn main() {\n    let mut counter = 0;\n    let tick1 = || {\n        counter += 1;\n    };\n    let tick2 = || {\n        tick1();\n    };\n    let _ = tick2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot borrow `tick1` as mutable, as `tick1` is not declared as mutable"), "{d:?}");
    let d = check("fn main() {\n    let mut counter = 0;\n    let mut tick1 = || {\n        counter += 1;\n    };\n    let tick2 = || {\n        tick1();\n    };\n    tick2();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("cannot borrow `tick2` as mutable"), "{d:?}");
    let ok = "fn main() {\n    let mut v = vec![1, 2];\n    let w = vec![3];\n    v.extend(w.iter());\n    v.extend(v.clone());\n    v.extend(v.iter().copied().collect::<Vec<_>>());\n    v.extend(&w);\n    let mut s = String::new();\n    let o = String::from(\"ab\");\n    s.extend(o.chars());\n    let mut counter = 0;\n    let mut tick1 = || {\n        counter += 1;\n    };\n    let mut tick2 = || {\n        tick1();\n    };\n    tick2();\n    let base = 3;\n    let read = || base + 1;\n    let call = || read() + 1;\n    let mut acc = vec![];\n    let mut push = |x: i32| acc.push(x);\n    let mut twice = |x: i32| {\n        push(x);\n        push(x);\n    };\n    twice(call());\n    println!(\"{:?} {} {} {:?}\", v, s, counter, acc);\n}\n";
    assert_eq!(check(ok), vec![]);
}

/// A reference copied out of what a call's result points into (`*names.first().unwrap()`, `m.first().copied()` with
/// a `Vec<&str>`) holds what the vector holds, not the borrow of the vector: clearing it while the copy lives is fine.
/// Dropping what the copy points to is not, and neither is a loan of the referent itself (`*id(&&s)`)
#[test]
fn part14_copied_out_references() {
    assert_eq!(check("fn main() {\n    let a = String::from(\"a\");\n    let mut names: Vec<&str> = vec![&a];\n    let t = *names.first().unwrap();\n    names.clear();\n    let u = names.first().copied().unwrap_or(t);\n    names.clear();\n    let w = *names.first().unwrap_or(&\"x\");\n    names.clear();\n    println!(\"{} {} {}\", t, u, w);\n}\n"), vec![]);
    assert_eq!(check("struct S<'a> {\n    items: Vec<&'a str>,\n}\nimpl<'a> S<'a> {\n    fn go(&mut self) -> &'a str {\n        let t = *self.items.first().unwrap();\n        self.items.clear();\n        t\n    }\n}\nfn main() {\n    let a = String::from(\"a\");\n    let mut s = S { items: vec![&a] };\n    println!(\"{}\", s.go());\n}\n"), vec![]);
    let d = check("fn main() {\n    let a = String::from(\"a\");\n    let names: Vec<&str> = vec![&a];\n    let t = *names.first().unwrap();\n    drop(a);\n    println!(\"{}\", t);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot move out of `a` because it is borrowed"), "{d:?}");
    let d = check("fn id<T>(t: T) -> T {\n    t\n}\nfn main() {\n    let s = String::from(\"a\");\n    let t = *id(&&s);\n    drop(s);\n    println!(\"{}\", t);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot move out of `s` because it is borrowed"), "{d:?}");
    // (an `index` call's element (`m[&1]` of a HashMap), a `Copied` iterator's item; the iterator itself still holds
    // the borrow)
    assert_eq!(check("use std::collections::HashMap;\nfn main() {\n    let a = String::from(\"a\");\n    let mut m: HashMap<u8, &str> = HashMap::new();\n    m.insert(1, &a);\n    let x = m[&1];\n    m.clear();\n    let mut w: Vec<&str> = vec![&a];\n    let z = w.iter().copied().next().unwrap();\n    w.clear();\n    println!(\"{} {}\", x, z);\n}\n"), vec![]);
    let d = check("fn main() {\n    let mut w: Vec<String> = vec![String::from(\"a\")];\n    let mut it = w.iter().cloned();\n    let z = it.next();\n    w.clear();\n    println!(\"{:?} {:?}\", z, it.next());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot borrow `w` as mutable"), "{d:?}");
}

/// From the review of #67: an item's lint attributes as lists (`#[deny(dead_code, mismatched_lifetime_syntaxes)]`,
/// `#[allow(.., ..)]`, `#[expect(..)]`), a type parameter named as a struct hides no lifetime, `let _ = x;` is a use for
/// the unused-variable lint, a `let .. else` block's `let _ = x;` and `'static` annotation in a closure, a program's own
/// `clone`, a skipped `let _ = s.missing;` (E0609), and `s.take(s.get())` (E0502: the argument's shared loan is held
/// until the `&mut self` is taken)
#[test]
fn part13_review() {
    let d = check("#[deny(dead_code, mismatched_lifetime_syntaxes)]\nfn f<'a>(x: &'a u8) -> &u8 {\n    x\n}\nfn main() {\n    let v = 1;\n    let _ = f(&v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("eliding a lifetime that's named elsewhere is confusing"), "{d:?}");
    assert_eq!(check("#![deny(mismatched_lifetime_syntaxes)]\nstruct S<'s>(&'s u8);\nfn f<'a, S>(x: &'a u8) -> (Option<S>, &'a u8) {\n    (None, x)\n}\nfn main() {\n    let v = 1;\n    let _ = f::<u8>(&v);\n    let _ = S(&v);\n}\n"), vec![]);
    assert_eq!(check("#![deny(mismatched_lifetime_syntaxes)]\n#[allow(dead_code, mismatched_lifetime_syntaxes)]\nfn f<'a>(x: &'a u8) -> &u8 {\n    x\n}\n#[expect(mismatched_lifetime_syntaxes)]\nfn g<'a>(x: &'a u8) -> &u8 {\n    x\n}\nfn main() {\n    let v = 1;\n    let _ = (f(&v), g(&v));\n}\n"), vec![]);
    assert_eq!(check("#![deny(unused_variables)]\nfn main() {\n    let x = 1;\n    let _ = x;\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let x = String::from(\"a\");\n    let o: Option<u8> = None;\n    let c = move || {\n        let Some(_y) = o else {\n            let _ = x;\n            return;\n        };\n    };\n    c();\n}\n"), vec![]);
    let d = check("fn f(x: &usize, o: Option<u8>) {\n    let c = || {\n        let Some(_y) = o else {\n            let _p: &'static usize = x;\n            return;\n        };\n    };\n    c();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("struct S;\nstatic GLOBAL: S = S;\nimpl S {\n    fn clone(&self) -> &'static S {\n        &GLOBAL\n    }\n}\nfn f() -> &'static S {\n    let local = S;\n    (&local).clone()\n}\nfn main() {\n    let _ = f();\n}\n"), vec![]);
    let d = check("struct P {\n    a: u8,\n}\nfn main() {\n    let s = P { a: 1 };\n    let c = || {\n        let _ = s.missing;\n    };\n    c();\n    let _ = s.a;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("no field `missing` on type `P` (E0609)"), "{d:?}");
    let d = check("struct S {\n    n: u8,\n}\nimpl S {\n    fn get(&self) -> &u8 {\n        &self.n\n    }\n    fn take(&mut self, _x: &u8) {}\n}\nfn main() {\n    let mut s = S { n: 1 };\n    s.take(s.get());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 12 && d[0].1.contains("cannot borrow `s` as mutable because it is also borrowed as immutable"), "{d:?}");
    assert_eq!(check("struct S {\n    n: u8,\n}\nimpl S {\n    fn get(&self) -> &u8 {\n        &self.n\n    }\n    fn take(&mut self, _x: &u8) {}\n    fn val(&self) -> u8 {\n        self.n\n    }\n    fn put(&mut self, _x: u8) {}\n}\nfn main() {\n    let mut s = S { n: 1 };\n    s.put(s.val());\n    s.put(*s.get());\n    let t = S { n: 2 };\n    s.take(t.get());\n    let a = String::from(\"a\");\n    let mut v: Vec<&str> = vec![&a];\n    v.push(v.first().unwrap().clone());\n    v.push(*v.last().unwrap());\n    v.push(v[0]);\n    let n = v.len();\n    v.truncate(n - 1);\n    let mut w = vec![1, 2];\n    w.push(w.len());\n    w.push(*w.last().unwrap());\n    let mut st = String::from(\"x\");\n    st.push_str(&st.clone());\n    println!(\"{} {:?} {:?} {}\", s.n, v, w, st);\n}\n"), vec![]);
    assert_eq!(check("trait Dup: Clone {}\n#[derive(Clone)]\nstruct W(u8);\nimpl Dup for W {}\nfn duplicate<T: Dup>(x: T) -> T {\n    (&x).clone()\n}\nfn main() {\n    let w = duplicate(W(3));\n    println!(\"{}\", w.0);\n}\n"), vec![]);
}

/// A local holding a reference passed to a `&mut self` method of what it may borrow (`let w = &v; v.ext(w)`): what
/// it holds is held until the `&mut` is taken (E0502). One that borrows something else is fine
#[test]
fn part14_two_phase_through_a_local() {
    let d = check("struct S(Vec<i32>);\nimpl S {\n    fn ext(&mut self, _b: &S) {}\n    fn add(&mut self, _b: &[i32]) {}\n}\nfn main() {\n    let mut v = S(vec![0]);\n    let w = &v;\n    v.ext(w);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("cannot borrow `v` as mutable because it is also borrowed as immutable"), "{d:?}");
    assert_eq!(check("struct S(Vec<i32>);\nimpl S {\n    fn ext(&mut self, _b: &S) {}\n    fn add(&mut self, _b: &[i32]) {}\n}\nfn main() {\n    let mut v = S(vec![0]);\n    let u = S(vec![1]);\n    let w = &u;\n    v.ext(w);\n    let x = vec![3];\n    let r = &x;\n    v.add(r);\n    let a = String::from(\"a\");\n    let mut names: Vec<&str> = vec![];\n    let s: &str = &a;\n    names.push(s);\n    let t = names[0];\n    names.push(t);\n    println!(\"{}\", v.0.len() + names.len());\n}\n"), vec![]);
}

/// `[_, ref tail @ ..]` of an array borrows the elements between: tail holds the loan (E0506 on `a[2] = 0` while it
/// lives, borrowck/borrowck-vec-pattern-move-tail.rs); once tail is not used, the array may be written
#[test]
fn part14_ref_rest_binding() {
    let d = check("fn main() {\n    let mut a = [1, 2, 3, 4];\n    let t = match a {\n        [_, ref tail @ ..] => tail,\n    };\n    a[2] = 0;\n    println!(\"{}\", t[0]);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("because it is borrowed"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut a = [1, 2, 3, 4];\n    let t = match a {\n        [_, ref tail @ ..] => tail.len(),\n    };\n    a[2] = 0;\n    let [x, ref rest @ .., y] = a;\n    let n = rest.len();\n    a[0] = 9;\n    println!(\"{} {} {} {} {:?}\", t, x, y, n, a);\n}\n"), vec![]);
}

/// A closure's call of a fn that requires `T: 'static` of its function's T (nll/issue-98693.rs): E0310 when no bound
/// of the function gives it (`T: 'static`, a where clause, a trait whose `Self` is `'static`)
#[test]
fn part14_static_param_in_closure() {
    let d = check("fn s2<T: 'static>() {}\nfn test2<T>() {\n    let c = || {\n        s2::<T>();\n    };\n    c();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("the parameter type `T` may not live long enough"), "{d:?}");
    assert_eq!(check("use std::any::Any;\nfn s2<T: 'static>() {}\nfn a<T: 'static>() {\n    let c = || s2::<T>();\n    c();\n}\nfn b<T>()\nwhere\n    T: 'static,\n{\n    let c = || s2::<T>();\n    c();\n}\nfn d<T: Any>() {\n    let c = || s2::<T>();\n    c();\n}\ntrait St: 'static {}\nfn e<T: St>() {\n    let c = || s2::<T>();\n    c();\n}\nfn g<T: Send + 'static>(t: T) {\n    let c = move || {\n        std::thread::spawn(move || drop(t));\n    };\n    c();\n}\nfn main() {\n    a::<u8>();\n    b::<u8>();\n\n    g(1u8);\n}\n"), vec![]);
}

/// An `impl Trait + 'static` result: its hidden type's regions are 'static, whatever the parameters are
/// (impl-trait/type_parameters_captured.rs): returning a T is E0310, a `&'a u8` "lifetime may not live long enough"
#[test]
fn part14_static_opaque_result() {
    let d = check("trait Any {}\nimpl<T> Any for T {}\nfn foo<T>(x: T) -> impl Any + 'static {\n    x\n}\nfn bar<T>(x: T) -> Box<dyn Any + 'static> {\n    Box::new(x)\n}\nfn baz<'a>(x: &'a u8) -> impl Any + 'static {\n    x\n}\nfn main() {}\n");
    assert!(d.len() == 3 && d[0].0 == 4 && d[0].1.contains("E0310") && d[1].0 == 7 && d[2].0 == 10 && d[2].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("use std::fmt::Display;\ntrait Any {}\nimpl<T> Any for T {}\nfn f() -> impl Fn() + 'static {\n    || {}\n}\nfn g(x: u8) -> impl Iterator<Item = u8> + 'static {\n    std::iter::once(x)\n}\nfn h<T: 'static>(x: T) -> impl Any + 'static {\n    x\n}\nfn k(s: &str) -> impl Display + 'static {\n    s.to_string()\n}\nfn m<'a>(x: &'a u8) -> impl Fn() -> u8 + 'static {\n    let v: u8 = *x;\n    move || v\n}\nfn n(v: &[u8]) -> impl Iterator<Item = u8> + 'static {\n    v.to_vec().into_iter()\n}\nstruct W;\nimpl W {\n    fn it(&self) -> impl Display + 'static {\n        5\n    }\n}\nfn main() {\n    f()();\n    let _ = g(1).count();\n    let _ = h(1u8);\n    println!(\"{} {} {} {}\", k(\"a\"), m(&3)(), n(&[1]).count(), W.it());\n}\n"), vec![]);
}

/// Before edition 2024, an `impl Trait` result whose bounds name no lifetime captures none: what it returns may not
/// borrow through a parameter's reference, nor be one (E0700; impl-trait/precise-capturing/forgot-to-capture-lifetime.rs).
/// What is copied or owned, and a `+ '_` or `Item = &'a u8` that names the lifetime, are fine
#[test]
fn part14_rpit_captures_no_lifetime() {
    let d = check("use std::fmt::Display;\nfn s(x: &[u8]) -> impl Iterator<Item = u8> {\n    x.iter().copied()\n}\nfn t(x: &u8) -> impl Display {\n    x\n}\nfn main() {}\n");
    assert!(d.len() == 2 && d[0].0 == 3 && d[1].0 == 6 && d[0].1.contains("E0700"), "{d:?}");
    assert_eq!(check("use std::fmt::Display;\nfn a(x: &[u8]) -> impl Iterator<Item = u8> {\n    x.to_vec().into_iter()\n}\nfn b(x: &str) -> impl Display {\n    x.to_string()\n}\nfn c(x: &u8) -> impl Display {\n    *x\n}\nfn d(x: &[u8]) -> impl Iterator<Item = u8> + '_ {\n    x.iter().copied()\n}\nfn e<'a>(x: &'a [u8]) -> impl Iterator<Item = &'a u8> {\n    x.iter()\n}\nfn f(x: &[u8]) -> impl Fn() -> usize {\n    let n: usize = x.len();\n    move || n\n}\nfn main() {\n    println!(\"{} {} {} {} {} {}\", a(&[1]).count(), b(\"x\"), c(&1), d(&[2]).count(), e(&[3]).count(), f(&[4])());\n}\n"), vec![]);
    // (a parameter whose regions are 'static: `x: &'static u8`, `'a: 'static`)
    assert_eq!(check("use std::fmt::Display;\nfn a(x: &'static u8) -> impl Display {\n    x\n}\nfn b<'a: 'static>(x: &'a [u8]) -> impl Iterator<Item = u8> {\n    x.iter().copied()\n}\nfn c(_y: &u8, x: &'static str) -> impl Display {\n    x\n}\nfn main() {}\n"), vec![]);
    let mut s = Session::new(Options { jobs: 1, no_cache: true, edition: 2024, ..Options::default() });
    assert!(s.check("use std::fmt::Display;\nfn s(x: &[u8]) -> impl Iterator<Item = u8> {\n    x.iter().copied()\n}\nfn t(x: &u8) -> impl Display {\n    x\n}\nfn main() {}\n").is_empty());
}

/// A generic associated type's parameters named as its trait's or impl's (generic-associated-types/shadowing.rs):
/// E0496 for a lifetime, E0403 for a type parameter
#[test]
fn part15_gat_shadowing() {
    let d = check("trait Shadow<'a> {\n    type Bar<'a>;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0496"), "{d:?}");
    let d = check("trait NoShadowT<T> {\n    type Bar<U>;\n}\nimpl<T> NoShadowT<T> for Option<T> {\n    type Bar<T> = i32;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0403"), "{d:?}");
    assert_eq!(check("trait NoShadow<'a> {\n    type Bar<'b>;\n    fn f(&self) -> u8;\n}\nimpl<'a> NoShadow<'a> for &'a u32 {\n    type Bar<'b> = &'b u8;\n    fn f(&self) -> u8 {\n        1\n    }\n}\ntrait NoShadowT<T> {\n    type Bar<U>;\n}\nimpl<T> NoShadowT<T> for Option<T> {\n    type Bar<U> = U;\n}\ntrait WithSelf {\n    type Out<'x>\n    where\n        Self: 'x;\n}\nfn main() {\n    let x = 3u32;\n    println!(\"{}\", (&x).f());\n}\n"), vec![]);
}

/// E0261 in a signature's `impl` and `dyn` types, where the item writes no `for<..>` (which the parser does not keep):
/// `Box<dyn X<Y<'a> = &'a ()>>` with no `'a` (generic-associated-types/issue-67510.rs), `impl Iterator<Item =
/// Self::Item<'missing>>` (impl-trait/in-trait/missing-lt-outlives-in-rpitit-114274.rs)
#[test]
fn part15_undeclared_in_impl_and_dyn() {
    let d = check("trait X {\n    type Y<'a>;\n}\nfn f(x: Box<dyn X<Y<'a> = &'a ()>>) {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("use of undeclared lifetime name `'a` (E0261)"), "{d:?}");
    let d = check("fn g(x: &u8) -> impl std::fmt::Display + 'b {\n    *x\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 1 && d[0].1.contains("E0261"), "{d:?}");
    assert_eq!(check("use std::fmt::Display;\nfn f<'a>(x: &'a u8) -> impl Fn() -> u8 + 'a {\n    move || *x\n}\nfn g(x: &dyn Fn(&u8) -> u8) -> u8 {\n    x(&1)\n}\nfn h(x: Box<dyn Fn() -> u8 + 'static>) -> u8 {\n    x()\n}\nfn k<'a>(x: impl Iterator<Item = &'a u8>) -> usize {\n    x.count()\n}\nstruct S<'a>(&'a [u8]);\nimpl<'a> S<'a> {\n    fn it(&self) -> impl Iterator<Item = &'a u8> + '_ {\n        self.0.iter()\n    }\n    fn d(&self) -> Box<dyn Display + 'a> {\n        Box::new(self.0.len())\n    }\n}\ntrait T<'a> {\n    fn m(&self) -> Option<Box<dyn Display + 'a>>;\n}\nfn u<'a>(x: &'a u8) -> impl Display + use<'a> {\n    x\n}\nfn main() {\n    let v = [1u8];\n    let s = S(&v);\n    println!(\"{} {} {} {} {} {} {}\", f(&1)(), g(&|x| *x), h(Box::new(|| 2)), k(v.iter()), s.it().count(), s.d(), u(&3));\n}\n"), vec![]);
    // (a `for<..>` in the item: not checked)
    assert_eq!(check("fn h(f: &dyn for<'a> Fn(&'a u8) -> &'a u8) -> u8 {\n    *f(&1)\n}\nfn main() {\n    println!(\"{}\", h(&|x| x));\n}\n"), vec![]);
}

/// E0261 in a where clause's type (`&'missing Self: Add<Self>`, traits/eval-caching-error-region.rs), at that
/// predicate, where the item writes no `for<..>`
#[test]
fn part15_undeclared_in_where_types() {
    let d = check("use std::ops::Add;\npub trait Numoid: Sized\nwhere\n    &'missing Self: Add<Self>,\n{\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("use of undeclared lifetime name `'missing` (E0261)"), "{d:?}");
    assert_eq!(check("use std::ops::Add;\ntrait Numoid<'a>: Sized\nwhere\n    &'a Self: Add<Self>,\n{\n}\nfn f<'a, T>(x: &'a T) -> usize\nwhere\n    &'a T: IntoIterator,\n    Vec<&'a T>: Clone,\n{\n    let _ = x;\n    1\n}\nstruct W<'a, T>(&'a T)\nwhere\n    &'a T: Clone;\nfn main() {\n    println!(\"{}\", f(&vec![1u8]));\n}\n"), vec![]);
}

/// A slice's pattern elements are places, counted from the pattern's ends as rustc's MIR does
/// (borrowck/borrowck-slice-pattern-element-loan-slice.rs): the b-th from the start and the e-th from the end may be
/// one element when b + e reaches the least length both patterns need; a rest `ref mut t @ ..` is `&mut [T]` and holds
/// the elements it may be
#[test]
fn part15_slice_pattern_elements() {
    let conflict = |pats: (&str, &str)| {
        let src = format!("fn nop(_s: &[&i32]) {{}}\nfn f(s: &mut [i32]) {{\n    if let {} = *s {{\n        if let {} = *s {{\n            nop(&[a, b]);\n        }}\n    }}\n}}\nfn main() {{}}\n", pats.0, pats.1);
        let d = check(&src);
        assert!(d.len() <= 1 && d.iter().all(|x| x.0 == 4 && x.1.contains("cannot borrow `s[..]` as mutable")), "{src}\n{d:?}");
        !d.is_empty()
    };
    // (the same index from one end; from both ends within the least length)
    assert!(conflict(("[_, ref a, ..]", "[_, ref mut b, ..]")));
    assert!(conflict(("[.., ref a, _]", "[.., ref mut b, _]")));
    assert!(conflict(("[.., ref a, _, _, _]", "[_, ref mut b, ..]")));
    // (apart: other indexes from one end, and from both ends where the least length keeps them apart: of 3 or more
    // elements, the second from the end is not the first; of 4, the fourth from the end may be)
    assert!(!conflict(("[ref a, ..]", "[_, ref mut b, ..]")));
    assert!(!conflict(("[.., ref a]", "[.., ref mut b, _]")));
    assert!(conflict(("[ref a, ..]", "[.., ref mut b, _, _, _]")));
    assert!(!conflict(("[ref a, _, _, ..]", "[.., ref mut b, _]")));
    // (rests: `[_, ref mut t @ ..]` holds the second from the start, not the first; `[ref mut t @ .., _]` not the last)
    let rest = |a: &str, b: &str| check(&format!("fn f(s: &mut [i32]) {{\n    if let {a} = *s {{\n        if let {b} = *s {{\n            t[0] = *x;\n        }}\n    }}\n}}\nfn main() {{}}\n"));
    assert!(rest("[_, ref x, ..]", "[_, ref mut t @ ..]").iter().any(|d| d.0 == 3));
    assert_eq!(rest("[ref x, ..]", "[_, ref mut t @ ..]"), vec![]);
    assert!(rest("[.., ref x, _]", "[ref mut t @ .., _]").iter().any(|d| d.0 == 3));
    assert_eq!(rest("[.., ref x]", "[ref mut t @ .., _]"), vec![]);
    let d = check("fn f(s: &mut [i32]) {\n    if let [_, _, ref x @ ..] = *s {\n        if let [ref mut t @ .., _] = *s {\n            t[0] = x[0];\n        }\n    }\n}\nfn main() {}\n");
    assert!(d.iter().any(|d| d.0 == 3), "{d:?}");
    // (`ref mut t @ ..` of an array is `&mut [T]`)
    assert_eq!(check("fn main() {\n    let mut a = [1, 2, 3];\n    if let [ref mut t @ .., _] = a {\n        t[0] = 5;\n    }\n    println!(\"{}\", a[0]);\n}\n"), vec![]);
}

/// A fn named like a braced struct (`fn AddFlags(..) -> AddFlags`): its call makes a temporary, not a constructor's
/// value promoted to a static (borrowck/rvalue-borrow-scope-error.rs, E0716)
#[test]
fn part15_fn_named_like_a_struct() {
    let d = check("struct AddFlags {\n    bits: u64,\n}\n#[allow(non_snake_case)]\nfn AddFlags(bits: u64) -> AddFlags {\n    AddFlags { bits }\n}\nfn arg(x: &AddFlags) -> &AddFlags {\n    x\n}\nfn main() {\n    let x1 = arg(&AddFlags(1));\n    let y: &'static AddFlags = &AddFlags(2);\n    (x1, y);\n}\n");
    assert!(d.iter().any(|x| x.0 == 12 && x.1.contains("E0716")) && d.iter().all(|x| x.0 == 12 || x.0 == 13), "{d:?}");
    assert_eq!(check("struct W(u8);\nfn arg(x: &W) -> &W {\n    x\n}\nfn main() {\n    let x1 = arg(&W(1));\n    let y: &'static W = &W(2);\n    println!(\"{} {}\", x1.0, y.0);\n}\n"), vec![]);
}

/// Before edition 2021 a closure captures whole variables (nll/issue-51268.rs): `|| x.number += 1` holds all of x,
/// so reading `x.thing` while it lives is E0503; a by-reference capture in a closure's body stays of `*x`
/// (nll/issue-40510-4.rs)
#[test]
fn part15_whole_variable_captures() {
    let src = "struct Foo {\n    thing: u8,\n    number: usize,\n}\nfn f(x: &mut Foo) {\n    let c = || x.number += 1;\n    let d = x.thing;\n    let _ = c;\n    let _ = d;\n}\nfn main() {}\n";
    let d = check_ed(src, 2018);
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
    assert_eq!(check_ed(src, 2021), vec![]);
    assert_eq!(check_ed("fn f() {\n    let x: Vec<()> = Vec::new();\n    || {\n        || {\n            x.len()\n        }\n    };\n}\nfn main() {}\n", 2015), vec![]);
}

/// `copied` / `cloned` of an iterator: the adapter holds the iterator and its borrow (E0700 of
/// `s.0.iter().copied()` returned as `impl Iterator<Item = u8>`, nll/issue-73159-rpit-static.rs; E0502 of
/// `v.push(1)` while `v.iter().copied()` lives), and what is taken out of it, also through other adapters
/// (`.filter(..).collect()`), is the copies, which do not hold it
#[test]
fn part15_iterator_copied() {
    let d = check_ed("struct Foo<'a>(&'a [u8]);\nstruct V(Vec<u8>);\nfn a(x: &[u8]) -> impl Iterator<Item = u8> { x.iter().copied() }\nfn b<'a>(s: &Foo<'a>) -> impl Iterator<Item = u8> { s.0.iter().copied() }\nfn c(s: &V) -> impl Iterator<Item = u8> { s.0.iter().copied() }\nfn d(s: &V) -> impl Iterator<Item = &u8> { s.0.iter() }\nfn e(s: &V) -> impl Iterator<Item = u8> { s.0.iter().map(|x| *x) }\nfn main() {}\n", 2021);
    let mut lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    lines.dedup();
    assert!(lines == vec![3, 4, 5, 7] && d.iter().all(|x| x.1.contains("E0700")), "{d:?}");
    let d = check_ed("fn ok1(mut names: Vec<&str>) -> Vec<&str> {\n    let w: Vec<&str> = names.iter().copied().collect();\n    names.clear();\n    w\n}\nfn ok2(mut names: Vec<&str>) -> usize {\n    let t = names.iter().copied().next().unwrap();\n    names.clear();\n    t.len()\n}\nfn ok3(mut names: Vec<&str>) -> usize {\n    let w: Vec<&str> = names.iter().copied().filter(|s| !s.is_empty()).collect();\n    names.clear();\n    w.len()\n}\nfn ok4(mut names: Vec<&str>) -> usize {\n    let t = names.first().copied();\n    names.clear();\n    t.map_or(0, |s| s.len())\n}\nfn ok5(mut v: Vec<String>) -> usize {\n    let w: Vec<String> = v.iter().cloned().collect();\n    v.clear();\n    w.len()\n}\nfn bad1(mut v: Vec<u8>) -> usize {\n    let it = v.iter().copied();\n    v.push(1);\n    it.count()\n}\nfn bad2(mut v: Vec<u8>) -> usize {\n    let it = v.iter().copied().map(|x| x + 1);\n    v.push(1);\n    it.count()\n}\nfn main() {}\n", 2021);
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert_eq!(lines, vec![28, 33], "{d:?}");
    assert_eq!(check_ed("struct Foo<'a>(&'a [u8]);\nstruct S { r: &'static [u8], v: Vec<u8> }\nfn a(s: &Foo<'static>) -> impl Iterator<Item = u8> { s.0.iter().copied() }\nfn b(s: &S) -> impl Iterator<Item = u8> { s.r.iter().copied() }\nfn c<'a>(s: &Foo<'a>) -> impl Iterator<Item = u8> + 'a { s.0.iter().copied() }\nfn d(s: &S) -> impl Iterator<Item = u8> { s.v.clone().into_iter() }\nfn e<'a>(s: &Foo<'a>) -> impl Iterator<Item = u8> { s.0.to_vec().into_iter() }\nfn f<'a>(s: &Foo<'a>) -> impl std::fmt::Display { s.0.len() }\nfn g<'a>(s: &Foo<'a>) -> impl std::fmt::Display { s.0[0] }\nfn main() {}\n", 2021), vec![]);
}

/// unused_mut under `#![deny(unused_mut)]` (nll/issue-61424.rs, nll/capture-mut-ref.rs,
/// lint/unused/unused-mut-warning-captured-var.rs): a `let mut` that nothing needs mutable. Not a second assignment of
/// `let mut x;` that is assigned once, not a `move` closure that writes through the `&mut` it holds; and what needs it:
/// `&mut`, `+=`, a write of a part, a second assignment, a closure that mutates it, an array's methods
#[test]
fn part15_unused_mut() {
    let d = check("#![deny(unused_mut)]\nfn main() {\n    let mut x;\n    x = String::new();\n    println!(\"{}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("does not need to be mutable"), "{d:?}");
    let d = check("#![deny(unused_mut)]\npub fn f() {\n    let mut x = &mut 0;\n    let _ = move || {\n        *x = 1;\n    };\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    let d = check("#![forbid(unused_mut)]\nfn main() {\n    let mut x = 1;\n    (move || { println!(\"{}\", x); })();\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("#![deny(unused_mut)]\nuse std::collections::HashMap;\nuse std::fmt::Write as _;\nstruct P { n: u32 }\nimpl P { fn inc(&mut self) { self.n += 1; } }\nfn take(_: &mut [u8]) {}\nfn main() {\n    let mut v = vec![]; v.push(1);\n    let mut i = 0; while i < 3 { i += 1; }\n    let mut x; let mut k = 0; loop { x = 1; k += 1; if k > 2 { break; } } let _ = x;\n    let mut it = v.iter(); it.next();\n    let mut s = String::new(); s += \"a\"; s.push_str(\"b\"); write!(s, \"{}\", 1).unwrap();\n    let mut a = [0; 3]; a[1] = 2;\n    let mut t = (1, 2); t.0 = 3;\n    let mut y = 0; let mut c = || y += 1; c();\n    let mut count = 0; let mut inc = move || { count += 1; count }; inc();\n    let mut b = Box::new(1); *b = 2;\n    let mut vv = vec![0]; let mut r = &mut vv; r.push(2); let mut w = vec![0]; r = &mut w; r.push(3);\n    let mut z = 0; let zz = &mut z; *zz += 1;\n    let mut v2 = vec![1]; for e in v2.iter_mut() { *e += 1; }\n    let mut v3 = vec![1]; for e in &mut v3 { *e += 1; }\n    let mut p = P { n: 0 }; p.inc();\n    let mut rng = 0..3; while let Some(_) = rng.next() {}\n    let mut h = HashMap::new(); h.insert(1, 2); *h.entry(1).or_insert(0) += 1;\n    let mut buf = [0u8; 4]; buf.copy_from_slice(&[1, 2, 3, 4]);\n    let mut arr = [3, 2, 1]; arr.sort(); arr.reverse(); arr.swap(0, 1);\n    let mut arr2 = [1u8, 2]; take(&mut arr2);\n    let mut ve: Vec<u8> = vec![]; writeln!(&mut s, \"x\").unwrap(); ve.extend([1]);\n    let mut m = 5; m = 6; let _ = m;\n    let mut q = (vec![1], 2); q.0.push(3);\n    let mut o = Some(vec![1]); if let Some(ref mut l) = o { l.push(1); }\n    let mut o2 = Some(1); o2.take(); o2.replace(3);\n    let mut aa = 1; let mut bb = 2; std::mem::swap(&mut aa, &mut bb);\n    let mut st = String::from(\"x\"); st.clear(); st.truncate(0);\n    let mut sl = [1u8, 2, 3]; sl[..2].copy_from_slice(&[5, 6]);\n    let mut nested = vec![vec![1]]; nested[0].push(2);\n    let mut cl = vec![1]; let f = |x: u8| x; cl.retain(|&e| f(e) > 0);\n    let _ = (v, i, s, a, t, b, p, h, buf, arr, arr2, ve, q, o, o2, aa, bb, st, sl, nested, cl);\n}\n"), vec![]);
    assert_eq!(check("#![deny(unused_mut)]\nfn main() {\n    let mut a = [3, 2, 1]; a.swap(0, 1);\n    let mut b = [1, 2]; if let Some(x) = b.get_mut(0) { *x = 5; }\n\n\n    let mut e = [1, 2]; for x in e.iter_mut() { *x += 1; }\n    let mut f = [1, 2]; let _p = f.as_mut_ptr();\n    let mut g = [1, 2, 3, 4]; let (_l, _r) = g.split_at_mut(2);\n    let mut h = [1, 2]; for x in &mut h { *x += 1; }\n    let mut i = [1u8, 2]; i.fill(0);\n    let mut j = [[0u8; 2]; 2]; j[0][1] = 3;\n    let mut k = [1, 2]; k.iter_mut().for_each(|x| *x += 1);\n    let mut l = vec![1, 2]; l.swap(0, 1); l.iter_mut().for_each(|x| *x += 1);\n    let mut m = [1, 2]; m.chunks_mut(1).for_each(|c| c[0] = 0);\n    let _ = (a, b, e, f, h, i, j, k, l, m);\n}\n"), vec![]);
    // (an allow anywhere turns it off; no deny, no check)
    assert_eq!(check("#![deny(unused_mut)]\nfn main() {\n    #[allow(unused_mut)]\n    let mut x = 1;\n    let _ = x;\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let mut x = 1;\n    let _ = x;\n}\n"), vec![]);
}

/// Edition 2024: a block's tail temporaries go away at the end of the block (lifetimes/tail-expr-in-nested-expr.rs):
/// `{ String::new().as_str() }.len()` and `let x = { String::new().as_str() };` used after are E0716. A block whose
/// value holds no borrow of them is accepted; in 2021 `{ .. }.len()` is accepted (they live to the statement's end)
#[test]
fn part15_tail_temporaries_2024() {
    for src in ["fn main() {\n    let _ = { String::new().as_str() }.len();\n}\n", "fn main() {\n    let x = { String::new().as_str() };\n    let _ = x.len();\n}\n"] {
        let d = check_ed(src, 2024);
        assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0716"), "{src}\n{d:?}");
    }
    assert_eq!(check_ed("fn main() {\n    let _ = { String::new().as_str() }.len();\n}\n", 2021), vec![]);
    assert_eq!(check_ed("use std::cell::RefCell;\nfn a(v: &Vec<u8>) -> usize { { v.iter().count() } }\nfn b() -> usize { let n = { vec![1u8].iter().count() }; n }\nfn c() -> usize { let s = { String::from(\"ab\").as_str().len() }; s }\nfn d(c: &RefCell<Vec<u8>>) -> usize { let n = { c.borrow().len() }; n }\nfn e() -> String { let s = { String::from(\"x\").as_str().to_string() }; s }\nfn f(m: &std::collections::HashMap<u8, String>) -> usize { let n = { m.get(&1).map(|s| s.len()).unwrap_or(0) }; n }\nfn g() -> usize { let x = { let s = String::new(); s.len() }; x }\nfn main() { let _ = (a(&vec![]), b(), c(), e(), g()); }\n", 2024), vec![]);
}

/// From the third review of #69: a closure's call that needs `T: 'static` takes it from `T: 'a` with `'a: 'static`,
/// and from a supertrait whose `Self` must be 'static (`trait Derived: Base`, `trait Base: 'static`); a T in the type
/// argument (`needs_static::<Option<T>>()`) needs it too (E0310)
#[test]
fn part14_review2() {
    assert_eq!(check("fn needs_static<T: 'static>() {}\nfn f<'a, T>()\nwhere\n    T: 'a,\n    'a: 'static,\n{\n    let c = || needs_static::<T>();\n    c();\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("fn needs_static<T: 'static>() {}\ntrait Base: 'static {}\ntrait Derived: Base {}\nfn f<T: Derived>() {\n    let c = || needs_static::<T>();\n    c();\n}\nfn main() {}\n"), vec![]);
    let d = check("fn needs_static<T: 'static>() {}\nfn f<T>() {\n    let c = || needs_static::<Option<T>>();\n    c();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0310"), "{d:?}");
    assert_eq!(check("fn needs_static<T: 'static>() {}\nfn f<T: 'static>() {\n    let c = || needs_static::<Option<Box<T>>>();\n    c();\n}\nfn main() {}\n"), vec![]);
    // (a block argument whose value is such a local reserves the receiver too: `v.ext({ w })` is E0502)
    let d = check("struct V;\nimpl V {\n    fn ext(&mut self, _: &V) {}\n}\nfn main() {\n    let mut v = V;\n    let w = &v;\n    v.ext({ w });\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("cannot borrow `v` as mutable"), "{d:?}");
    assert_eq!(check("struct V;\nimpl V {\n    fn ext(&mut self, _: &V) {}\n}\nfn main() {\n    let mut v = V;\n    let u = V;\n    let w = &u;\n    v.ext({ w });\n    v.ext({ let x = &u; x });\n}\n"), vec![]);
}

/// From the review of #69: a parameter's reference bound to a local (`let y = x;`) is still captured by a no-capture
/// `impl Trait` (E0700); a parameter is 'static through outlives bounds (`'a: 'b, 'b: 'static`) and with an ADT of no
/// lifetime (`&'static String`); `_: &'static T` implies `T: 'static` for a closure's call
#[test]
fn part14_review() {
    let d = check("use std::fmt::Display;\nfn f(x: &u8) -> impl Display {\n    let y = x;\n    y\n}\nfn g(x: &[u8]) -> impl Iterator<Item = u8> {\n    let y = x;\n    y.iter().copied()\n}\nfn main() {}\n");
    assert!(d.iter().any(|x| x.0 == 4) && d.iter().any(|x| x.0 == 8) && d.iter().all(|x| x.0 == 4 || x.0 == 8), "{d:?}");
    assert_eq!(check("fn f<'a: 'b, 'b: 'static>(x: &'a u8) -> impl std::fmt::Display {\n    x\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("fn f(x: &'static String) -> impl std::fmt::Display {\n    x\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("fn needs_static<T: 'static>() {}\nfn f<T>(_: &'static T) {\n    let c = || needs_static::<T>();\n    c();\n}\nfn main() {}\n"), vec![]);
}

/// From the review of #71: `for` and its `<` apart (a line break, spaces) is a binder too; a binding the lowering makes
/// (`#sl0`) is not checked for unused_mut; an array's method that only reads it (`a.len()`) does not need it `mut`, nor
/// a method of a slice through a `&mut` binding (`s.sort()`)
#[test]
fn part15_review() {
    assert_eq!(check("fn f(_x: Box<dyn for\n<'a> Fn(&'a u8)>) {}\nfn g(_x: Box<dyn for  <'b> Fn(&'b u8)>) {}\nfn main() {\n    f(Box::new(|_| {}));\n    g(Box::new(|_| {}));\n}\n"), vec![]);
    assert_eq!(check("#![deny(unused_mut)]\nstruct Foo<T> { value: T }\nstatic X: u32 = 5;\nfn main() {\n    let Foo { value: &n } = Foo::<&'static u32> { value: &X };\n    println!(\"{}\", n);\n}\n"), vec![]);
    let d = check("#![deny(unused_mut)]\nfn main() {\n    let mut a = [1, 2];\n    println!(\"{}\", a.len());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("unused_mut"), "{d:?}");
    let d = check("#![deny(unused_mut)]\nfn main() {\n    let mut a = [3, 1, 2];\n    let mut s: &mut [i32] = &mut a;\n    s.sort();\n    println!(\"{:?}\", a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`s`"), "{d:?}");
    // (the second round: a `&mut self` method of the program's on an array takes it `&mut` (it was a type mismatch):
    // it needs the array `mut` (E0596 without), and so it is not unused_mut's)
    let tr = "trait Touch {\n    fn touch(&mut self);\n    fn look(&self) -> i32;\n}\nimpl Touch for [i32; 2] {\n    fn touch(&mut self) { self[0] += 1; }\n    fn look(&self) -> i32 { self[0] }\n}\n";
    let d = check(&format!("#![deny(unused_mut)]\n{tr}fn main() {{\n    let mut a = [0, 1];\n    a.touch();\n    let mut b = [0, 1];\n    println!(\"{{}} {{}}\", a.look(), b.look());\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 13 && d[0].1.contains("`b`"), "{d:?}");
    let d = check(&format!("{tr}fn main() {{\n    let a = [0, 1];\n    a.touch();\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("as mutable"), "{d:?}");
    // (the third round: an array of a type not `Copy` is borrowed by such a method, not moved; an `unsafe fn` of the
    // trait is E0133 outside `unsafe`; `#[allow (unused_mut)]` with a space allows it too; and in edition 2024 a block's
    // value that holds a reference into its tail's temporaries (`{ (String::new().as_str(),) }.0`) is E0716)
    assert_eq!(check("trait Touch {\n    fn touch(&mut self);\n    fn look(&self) -> usize;\n}\nimpl Touch for [String; 1] {\n    fn touch(&mut self) { self[0].push('y'); }\n    fn look(&self) -> usize { self[0].len() }\n}\nfn main() {\n    let mut a = [String::from(\"x\")];\n    a.touch();\n    println!(\"{} {}\", a.look(), a.look());\n}\n"), vec![]);
    let d = check("trait U {\n    unsafe fn danger(&self) -> i32;\n}\nimpl U for [i32; 2] {\n    unsafe fn danger(&self) -> i32 { self[0] }\n}\nfn main() {\n    let a = [1, 2];\n    println!(\"{}\", a.danger());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("E0133"), "{d:?}");
    assert_eq!(check("#![deny(unused_mut)]\nfn main() {\n    #[allow (unused_mut)]\n    let mut x = 1;\n    let _ = x;\n}\n"), vec![]);
    let src = "fn main() {\n    let n = { (String::new().as_str(),) }.0.len();\n    println!(\"{}\", n);\n}\n";
    let d = check_ed(src, 2024);
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check_ed(src, 2021), vec![]);
    // (the fourth round: such a `&self` method borrows the array, which its result holds (E0505 when the array is
    // moved while it is used); and `allow (unused_mut)` in a comment allows nothing)
    let d = check("trait Head {\n    fn head(&self) -> &String;\n}\nimpl Head for [String; 1] {\n    fn head(&self) -> &String { &self[0] }\n}\nfn main() {\n    let a = [String::from(\"x\")];\n    let r = a.head();\n    drop(a);\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("because it is borrowed"), "{d:?}");
    let d = check("#![deny(unused_mut)]\n// we allow (unused_mut) nowhere\nfn main() {\n    let mut x = 1;\n    let _ = x;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("unused_mut"), "{d:?}");
}

/// `&*b` of a `Box<dyn Tr>` / `Box<[T]>` / `Box<str>` borrows what the box owns, as of a `Box<T>`: moving b while it
/// lives is E0505, and returning it in a `Box<dyn X>` is E0515 (regions/regions-close-object-into-object-*.rs)
#[test]
fn part16_unsized_box_reborrow() {
    let d = check("trait A {}\nfn f(v: Box<dyn A>) {\n    let r = &*v;\n    drop(v);\n    let _ = r;\n}\nfn g(v: Box<[u8]>) {\n    let r = &*v;\n    drop(v);\n    let _ = r;\n}\nfn main() {}\n");
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert!(lines == vec![4, 9] && d.iter().all(|x| x.1.contains("because it is borrowed")), "{d:?}");
    let d = check("trait A {}\nstruct B<'a>(&'a (dyn A + 'a));\ntrait X {}\nimpl<'a> X for B<'a> {}\nfn f(v: Box<dyn A>) -> Box<dyn X> {\n    Box::new(B(&*v))\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
    // (`&mut` through a method's auto-ref: `b.iter_mut()` of a box not declared `mut`, `self.0.sort()` through `&self`
    // (E0596), at the receiver)
    let d = check("struct E(Box<[u8]>);\nimpl E {\n    fn g(&self) {\n        self.0.sort();\n    }\n}\nfn h(b: Box<[u8]>) -> usize {\n    b.iter_mut().count()\n}\nfn main() {}\n");
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert!(lines == vec![4, 8] && d.iter().all(|x| x.1.contains("as mutable")), "{d:?}");
    // (what is fine: the borrow ends before the move, a `&mut *b` of a `mut` box, a box of a `&'static`)
    assert_eq!(check("trait A { fn n(&self) -> u8; }\nimpl A for u8 { fn n(&self) -> u8 { *self } }\nfn f(v: Box<dyn A>) -> u8 {\n    let r = &*v;\n    let n = r.n();\n    drop(v);\n    n\n}\nfn g(mut v: Box<[u8]>) -> u8 {\n    let r = &mut *v;\n    r[0] = 1;\n    v[0]\n}\nfn h(v: Box<&'static str>) -> &'static str {\n    *v\n}\nfn main() {\n    println!(\"{} {} {}\", f(Box::new(3u8)), g(vec![0u8].into_boxed_slice()), h(Box::new(\"x\")));\n}\n"), vec![]);
}

/// `let ref x = e;` binds a reference to e's value (its type is `&T`, as an element of `[x]` too), and a temporary e
/// the `let` extends goes away with the block's locals: returning `x` of `let ref mut x = 1234543;` is E0515
/// (nll/return-ref-mut-issue-46557.rs), and `r = z;` of `let ref z = f();` in an inner block used after is E0716
#[test]
fn part16_let_ref_temporaries() {
    let d = check("fn g() -> &'static mut u32 {\n    let ref mut x = 1234543;\n    x\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0515"), "{d:?}");
    let d = check("fn f() -> u32 { 3 }\nfn main() {\n    let ref mut x = 1234543;\n    *x += 1;\n    let ref y = f();\n    let r;\n    {\n        let ref z = f();\n        r = z;\n    }\n    println!(\"{} {} {}\", x, y, r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("E0716"), "{d:?}");
    assert_eq!(check("fn h() -> &'static u32 {\n    let x = &(5u32 + 1);\n    x\n}\nfn k() -> &'static u32 {\n    let ref x = 5u32;\n    x\n}\nfn main() {\n    let ref init = 0i32;\n    let a: &i32 = init;\n    let out = [init];\n    println!(\"{} {:?} {} {}\", a, out, h(), k());\n}\n"), vec![]);
}

/// A call's `&mut place` argument whose referent shares a lifetime or a type parameter with another argument stores
/// what that argument borrows (`push(&x, &mut v)` of `fn push<'a>(r: &'a i32, v: &mut Vec<&'a i32>)`): `x = 2;` while
/// v is used is E0506 (borrowck/cannot-assign-borrowed-ref-in-slice.rs)
#[test]
fn part16_mut_argument_flows() {
    let d = check("fn push<'a>(r: &'a i32, v: &mut Vec<&'a i32>) { v.push(r) }\nfn save<'a>(r: &'a i32, to: &mut [&'a i32]) { to[0] = r; }\nfn put<T>(a: T, v: &mut Vec<T>) { v.push(a) }\nfn a() {\n    let mut v = vec![];\n    let mut x = 1;\n    push(&x, &mut v);\n    x = 2;\n    println!(\"{:?} {}\", v, x);\n}\nfn b() {\n    let init = 0i32;\n    let mut x = 1;\n    let mut out = [&init];\n    save(&x, &mut out);\n    x = 3;\n    println!(\"{:?} {}\", out, x);\n}\nfn c() {\n    let mut v = vec![];\n    let mut x = 1;\n    put(&x, &mut v);\n    x = 2;\n    println!(\"{:?} {}\", v, x);\n}\nfn main() { a(); b(); c(); }\n");
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert!(lines == vec![8, 16, 23] && d.iter().all(|x| x.1.contains("because it is borrowed")), "{d:?}");
    // (not through an elided or another lifetime, nor after the borrower's last use; behind a reference, what the
    // referent holds: `swap(&mut values, &mut x.values)` stores no loan of values in x (borrowck/borrowck-mut-uniq.rs))
    assert_eq!(check("struct Ints { values: Vec<isize> }\nfn add_int(x: &mut Ints, v: isize) {\n    let mut values = Vec::new();\n    std::mem::swap(&mut values, &mut x.values);\n    values.push(v);\n    std::mem::swap(&mut values, &mut x.values);\n}\nfn main() {\n    let mut i = Ints { values: vec![] };\n    add_int(&mut i, 1);\n    println!(\"{}\", i.values.len());\n}\n"), vec![]);
    let d = check("fn main() {\n    let mut x = 1;\n    let k = 0;\n    let mut a = &k;\n    let mut b = &x;\n    std::mem::swap(&mut a, &mut b);\n    x = 2;\n    println!(\"{} {}\", a, b);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
    assert_eq!(check("fn f(_a: &u8, _v: &mut Vec<&u8>) {}\nfn g<'a, 'b>(_a: &'a u8, _v: &mut Vec<&'b u8>) {}\nfn push<'a>(r: &'a u8, v: &mut Vec<&'a u8>) { v.push(r) }\nfn main() {\n    let k = 0u8;\n    let mut v: Vec<&u8> = vec![&k];\n    let mut x = 1u8;\n    f(&x, &mut v);\n    g(&x, &mut v);\n    x = 2;\n    let mut w = vec![];\n    push(&x, &mut w);\n    println!(\"{:?} {:?}\", v, w);\n    x = 3;\n    println!(\"{}\", x);\n}\n"), vec![]);
}

/// A call through a closure bound whose result borrows an argument by the bound's lifetimes holds that argument's loans:
/// `let y = f(&x); x = 5;` with `F: Fn(&isize) -> &isize` is E0506 (higher-ranked/trait-bounds/hrtb-identity-fn-borrows.rs
/// has the same shape through a trait of its own). Not a result of no reference, a `'static` one, nor one tied to
/// another argument
#[test]
fn part16_fn_bound_results() {
    let d = check("fn g<F: Fn(&isize) -> &isize>(f: F) {\n    let mut x = 3;\n    let y = f(&x);\n    x = 5;\n    drop(y);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("because it is borrowed"), "{d:?}");
    assert_eq!(check("fn g1<F: Fn(&isize) -> isize>(f: F) {\n    let mut x = 3;\n    let y = f(&x);\n    x = 5;\n    drop(y);\n}\nfn g2<F: for<'a> Fn(&'a isize, &isize) -> &'a isize>(f: F) {\n    let mut x = 3;\n    let z = 4;\n    let y = f(&z, &x);\n    x = 5;\n    drop(y);\n}\nfn g3<F: Fn(&isize) -> &'static isize>(f: F) {\n    let mut x = 3;\n    let y = f(&x);\n    x = 5;\n    drop(y);\n}\nfn g4<F: Fn(&isize) -> &isize>(f: F) {\n    let mut x = 3;\n    f(&x);\n    x = 5;\n    drop(x);\n}\nfn main() {}\n"), vec![]);
}

/// A view of a str / slice (`s.chars()`, `s.split(',')`) holds what the receiver does: `v0.chars()` returned as an
/// `impl Iterator<Item = char>` that captures no lifetime is E0700 (lifetimes/issue-105227.rs), and `t.chars()` of
/// `t: &str = &s` kept while `s.push('c')` is E0502
#[test]
fn part16_str_views() {
    let d = check("fn c1(v0: &str, v1: &str) -> impl Iterator<Item = char> {\n    v0.chars().chain(v1.chars())\n}\nfn c2(v0: &str) -> impl Iterator<Item = char> {\n    v0.chars()\n}\nfn main() {}\n");
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert!(lines == vec![2, 5] && d.iter().all(|x| x.1.contains("E0700")), "{d:?}");
    let d = check("fn main() {\n    let mut s = String::from(\"ab\");\n    let t: &str = &s;\n    let it = t.chars();\n    s.push('c');\n    println!(\"{}\", it.count());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
    assert_eq!(check("fn main() {\n    let mut s = String::from(\"ab\");\n    let n = s.as_str().chars().count();\n    s.push('x');\n    let w: Vec<&str> = s.split(',').collect();\n    println!(\"{} {:?}\", n, w);\n    let v = vec![1u8, 2];\n    let sl: &[u8] = &v;\n    let c: Vec<u8> = sl.iter().copied().collect();\n    drop(v);\n    println!(\"{:?}\", c);\n}\n"), vec![]);
}

/// Before edition 2024, a block's value that a statement discards (`{ let t1 = D(1); D("o").next(&t1) };`) is dropped at
/// the statement's end, after the block's locals: when its `Drop` may see a borrow of one, that local does not live long
/// enough (E0597, nll/issue-54382-use-span-of-tail-of-block.rs), also through an inner block. Not a value that borrows
/// none of them, nor one with no `Drop`
#[test]
fn part16_discarded_block_values() {
    let pre = "#[derive(Debug)]\nstruct D<T: std::fmt::Debug>(T);\nimpl<T: std::fmt::Debug> Drop for D<T> {\n    fn drop(&mut self) {\n        println!(\"dropping {:?}\", self);\n    }\n}\nimpl<T: std::fmt::Debug> D<T> {\n    fn next<U: std::fmt::Debug>(&self, o: U) -> D<U> { D(o) }\n}\n";
    let d = check(&format!("{pre}fn a() {{\n    {{\n        let t1 = D(1);\n        D(\"other\").next(&t1)\n    }};\n}}\nfn b() {{\n    {{\n        let t2 = D(2);\n        {{\n            D(\"other\").next(&t2)\n        }}\n    }};\n}}\nfn main() {{ a(); b(); }}\n"));
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert!(lines == vec![14, 21] && d.iter().all(|x| x.1.contains("does not live long enough")), "{d:?}");
    assert_eq!(check(&format!("{pre}struct P<'a>(&'a u8);\nfn main() {{\n    let k = D(0);\n    {{\n        let t1 = D(1);\n        D(\"o\").next(&k)\n    }};\n    {{\n        let t2 = 3u8;\n        P(&t2)\n    }};\n    {{\n        let t3 = D(3);\n        D(\"o\").next(5)\n    }};\n}}\n")), vec![]);
}

/// `&a` of a reference `a: &T` (or `&&T`) where a `&T` is expected is a deref coercion: the pointer a holds, a reborrow
/// of `*a`, not a borrow of the local a. As a result (`fn f(a: &i32) -> &i32 { &a }`, also in an `if`'s branches) it
/// was a type mismatch, and through a `let` or an argument (`g(&a)`) a false E0515. What a holds is still followed:
/// `let a = &x; let b = &a; &b` returned is E0515 of x
#[test]
fn part16_deref_coercion() {
    assert_eq!(check("fn f1(a: &i32) -> &i32 {\n    &a\n}\nfn f2<'x>(a: &'x i32, b: &'x i32) -> &'x i32 {\n    if *a < *b { &a } else { &b }\n}\nfn f3(a: &i32) -> &i32 {\n    let r: &i32 = &a;\n    r\n}\nfn g(x: &i32) -> &i32 {\n    x\n}\nfn f4(a: &i32) -> &i32 {\n    g(&a)\n}\nfn f5<'x>(a: &'x i32) -> &'x i32 {\n    let b = &a;\n    &b\n}\nfn h(x: &mut i32) {\n    *x += 1;\n}\nfn f6(mut a: &mut i32) {\n    h(&mut a);\n}\nfn main() {\n    let (x, y) = (1, 2);\n    let mut z = 3;\n    println!(\"{} {} {} {} {}\", f1(&x), f2(&x, &y), f3(&x), f4(&x), f5(&x));\n    f6(&mut z);\n}\n"), vec![]);
    // (`&mut a` borrows a mutably: E0596 when a is not `mut` (did_you_mean/issue-37139.rs), and what it reborrows
    // through it cannot be returned)
    let d = check("fn h(x: &mut i32) {\n    *x += 1;\n}\nfn main() {\n    let mut v = 1;\n    let ref mut x = v;\n    h(&mut x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("not declared as mutable"), "{d:?}");
    let d = check("fn f() -> &'static i32 {\n    let x = 1;\n    let a = &x;\n    let b = &a;\n    &b\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`x`"), "{d:?}");
    let d = check("fn main() {\n    let mut x = 1;\n    let mut a = &mut x;\n    let r: &mut i32 = &mut a;\n    let s = &x;\n    *r = 2;\n    println!(\"{}\", s);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
}

/// A guard of the prelude (`c.borrow()`'s `Ref`, a `MutexGuard`) releases what it borrows when it is dropped: one made
/// in a block's tail (`y.borrow().clone()`, `*a.borrow() + 1`), which lives to the end of the enclosing statement
/// before edition 2024, makes the block's local it borrows not live long enough (E0597,
/// span/issue-23338-locals-die-before-temps-of-body.rs, span/destructor-restrictions.rs). Not one made in a statement
#[test]
fn part16_guard_tail_temporaries() {
    let d = check("use std::cell::RefCell;\nfn foo(x: RefCell<String>) -> String {\n    let ret = {\n        let y = x;\n        y.borrow().clone()\n    };\n    ret\n}\nfn main() {\n    let b = {\n        let a = Box::new(RefCell::new(4));\n        *a.borrow() + 1\n    };\n    println!(\"{} {}\", b, foo(RefCell::new(String::new())));\n}\n");
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert!(lines == vec![5, 12] && d.iter().all(|x| x.1.contains("does not live long enough")), "{d:?}");
    assert_eq!(check("use std::cell::RefCell;\nfn main() {\n    let c = RefCell::new(vec![1]);\n    let n = c.borrow().len();\n    let g = c.borrow();\n    let m = g[0];\n    drop(g);\n    c.borrow_mut().push(2);\n    let t = {\n        let d = RefCell::new(5);\n        let v = *d.borrow();\n        v\n    };\n    println!(\"{} {} {}\", n, m, t);\n}\n"), vec![]);
}

/// A block's value that holds what one of its locals holds (`{ let x = 1; let y = &x; y }`, or a call's result through
/// a local, `f(a, b)` of `let f = &shorten_lifetime;`, nll/promoted-bounds.rs) used after the block: x does not live
/// long enough (E0597). It was checked only when a `let` took a tail that borrowed directly (`{ let x = 1; &x }`), as
/// the block's names are gone after it. Not a value that holds nothing of the block's (`{ let y = &outer; y }`)
#[test]
fn part16_block_values_through_locals() {
    let d = check("fn a() {\n    let r = { let x = 1; let y = &x; y };\n    println!(\"{}\", r);\n}\nfn s<'a, 'b, 'm>(a: &'a i32, b: &'b i32) -> &'m i32\nwhere\n    'a: 'm,\n    'b: 'm,\n{\n    if *a < *b { &a } else { &b }\n}\nfn b() {\n    let f = &s;\n    let a = &5;\n    let p = {\n        let l = 3;\n        let b = &l;\n        let c = f(a, b);\n        c\n    };\n    println!(\"{}\", p);\n}\nfn main() { a(); b(); }\n");
    let lines: Vec<usize> = d.iter().map(|x| x.0).collect();
    assert!(lines == vec![2, 17] && d.iter().all(|x| x.1.contains("does not live long enough")), "{d:?}");
    // (also where the value is used other than by a `let`: a match's scrutinee (span/issue-40157.rs); not by a
    // statement that discards it)
    let d = check("fn main() {\n    let n = match { let foo = vec![1, 2]; foo.get(1) } { x => x };\n    println!(\"{:?}\", n);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("`foo` does not live long enough"), "{d:?}");
    assert_eq!(check("fn main() {\n    let outer = 1;\n    { let x = 1; let y = &x; y };\n    let n = match { let v = vec![1]; v.len() } { k => k };\n    let m = match { let y = &outer; y } { k => *k };\n    println!(\"{} {}\", n, m);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let outer = 5;\n    let n = { let s = String::from(\"a\"); s.len() };\n    let r = { let y = &outer; y };\n    let v = vec![1, 2];\n    let w = { let it = &v; it.len() };\n    let z: &i32 = { let q = &v; &q[0] };\n    let t = { let k = (1, &outer); k };\n    println!(\"{} {} {} {} {:?}\", n, r, w, z, t);\n}\n"), vec![]);
}

/// A call's argument of a type parameter that outlives a lifetime the result names may be held by the result:
/// `scoped(|| { let _z = y; })` of `fn scoped<'a, F: Fn() + Send + 'a>(f: F) -> Guard<'a>`, a block's value used after
/// the block, holds what the closure holds (span/send-is-not-static-ensures-scoping.rs, E0597)
#[test]
fn part16_type_parameter_arguments() {
    let g = "struct Guard<'a> {\n    f: Box<dyn Fn() + Send + 'a>,\n}\nfn scoped<'a, F: Fn() + Send + 'a>(f: F) -> Guard<'a> {\n    Guard { f: Box::new(f) }\n}\nimpl<'a> Guard<'a> {\n    fn join(self) {}\n}\n";
    let d = check(&format!("{g}fn main() {{\n    let bad = {{\n        let x = 1;\n        let y = &x;\n        scoped(|| {{\n            let _z = y;\n        }})\n    }};\n    bad.join();\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 13 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    assert_eq!(check("struct G<'a> {\n    f: Box<dyn Fn() + 'a>,\n}\nfn scoped<'a, F: Fn() + 'a>(f: F) -> G<'a> {\n    G { f: Box::new(f) }\n}\nfn keep<T: Clone>(t: T) -> Vec<T> {\n    vec![t]\n}\nfn main() {\n    let x = 1;\n    let g = scoped(|| println!(\"{}\", x));\n    (g.f)();\n    let v = { let s = String::from(\"a\"); keep(s.len()) };\n    println!(\"{:?}\", v);\n}\n"), vec![]);
}

/// A borrow of a place in a temporary (`&f().0`, `&x.borrow().1`, `&v.clone()[0]`, `&f().0[..]`, a `&mut` field
/// reborrowed through a guard where a `&mut` is annotated): the temporary goes away at the end of the statement unless
/// the `let` extends it (E0716). Not through a reference the temporary is (`&v.iter().next().unwrap().1`)
#[test]
fn part16_temporary_places() {
    temp_error("fn mk() -> (i32, i32) {\n    (0, 1)\n}\nfn main() {\n    let v;\n    v = &mk().1;\n    println!(\"{}\", v);\n}\n", 6);
    temp_error("use std::cell::RefCell;\nfn id<T>(t: T) -> T {\n    t\n}\nfn main() {\n    let x = RefCell::new((0, 1));\n    let v: &i32 = id(&x.borrow().1);\n    println!(\"{}\", v);\n}\n", 7);
    temp_error("fn main() {\n    let x = vec![(0, 1)];\n    let v;\n    v = &x.clone()[0].1;\n    println!(\"{:?}\", v);\n}\n", 4);
    temp_error("fn main() {\n    let v;\n    v = &Box::new((0, 1)).1;\n    println!(\"{:?}\", v);\n}\n", 3);
    temp_error("fn mk() -> (String, i32) {\n    (String::new(), 1)\n}\nfn main() {\n    let v: &str;\n    v = &mk().0[..];\n    println!(\"{:?}\", v);\n}\n", 6);
    temp_error("use std::cell::RefCell;\nfn main() {\n    let mut r = 0;\n    let x = RefCell::new((&mut r, 0));\n    let val: &mut i32 = x.borrow_mut().0;\n    println!(\"{}\", val);\n}\n", 5);
    assert_eq!(check("use std::cell::RefCell;\nfn main() {\n    let x = RefCell::new((0, 1));\n    let v = &x.borrow().1;\n    println!(\"{}\", v);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let x = vec![(0, 1)];\n    let v = &x.clone()[0].1;\n    println!(\"{:?}\", v);\n}\n"), vec![]);
    assert_eq!(check("static P: (i32, i32) = (0, 1);\nfn mk() -> &'static (i32, i32) {\n    &P\n}\nfn main() {\n    let v;\n    v = &mk().1;\n    println!(\"{}\", v);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let x = vec![(0, 1)];\n    let v;\n    v = &x.iter().next().unwrap().1;\n    println!(\"{:?}\", v);\n}\n"), vec![]);
    assert_eq!(check("fn mk() -> (i32, i32) {\n    (0, 1)\n}\nfn main() {\n    let v;\n    v = &mk().1;\n    let _ = 0;\n}\n"), vec![]);
    assert_eq!(check("use std::cell::RefCell;\nfn main() {\n    let mut r = 0;\n    let x = RefCell::new((&mut r, 0));\n    let val: &mut i32 = &mut *x.borrow_mut().0;\n    *val = 2;\n}\n"), vec![]);
}

/// A returned `match`'s arm after the first whose value flows out as it is (not a block, an `if` or a constructor's
/// call) is reported at the match, as rustc's blame does (regions/regions-free-region-ordering-incorrect.rs); the
/// first arm, an `if`'s branches and what an arm makes are reported where they are
#[test]
fn part16_match_arm_returns() {
    one_error("fn f<'a, 'b>(c: bool, x: &'a i32, y: &'b i32) -> &'b i32 {\n    if c {\n        y\n    } else {\n        x\n    }\n}\nfn main() {}\n", 5);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> &'b i32 {\n    match c {\n        Some(_) => y,\n        None => x,\n    }\n}\nfn main() {}\n", 2);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> &'b i32 {\n    let _ = 1;\n    {\n        match c {\n            Some(_) => y,\n            None => x,\n        }\n    }\n}\nfn main() {}\n", 4);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> &'b i32 {\n    return match c {\n        Some(_) => y,\n        None => x,\n    };\n}\nfn main() {}\n", 2);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> &'b i32 {\n    match c {\n        Some(_) => y,\n        None => {\n            x\n        }\n    }\n}\nfn main() {}\n", 5);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> &'b i32 {\n    match c {\n        Some(_) => x,\n        None => y,\n    }\n}\nfn main() {}\n", 3);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> &'b i32 {\n    match c {\n        Some(0) => return y,\n        Some(_) => x,\n        None => y,\n    }\n}\nfn main() {}\n", 2);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a (i32, i32), y: &'b i32) -> &'b i32 {\n    match c {\n        Some(0) => y,\n        _ => if true { x.0.max(0); &x.1 } else { y },\n    }\n}\nfn main() {}\n", 4);
    one_error("fn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> Option<&'b i32> {\n    match c {\n        Some(0) => Some(y),\n        _ => Some(x),\n    }\n}\nfn main() {}\n", 4);
    one_error("fn id<T>(t: T) -> T { t }\nfn f<'a, 'b>(c: Option<i32>, x: &'a i32, y: &'b i32) -> &'b i32 {\n    match c {\n        Some(0) => y,\n        _ => id(x),\n    }\n}\nfn main() {}\n", 3);
}

/// From the review of #88: a block in an argument of a block tail's watched call keeps the outer tail's position watches
/// (it removed them: an index out of bounds); a discarded block value is drop-checked in edition 2024 too; a `dyn Fn`
/// call's result holds what it borrows, as a generic bound's; a coerced `&x` through a `&mut` lives no longer than it;
/// and what a local passed holds before a later argument assigns it flows too (both loans, as rustc's region)
#[test]
fn part16_review() {
    let d = check("use std::cell::RefCell;\nfn main() {\n    let c = RefCell::new(vec![1, 2]);\n    let n = {\n        let _d = 1;\n        c.borrow().get({ let e = RefCell::new(0usize); let v = *e.borrow(); v }).copied()\n    };\n    let m = {\n        let _d = 1;\n        c.borrow().get({ let e = RefCell::new(1usize); *e.borrow() }).copied()\n    };\n    println!(\"{:?} {:?}\", n, m);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 10 && d[0].1.contains("`e` does not live long enough"), "{d:?}");
    let d = check_ed("struct D<'a>(&'a i32);\nimpl Drop for D<'_> { fn drop(&mut self) { println!(\"{}\", self.0); } }\nfn main() {\n    { let x = 0; D(&x) };\n}\n", 2024);
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let f: &dyn Fn(&i32) -> &i32 = &|v| v;\n    let mut x = 1;\n    let r = f(&x);\n    x = 2;\n    println!(\"{} {}\", r, x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("because it is borrowed"), "{d:?}");
    let d = check("fn f<'short, 'long>(x: &'short mut &'long mut i32) -> &'long i32 {\n    &x\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("fn put<'a>(r: &'a i32, _u: (), v: &mut Vec<&'a i32>) { v.push(r) }\nfn main() {\n    let k = 1;\n    let mut x = 2;\n    let mut r = &k;\n    let mut v = vec![];\n    put(r, { r = &x; }, &mut v);\n    x = 3;\n    println!(\"{:?} {} {}\", v, r, x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("`x` because it is borrowed"), "{d:?}");
    let d = check("fn put<'a>(r: &'a i32, _u: (), v: &mut Vec<&'a i32>) { v.push(r) }\nfn main() {\n    let mut k = 1;\n    let x = 2;\n    let mut r = &k;\n    let mut v = vec![];\n    put(r, { r = &x; }, &mut v);\n    k = 3;\n    println!(\"{:?} {} {}\", v, r, k);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("`k` because it is borrowed"), "{d:?}");
    assert_eq!(check("fn main() {\n    let f: &dyn Fn(&i32) -> &i32 = &|v| v;\n    let g: &dyn Fn(&i32) -> i32 = &|v| *v;\n    let mut x = 1;\n    let r = f(&x);\n    println!(\"{}\", r);\n    let n = g(&x);\n    x = 2;\n    println!(\"{} {}\", n, x);\n}\n"), vec![]);
    assert_eq!(check("fn f<'a, 'b>(x: &'a &'b i32) -> &'b i32 {\n    &x\n}\nfn g<'a, 'b>(x: &'a mut &'b i32) -> &'b i32 {\n    &x\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check_ed("struct D<'a>(&'a i32);\nimpl Drop for D<'_> { fn drop(&mut self) { println!(\"{}\", self.0); } }\nfn main() {\n    let y = 1;\n    { let x = 0; let _ = x; D(&y) };\n}\n", 2024), vec![]);
    // (the second round: a `let ref` temporary of a block ends with it, also when a holder of the enclosing block's
    // value is pushed after it; a coerced `&x` keeps its loan as long as the lifetime it is annotated with, and through
    // `&mut` links only it lives no longer than x (E0515 when returned))
    let d = check("fn make() -> String { String::from(\"a\") }\nfn main() {\n    let r = { let ref x = make(); x };\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("temporary value dropped while borrowed"), "{d:?}");
    let d = check("fn f<'a>(x: &'a mut i32) {\n    let r: &'a i32 = &x;\n    *x = 2;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("because it is borrowed"), "{d:?}");
    let d = check("fn f(x: &mut i32) {\n    let r: &i32 = &x;\n    println!(\"{}\", r);\n    *x = 2;\n}\nfn g<'a>(x: &'a mut i32) -> &'a i32 {\n    &x\n}\nfn h(v: &mut Vec<i32>) -> usize {\n    let s: &Vec<i32> = &v;\n    let n = s.len();\n    v.push(1);\n    n\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("E0515"), "{d:?}");
    assert_eq!(check("fn make() -> String { String::from(\"a\") }\nfn main() {\n    let r = { let ref x = make(); x.len() };\n    println!(\"{}\", r);\n}\n"), vec![]);
    // (the third round: only a place the function owns is bounded by the coercion's borrow of it; one reached through a
    // reference (`&s.f` of `s: &mut S`, `&*x`) is a reborrow through it)
    assert_eq!(check("struct S<'a> { f: &'a mut i32 }\nfn g<'a>(s: &'a mut S<'a>) -> &'a i32 {\n    &s.f\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("fn g<'a>(x: &'a mut &'a mut i32) -> &'a i32 {\n    &*x\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("struct S<'a> {\n    r: &'a mut i32,\n}\nfn f<'a>(s: &'a S<'a>) -> &'a i32 {\n    &s.r\n}\nfn main() {}\n"), vec![]);
    let d = check("struct S<'a> { f: &'a mut i32 }\nfn g<'a>(s: S<'a>) -> &'a i32 {\n    &s.f\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0515"), "{d:?}");
    // (the fourth: such a reborrow is bounded by the references that reach the place too, as the explicit one)
    for s in ["&'short mut S<'long>", "&'short S<'long>"] {
        let d = check(&format!("struct S<'a> {{ r: &'a mut i32 }}\nfn f<'short, 'long>(s: {s}) -> &'long i32 {{\n    &s.r\n}}\nfn main() {{}}\n"));
        assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("`'short` must outlive `'long`"), "{d:?}");
    }
    assert_eq!(check("struct S<'a> { r: &'a mut i32 }\nfn g<'short, 'long>(s: &'short S<'long>) -> &'short i32 {\n    &s.r\n}\nfn h<'short, 'long: 'short>(s: &'short mut S<'long>) -> &'short i32 {\n    &s.r\n}\nfn main() {}\n"), vec![]);
}

/// `c[i]` of a struct whose `Deref` is to a slice (`type Target = [T]`) is `c.deref()[i]` (`deref_mut` for a
/// `&mut c[i]`): it was "cannot index", and what it borrows is the struct, so a write of another field while it is
/// used is E0506 (borrowck/issue-81365-6.rs); two shared ones and a `&mut` one after them are fine
#[test]
fn part17_index_through_deref() {
    let d = check("use std::ops::Deref;\n\nstruct Container {\n    target: Vec<()>,\n    container_field: bool,\n}\n\nimpl Deref for Container {\n    type Target = [()];\n    fn deref(&self) -> &Self::Target {\n        &self.target\n    }\n}\n\nimpl Container {\n    fn bad_borrow(&mut self) {\n        let first = &self[0];\n        self.container_field = true;\n        first;\n    }\n}\n\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 18 && d[0].1.contains("because it is borrowed"), "{d:?}");
    assert_eq!(check("use std::ops::{Deref, DerefMut};\nstruct C { v: Vec<i32>, s: String }\nimpl Deref for C { type Target = [i32]; fn deref(&self) -> &[i32] { &self.v } }\nimpl DerefMut for C { fn deref_mut(&mut self) -> &mut [i32] { &mut self.v } }\nstruct S(String);\nimpl Deref for S { type Target = str; fn deref(&self) -> &str { &self.0 } }\nfn main() {\n    let mut c = C { v: vec![1, 2, 3], s: String::new() };\n    let a = &c[0];\n    let b = &c[1];\n    println!(\"{} {} {}\", a, b, c[2]);\n    c[1] = 20;\n    let m = &mut c[2];\n    *m += 10;\n    println!(\"{:?} {} {}\", &c[..], c.len(), c.s.len());\n    let r = &mut c;\n    r[0] = 7;\n    println!(\"{}\", r[0] + r[1]);\n}\n"), vec![]);
}

/// A move out of what a raw pointer points to (`let y = *x;` with `x: *const Box<isize>`, `(*x).a`, a pattern of `*x`)
/// is E0507, as rustc's "behind a raw pointer" (borrowck/borrowck-move-from-unsafe-ptr.rs); a copy and a `ref` binding
/// are not
#[test]
fn part17_raw_pointer_moves() {
    let d = check("unsafe fn foo(x: *const Box<isize>) -> Box<isize> {\n    let y = *x;\n    return y;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("behind a raw pointer"), "{d:?}");
    let d = check("struct S { a: String }\nunsafe fn foo(x: *const S) -> String {\n    (*x).a\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("behind a raw pointer"), "{d:?}");
    let d = check("unsafe fn foo(x: *const (i32, String)) -> usize {\n    let (a, s) = *x;\n    a as usize + s.len()\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("behind a raw pointer"), "{d:?}");
    assert_eq!(check("unsafe fn foo(x: *const (i32, i32)) -> i32 {\n    let (a, b) = *x;\n    a + b\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("unsafe fn foo(x: *const (i32, String)) -> usize {\n    let (a, ref s) = *x;\n    a as usize + s.len()\n}\nfn main() {}\n"), vec![]);
}

/// A write through a `*const` pointer (`*ptr = 3`, `(*p).a[1] += 1`) is E0594, as rustc's "behind a `*const`
/// pointer" (borrowck/no-invalid-mut-suggestion-for-raw-pointer-issue-127562.rs); through a `*mut` one, or of a
/// `*const` value itself, it is fine
#[test]
fn part17_const_pointer_writes() {
    let d = check("fn main() {\n    let val = 2;\n    let ptr = &raw const val;\n    unsafe { *ptr = 3; }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("behind a `*const` pointer"), "{d:?}");
    let d = check("struct P { x: i32, a: [i32; 2] }\nunsafe fn f(p: *const P, q: *mut P) {\n    (*q).x = 1;\n    (*q).a[0] += 1;\n    (*p).a[1] += 1;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("behind a `*const` pointer"), "{d:?}");
    assert_eq!(check("unsafe fn f(p: *const i32) -> i32 {\n    let mut v = *p;\n    v += 1;\n    v\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("unsafe fn f(p: *mut *const i32, q: *const i32) {\n    *p = q;\n}\nfn main() {}\n"), vec![]);
}

/// A closure boxed where a `Box<dyn Fn()>` / `Box<dyn FnMut()>` is expected is that kind: its body moving out what it
/// captures by value (`Box::new(|| x)`, `Box::new(move || v)`) is E0507, as where a bound wants it. A `Box<dyn FnOnce()>`
/// may move it
#[test]
fn part17_boxed_fn_closures() {
    let d = check("fn f() -> Box<dyn Fn() -> Box<i32>> {\n    let x = Box::new(0);\n    Box::new(|| x)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("a captured variable in an `Fn` closure"), "{d:?}");
    let d = check("fn f() -> Box<dyn Fn() -> Box<i32>> {\n    let x = Box::new(0);\n    Box::new(move || x)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("a captured variable in an `Fn` closure"), "{d:?}");
    let d = check("fn g() -> Box<dyn FnMut() -> Vec<i32>> {\n    let v = vec![1];\n    Box::new(move || v)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("a captured variable in an `FnMut` closure"), "{d:?}");
    assert_eq!(check("fn f() -> Box<dyn FnOnce() -> Box<i32>> {\n    let x = Box::new(0);\n    Box::new(|| x)\n}\nfn g() -> Box<dyn FnMut() -> usize> {\n    let mut v = vec![1];\n    Box::new(move || { v.push(2); v.len() })\n}\nfn main() { println!(\"{} {}\", f()(), g()()); }\n"), vec![]);
}

/// A move out of an array that is no local's place, a constant's or a temporary's (`C[0]`, `mk()[0]`, `[S][0]`): E0508,
/// as out of a local's (borrowck/move-error-in-promoted*.rs). A copy, a borrow and a method's `&self` are fine
#[test]
fn part17_non_place_array_moves() {
    let d = check("fn mk() -> [String; 2] { [String::new(), String::new()] }\nfn main() {\n    let s = mk()[0];\n    println!(\"{}\", s);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("a non-copy array (E0508)"), "{d:?}");
    let d = check("#[derive(Clone)]\nstruct S2;\nfn main() {\n    const C: [S2; 1] = [S2];\n    let _ = C[0].clone();\n    let _s = C[0];\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("`[S2; 1]`, a non-copy array"), "{d:?}");
    assert_eq!(check("const C: [i32; 2] = [1, 2];\nfn mk() -> [String; 2] { [String::new(), String::new()] }\nfn main() {\n    let a = C[0];\n    let n = mk()[1].len();\n    let r = &mk()[0];\n    let c = mk()[0].clone();\n    println!(\"{} {} {} {}\", a, n, r, c);\n}\n"), vec![]);
}

/// A `&self` method whose self type is invariant in a lifetime an argument is borrowed for (or that it outlives,
/// `'f: 'c`) stores what the argument borrows in the receiver, as rustc's regions do: `c.push(&mut y)` twice of
/// `fn push(&self, r: &'c mut i32)` with C holding a `RefCell<Vec<&'c mut i32>>` conflicts
/// (borrowck/mut-borrow-conflict-in-closures-vec.rs, E0499). Not a covariant self type, nor once the receiver is gone
#[test]
fn part17_shared_receiver_stores() {
    let d = check("struct C<'c> { v: std::cell::RefCell<Vec<&'c mut i32>> }\nimpl<'c> C<'c> {\n    fn push<'f: 'c>(&self, r: &'f mut i32) { self.v.borrow_mut().push(r) }\n}\nfn main() {\n    let mut y = 1;\n    let c = C { v: std::cell::RefCell::new(vec![]) };\n    c.push(&mut y);\n    c.push(&mut y);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("mutably borrowed"), "{d:?}");
    let d = check("struct C<'c> { v: std::cell::RefCell<Vec<&'c mut i32>> }\nimpl<'c> C<'c> {\n    fn push(&self, r: &'c mut i32) { self.v.borrow_mut().push(r) }\n}\nfn main() {\n    let mut y = 1;\n    let c = C { v: std::cell::RefCell::new(vec![]) };\n    c.push(&mut y);\n    c.push(&mut y);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("mutably borrowed"), "{d:?}");
    let d = check("trait Push<'c> {\n    fn push(&self, r: &'c mut i32);\n}\nimpl<'c> Push<'c> for std::cell::RefCell<Vec<&'c mut i32>> {\n    fn push(&self, r: &'c mut i32) { self.borrow_mut().push(r) }\n}\nfn main() {\n    let mut y = 1;\n    let c = std::cell::RefCell::new(vec![]);\n    c.push(&mut y);\n    c.push(&mut y);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("mutably borrowed"), "{d:?}");
    let d = check("use std::cell::RefCell;\nstruct C<'c> { v: RefCell<Vec<&'c i32>> }\nimpl<'c> C<'c> {\n    fn push(&self, r: &'c i32) { self.v.borrow_mut().push(r) }\n    fn len(&self) -> usize { self.v.borrow().len() }\n}\nfn main() {\n    let mut y = 1;\n    let c = C { v: RefCell::new(vec![]) };\n    c.push(&y);\n    y = 2;\n    println!(\"{} {}\", c.len(), y);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("because it is borrowed"), "{d:?}");
    assert_eq!(check("struct C<'c> { v: Vec<&'c i32> }\nimpl<'c> C<'c> {\n    fn look(&self, r: &'c i32) -> usize { self.v.len() + *r as usize }\n}\nfn main() {\n    let mut y = 1;\n    let c = C { v: vec![] };\n    c.look(&y);\n    y = 2;\n    c.look(&y);\n}\n"), vec![]);
    assert_eq!(check("use std::cell::RefCell;\nstruct C<'c> { v: RefCell<Vec<&'c i32>> }\nimpl<'c> C<'c> {\n    fn push(&self, r: &'c i32) { self.v.borrow_mut().push(r) }\n    fn len(&self) -> usize { self.v.borrow().len() }\n}\nfn main() {\n    let mut y = 1;\n    {\n        let c = C { v: RefCell::new(vec![]) };\n        c.push(&y);\n        println!(\"{}\", c.len());\n    }\n    y = 2;\n    let c2 = C { v: RefCell::new(vec![]) };\n    c2.push(&y);\n    y = 3;\n    println!(\"{}\", y);\n}\n"), vec![]);
}

/// A `&mut` place around a method call (`&mut *s.0.borrow_mut()`) does not make the call's receiver a mutable place:
/// `s.0` of `s: Pin<&S>` / `Pin<&mut S>` / `&Vec<RefCell<()>>` needs no `mut s` (typeck/issue-73592-borrow_mut-through-deref.rs);
/// the prelude's `Pin<&T>` derefs shared
#[test]
fn part17_method_receivers_in_mut_places() {
    assert_eq!(check("//@ check-pass\n//@ run-rustfix\n//\n// rust-lang/rust#73592: borrow_mut through Deref should work.\n//\n// Before #72280, when we see something like `&mut *rcvr.method()`, we\n// incorrectly requires `rcvr` to be type-checked as a mut place. While this\n// requirement is usually correct for smart pointers, it is overly restrictive\n// for types like `Mutex` or `RefCell` which can produce a guard that\n// implements `DerefMut` from `&self`.\n//\n// Making it more confusing, because we use Deref as the fallback when DerefMut\n// is implemented, we won't see an issue when the smart pointer does not\n// implement `DerefMut`. It only causes an issue when `rcvr` is obtained via a\n// type that implements both `Deref` or `DerefMut`.\n//\n// This bug is only discovered in #73592 after it is already fixed as a side-effect\n// of a refactoring made in #72280.\n\n#![warn(unused_mut)]\n\nuse std::pin::Pin;\nuse std::cell::RefCell;\n\nstruct S(RefCell<()>);\n\nfn test_pin(s: Pin<&S>) {\n    // This works before #72280.\n    let _ = &mut *s.0.borrow_mut();\n}\n\nfn test_pin_mut(s: Pin<&mut S>) {\n    // This should compile but didn't before #72280.\n    let _ = &mut *s.0.borrow_mut();\n}\n\nfn test_vec(s: &Vec<RefCell<()>>) {\n    // This should compile but didn't before #72280.\n    let _ = &mut *s[0].borrow_mut();\n}\n\nfn test_mut_pin(mut s: Pin<&S>) {\n    //~^ WARN variable does not need to be mutable\n    let _ = &mut *s.0.borrow_mut();\n}\n\nfn test_mut_pin_mut(mut s: Pin<&mut S>) {\n    //~^ WARN variable does not need to be mutable\n    let _ = &mut *s.0.borrow_mut();\n}\n\nfn main() {\n    let mut s = S(RefCell::new(()));\n    test_pin(Pin::new(&s));\n    test_pin_mut(Pin::new(&mut s));\n    test_mut_pin(Pin::new(&s));\n    test_mut_pin_mut(Pin::new(&mut s));\n    test_vec(&vec![s.0]);\n}\n"), vec![]);
    assert_eq!(check("use std::cell::RefCell;\nuse std::pin::Pin;\nstruct S(RefCell<i32>);\nfn t(s: Pin<&S>) -> i32 {\n    let r = &mut *s.0.borrow_mut();\n    *r += 1;\n    *r\n}\nfn main() { let s = S(RefCell::new(1)); println!(\"{}\", t(Pin::new(&s))); }\n"), vec![]);
}

/// A place read or a call's result of a type that holds no reference (`*d` of `d: &mut bool`, `mem::replace(d, false)`)
/// carries no loan, and a statement's value is discarded: `let (b, log) = match *e { (G(_, log), ref mut d) => (*d, log) };`
/// held d's loan in log (drop/issue-23611-enum-swap-in-drop.rs, a false error). A reference read (`d`) still holds it,
/// and a `break` gives its value to the loop
#[test]
fn part17_values_with_no_references() {
    assert_eq!(check("use std::cell::Cell;\nstruct G<'a>(u32, &'a Cell<u32>);\nfn go<'a>(e: &mut (G<'a>, bool)) {\n    let (b, log) = match *e {\n        (G(_, log), ref mut d) => (*d, log),\n    };\n    let r = &mut *e;\n    let _ = r;\n    log.set(b as u32);\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("use std::cell::Cell;\nuse std::mem;\ntype Log<'a> = &'a Cell<u32>;\nstruct GaspA<'a>(for<'b> fn(u32, &'b str, Log<'a>), u32, Log<'a>);\nenum E<'a> { A(GaspA<'a>, bool) }\nfn f_a(_x: u32, _c: &str, _l: Log) {}\nimpl<'a> Drop for E<'a> {\n    fn drop(&mut self) {\n        let (do_drop, log) = match *self {\n            E::A(GaspA(ref f, ref mut val_a, log), ref mut do_drop) => {\n                f(0xA1A0, &format!(\"a={:x}\", val_a), log);\n                *val_a += 1;\n                (mem::replace(do_drop, false), log)\n            }\n        };\n        if do_drop {\n            mem::replace(self, E::A(GaspA(f_a, 0xA3A0, log), true));\n        }\n    }\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let mut x = 1;\n    let v = {\n        let n = x + 1;\n        let _ = &x;\n        n\n    };\n    x = 2;\n    println!(\"{} {}\", v, x);\n}\n"), vec![]);
    let d = check("use std::cell::Cell;\nstruct G<'a>(u32, &'a Cell<u32>);\nfn go<'a>(e: &mut (G<'a>, bool)) {\n    let (b, log) = match *e {\n        (G(_, log), ref mut d) => (d, log),\n    };\n    let r = &mut *e;\n    let _ = r;\n    *b = true;\n    log.set(1);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("mutably borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut x = 1;\n    let r = loop {\n        let t = &x;\n        if true { break t; }\n    };\n    x = 2;\n    println!(\"{} {}\", r, x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut x = vec![1];\n    let v = {\n        x.push(2);\n        &x\n    };\n    x.push(3);\n    println!(\"{:?}\", v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("also borrowed as immutable"), "{d:?}");
}

/// `BinaryHeap::peek_mut`'s guard holds the heap's `&mut` borrow while it lives: a read of the heap meanwhile conflicts
/// (E0502 in rustc). It was "no method named `peek_mut`"
#[test]
fn part17_peek_mut() {
    let d = check("use std::collections::BinaryHeap;\nfn main() {\n    let mut heap: BinaryHeap<i32> = BinaryHeap::new();\n    heap.push(1);\n    let b = heap.peek_mut();\n    println!(\"{:?}\", heap);\n    drop(b);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("mutably borrowed"), "{d:?}");
}

/// A scrutinee value whose drop may use what it borrows (`match heap.peek_mut() { Some(_) => .. }`, also when an arm
/// moves a part out) is a temporary of the statement: its loans live through the arms (borrowck/issue-85581.rs, E0499).
/// A last statement's temporaries end with it, before the block's locals (borrowck/issue-47646.rs had a false E0597).
/// Not a scrutinee bound whole (`x => drop(x)`), nor one with no drop (`Option<&mut i32>`)
#[test]
fn part17_scrutinee_temporaries() {
    let d = check("use std::collections::BinaryHeap;\nfn main() {\n    let mut heap: BinaryHeap<i32> = BinaryHeap::new();\n    heap.push(1);\n    match heap.peek_mut() {\n        Some(mut t) => { *t = 2; drop(t); heap.pop(); }\n        None => {}\n    }\n    match heap.peek_mut() {\n        Some(_) => {}\n        None => {}\n    }\n    heap.push(3);\n    println!(\"{:?}\", heap.len());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("mutably borrowed"), "{d:?}");
    let d = check("use std::collections::BinaryHeap;\nfn f(heap: &mut BinaryHeap<i32>) {\n    if let Some(_) = heap.peek_mut() {\n        heap.pop();\n    }\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("mutably borrowed"), "{d:?}");
    assert_eq!(check("use std::collections::BinaryHeap;\nfn main() {\n    let mut heap: BinaryHeap<i32> = BinaryHeap::new();\n    let borrow = heap.peek_mut();\n    match (borrow, ()) {\n        (Some(_), ()) => {}\n        _ => {}\n    };\n}\n"), vec![]);
    assert_eq!(check("fn mk(v: &mut Vec<i32>) -> Option<&mut i32> { v.first_mut() }\nfn main() {\n    let mut v = vec![1];\n    match mk(&mut v) {\n        Some(_) => { v.push(2); }\n        None => {}\n    }\n}\n"), vec![]);
    assert_eq!(check("use std::collections::BinaryHeap;\nfn main() {\n    let mut heap: BinaryHeap<i32> = BinaryHeap::new();\n    heap.push(1);\n    match heap.peek_mut() {\n        x => { drop(x); heap.pop(); }\n    }\n}\n"), vec![]);
    // (its `ref` bindings' borrow of its own storage is not what its drop uses: dropck/eager-by-ref-binding-for-guards.rs)
    for p in ["(long2, ref short2) if true", "(ref long2, _) if true", "(_, ref short2) if true"] {
        assert_eq!(check(&format!("struct Struct<T>(T);\nimpl<T> Drop for Struct<T> {{\n    fn drop(&mut self) {{}}\n}}\nfn main() {{\n    match (Struct(&&0), 1) {{\n        {p} => {{ let _ = 0; }}\n        _ => unreachable!(),\n    }}\n}}\n")), vec![]);
    }
}

/// `let x = &*R.m();` with R a temporary a call makes and a result that borrows it: the `let` extends the result, not R,
/// which goes away at the end of the statement (E0716 when x is used after): `&mut *RefCell::new(1).borrow_mut()`,
/// `&*String::from("b").as_str()`. The lowering's copy of a guard's deref keeps what the borrow check follows. Not a
/// result that borrows nothing of R (`&String::from("x").len()`, `&RefCell::new(1).borrow().clone()`)
#[test]
fn part17_receivers_of_extended_borrows() {
    temp_error("use std::cell::RefCell;\nfn main() {\n    let r = &mut *RefCell::new(1).borrow_mut();\n    *r += 1;\n}\n", 3);
    temp_error("use std::cell::RefCell;\nfn main() {\n    let r = &*RefCell::new(vec![1]).borrow();\n    println!(\"{:?}\", r);\n}\n", 3);
    temp_error("fn main() {\n    let r = &String::from(\"a\").clone();\n    let s = &*String::from(\"b\").as_str();\n    println!(\"{} {}\", r, s);\n}\n", 3);
    assert_eq!(check("use std::cell::RefCell;\nfn main() {\n    let c = RefCell::new(1);\n    let r = &mut *c.borrow_mut();\n    *r += 1;\n}\n"), vec![]);
    assert_eq!(check("use std::cell::RefCell;\nfn main() {\n    let a = &*String::from(\"b\");\n    let n = &RefCell::new(1).borrow().clone();\n    let l = &String::from(\"x\").len();\n    let m = &vec![1, 2].iter().count();\n    println!(\"{} {} {} {}\", a, n, l, m);\n}\n"), vec![]);
}

/// A `&mut self` method of what a program's `DerefMut` reaches (`w.push(2)` with `W: DerefMut<Target = Vec<usize>>`)
/// borrows w mutably, as autoderef takes `deref_mut` for it: E0596 without `mut w` or through a `&W`, E0502 against a
/// shared loan of w or a read of w in the arguments (`w.push(w.len())`, as `(&mut *w).push(w.len())`), and a result
/// that borrows from it holds the loan. A `&self` method borrows it shared
#[test]
fn part17_deref_mut_methods() {
    let w = "use std::ops::{Deref, DerefMut};\nstruct W(Vec<usize>);\nimpl Deref for W { type Target = Vec<usize>; fn deref(&self) -> &Vec<usize> { &self.0 } }\nimpl DerefMut for W { fn deref_mut(&mut self) -> &mut Vec<usize> { &mut self.0 } }\n";
    let d = check(&format!("{w}fn main() {{\n    let w = W(vec![1]);\n    w.push(2);\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("not declared as mutable"), "{d:?}");
    let d = check(&format!("{w}fn f(w: &W) {{\n    w.push(1);\n}}\nfn main() {{}}\n"));
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("behind a `&` reference"), "{d:?}");
    let d = check(&format!("{w}fn main() {{\n    let mut w = W(vec![1]);\n    let r = &w;\n    w.push(3);\n    println!(\"{{}}\", r.len());\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("also borrowed as immutable"), "{d:?}");
    let d = check(&format!("{w}fn main() {{\n    let mut w = W(vec![1]);\n    w.push(w.len());\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("mutably borrowed"), "{d:?}");
    let d = check(&format!("{w}fn main() {{\n    let mut w = W(vec![1]);\n    (&mut *w).push(w.len());\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("mutably borrowed"), "{d:?}");
    let d = check(&format!("{w}fn main() {{\n    let mut w = W(vec![1]);\n    let r = w.last_mut().unwrap();\n    let k = w.len();\n    *r = k;\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("mutably borrowed"), "{d:?}");
    assert_eq!(check(&format!("{w}fn main() {{\n    let mut w = W(vec![1]);\n    w.push(2);\n    let n = w.len();\n    w[0] = n;\n    let r = &w;\n    println!(\"{{}}\", r.len());\n}}\n")), vec![]);
    assert_eq!(check(&format!("{w}fn f(w: &mut W) -> usize {{\n    w.push(1);\n    let n = w.len();\n    w.push(n);\n    w.len()\n}}\nfn main() {{}}\n")), vec![]);
}

/// A `ref` binding of a loop's item (`for ref x in 0..3`, `while let Some(ref x) = it.next()`) borrows a temporary of
/// the iteration, gone at its end, as a loop's body is a terminating scope (also the temporaries of the body's tail,
/// `loop { match it.next() { Some(ref x) => .. } }`): E0716 when the borrow is kept past it. A constant scrutinee is
/// promoted (`match Some(1) { Some(ref x) => .. }`), and an item that holds references is copied out (`out.push(*s)`
/// of `for ref s in v.iter()`)
#[test]
fn part17_loop_iteration_temporaries() {
    let tail = "    println!(\"{:?}\", v);\n}\n";
    temp_error(&format!("fn main() {{\n    let mut v = vec![];\n    for ref x in 0..3 {{\n        v.push(x);\n    }}\n{tail}"), 3);
    temp_error(&format!("fn main() {{\n    let mut v = vec![];\n    for ref x in vec![1, 2].into_iter() {{\n        v.push(x);\n    }}\n{tail}"), 3);
    temp_error(&format!("fn main() {{\n    let mut v = vec![];\n    for ref x in [String::new()] {{\n        v.push(x);\n    }}\n{tail}"), 3);
    temp_error(&format!("fn main() {{\n    let mut v = vec![];\n    let mut it = vec![1, 2].into_iter();\n    while let Some(ref x) = it.next() {{\n        v.push(x);\n    }}\n{tail}"), 4);
    temp_error(&format!("fn main() {{\n    let mut v = vec![];\n    let mut it = vec![1, 2].into_iter();\n    loop {{\n        match it.next() {{\n            Some(ref x) => v.push(x),\n            None => break,\n        }}\n    }}\n{tail}"), 5);
    assert_eq!(check(&format!("fn main() {{\n    let mut v: Vec<&i32> = vec![];\n    match Some(1) {{\n        Some(ref x) => v.push(x),\n        None => {{}}\n    }};\n    let r: &i32 = match 5 {{ ref y => y }};\n    println!(\"{{}}\", r);\n{tail}")), vec![]);
    assert_eq!(check("fn main() {\n    let v = vec![String::new(), String::from(\"a\")];\n    let mut out: Vec<&String> = vec![];\n    for ref s in v.iter() {\n        out.push(*s);\n    }\n    for (_i, ref s) in v.iter().enumerate() {\n        out.push(*s);\n    }\n    let mut it = v.iter();\n    while let Some(ref s) = it.next() {\n        out.push(s);\n    }\n    match v.iter().next() {\n        Some(ref s) => out.push(*s),\n        None => {}\n    };\n    let mut n: Vec<i32> = vec![];\n    for ref x in 0..3 {\n        n.push(*x);\n    }\n    println!(\"{} {:?}\", out.len(), n);\n}\n"), vec![]);
}

/// A call's turbofish gives the fn's type arguments to the inference: `let c = 66; id::<&'static u32>(&c)` makes c a
/// u32 (it was "mismatched types: expected `T0`, found `&i32`"), and the borrow must be 'static (E0597), also through a
/// fn item with every type argument written (`let f = id::<&'static u32>; f(&c)`), as
/// nll/user-annotations/adt-tuple-struct-calls.rs writes them
#[test]
fn part17_fn_turbofish_inference() {
    let id = "fn id<T>(t: T) -> T { t }\n";
    for call in ["id::<&'static u32>(&c);", "let f = id::<&'static u32>;\n    f(&c);"] {
        let d = check(&format!("{id}fn main() {{\n    let c = 66;\n    {call}\n}}\n"));
        assert!(d.len() == 1 && d[0].1.contains("does not live long enough"), "{call}: {d:?}");
    }
    assert_eq!(check(&format!("{id}fn main() {{\n    let c = 66;\n    id::<u32>(c);\n    let d = 1;\n    let _r = id::<&u32>(&d);\n    let f = id::<u64>;\n    let _g = f(300);\n}}\n")), vec![]);
}

/// A reference copied out of what a local points to (`*r` of `r: &&String`, `&**s`, a `&&String` where a `&String` is
/// expected) holds what that place holds, not the loan the local is: `let r = &a; t = *r;` used after a's scope is
/// accepted, as in rustc. What a holds is still followed (a borrow of a dead s, a write of the Vec it points into)
#[test]
fn part18_copied_out_references() {
    assert_eq!(check("fn main() {\n    let v = vec![String::new(), String::from(\"a\")];\n    let t: &String;\n    {\n        let a: &String = &v[0];\n        let r = &a;\n        t = *r;\n    }\n    println!(\"{}\", t);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let v = vec![String::new()];\n    let mut out: Vec<&String> = vec![];\n    {\n        let a: &String = &v[0];\n        let r = &a;\n        out.push(r);\n    }\n    println!(\"{}\", out.len());\n}\n"), vec![]);
    let d = check("fn main() {\n    let t: &String;\n    {\n        let s = String::new();\n        let a: &String = &s;\n        let r = &a;\n        t = *r;\n    }\n    println!(\"{}\", t);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("does not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let mut v = vec![String::new()];\n    let a: &String = &v[0];\n    let r = &a;\n    let t = *r;\n    v.push(String::new());\n    println!(\"{}\", t);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("borrowed as immutable"), "{d:?}");
}

/// A `for` loop's item over `&v` / `v.iter()` holds the loop's loan of v: keeping it past the loop and changing v is
/// E0502 (it held nothing). A `ref` binding of an item or a scrutinee that holds references borrows a temporary
/// that holds them: `*s` copies them out, and `s` itself kept past the iteration is E0716
#[test]
fn part18_loop_items() {
    for (head, keep) in [("for x in &v {", "out.push(x);"), ("for x in v.iter() {", "out.push(x);"), ("for ref x in &v {", "out.push(*x);"), ("for x in &v {", "keep = Some(x);")] {
        let d = check(&format!("fn main() {{\n    let mut v = vec![String::new()];\n    let mut out: Vec<&String> = vec![];\n    let mut keep: Option<&String> = None;\n    {head}\n        {keep}\n    }}\n    v.push(String::new());\n    println!(\"{{}} {{:?}}\", out.len(), keep);\n}}\n"));
        assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("borrowed as immutable"), "{head} {keep}: {d:?}");
    }
    temp_error("fn main() {\n    let v = vec![String::new()];\n    let mut out: Vec<&&String> = vec![];\n    match v.iter().next() {\n        Some(ref s) => out.push(s),\n        None => {}\n    };\n    println!(\"{}\", out.len());\n}\n", 4);
    temp_error("fn main() {\n    let v = vec![String::new()];\n    let mut out: Vec<&&String> = vec![];\n    for ref s in v.iter() {\n        out.push(s);\n    }\n    println!(\"{}\", out.len());\n}\n", 4);
    assert_eq!(check("fn main() {\n    let v = vec![String::new(), String::from(\"a\")];\n    let mut out: Vec<&String> = vec![];\n    for ref s in &v {\n        out.push(s);\n    }\n    for (_i, ref s) in v.iter().enumerate() {\n        out.push(*s);\n    }\n    match v.iter().next() {\n        Some(ref s) => out.push(*s),\n        None => {}\n    };\n    println!(\"{}\", out.len());\n}\n"), vec![]);
}

/// The review of #97, each case checked with rustc 1.97:
/// - a `ref` binding of a scrutinee or an item that holds references and owns storage
///   (`(ref s, _)` of `(String::from("x"), n)`) borrows the temporary, E0716 when kept past it;
/// - a `for ref` item's temporary ends on a `break` too, and an `enumerate` pair's `ref` bindings borrow one
///   (E0716);
/// - a write through a `*mut` a `*const` points to (`**p = 1`) is fine
#[test]
fn part17_review() {
    temp_error("fn f(n: &i32) -> usize {\n    let r: &String = match (String::from(\"x\"), n) { (ref s, _) => s };\n    r.len()\n}\nfn main() {}\n", 2);
    temp_error("fn g(n: &i32) -> usize {\n    let mut keep: &String = &String::new();\n    for ref s in [(String::from(\"x\"), n)] { keep = &s.0; }\n    keep.len()\n}\nfn main() {}\n", 3);
    temp_error("fn main() {\n    let mut saved = &0;\n    for ref x in [1, 2] { saved = x; break; }\n    println!(\"{}\", saved);\n}\n", 3);
    temp_error("fn main() {\n    let v = vec![10, 20];\n    let mut saved = &0usize;\n    for (ref i, _) in v.iter().enumerate() { saved = i; }\n    println!(\"{}\", saved);\n}\n", 4);
    assert_eq!(check("fn main() {\n    let mut v = 0;\n    let q: *mut i32 = &mut v;\n    let p: *const *mut i32 = &q;\n    unsafe { **p = 1; }\n    println!(\"{}\", v);\n}\n"), vec![]);
    let d = check("fn main() {\n    let v = 0;\n    let q: *const i32 = &v;\n    let p: *mut *const i32 = &mut (q as *const i32) as *mut *const i32;\n    unsafe { **p = 1; }\n}\n");
    assert!(d.len() == 1 && d[0].1.contains("behind a `*const` pointer"), "{d:?}");
}

#[test]
fn part17_review2() {
    // (a loop's items through `copied` / `cloned` hold no loan of v; a loop over a value inside one over `&v` holds
    // nothing of it)
    assert_eq!(check("fn main() {\n    let n = 1;\n    let mut v = vec![&n];\n    let mut saved = &n;\n    for x in v.iter().copied() { saved = x; }\n    v.clear();\n    println!(\"{}\", saved);\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let n = 1;\n    let mut v = vec![1, 2];\n    let mut saved = &n;\n    for _x in &v { for y in [&n] { saved = y; } }\n    v.clear();\n    println!(\"{}\", saved);\n}\n"), vec![]);
    // (`&**s` with `s: &&i32`: through the shared reference s points to, what that holds, not s's loan of a)
    assert_eq!(check("fn main() {\n    let n = 1;\n    let r;\n    {\n        let a = &n;\n        let s = &a;\n        r = &**s;\n    }\n    println!(\"{}\", r);\n}\n"), vec![]);
    let d = check("fn main() {\n    let mut m = 2;\n    let q;\n    {\n        let a = &mut m;\n        let s = &a;\n        q = &**s;\n    }\n    println!(\"{}\", q);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("`a` does not live long enough"), "{d:?}");
    // (a struct whose `deref_mut` gives itself: E0055, not a `&mut self` method through it)
    let d = check("use std::ops::{Deref, DerefMut};\nstruct S;\nimpl Deref for S { type Target = S; fn deref(&self) -> &S { self } }\nimpl DerefMut for S { fn deref_mut(&mut self) -> &mut S { self } }\nfn main() {\n    let mut s = S;\n    s.reverse();\n}\n");
    assert!(d.len() == 1 && d[0].1.contains("E0055"), "{d:?}");
    // (two blanket impls of a generic trait: not told apart, so neither is taken)
    let d = check("trait A<X> { fn get(&self) -> i32; }\nimpl<T> A<u8> for T { fn get(&self) -> i32 { 8 } }\nimpl<T> A<u16> for T { fn get(&self) -> i32 { 16 } }\nfn f(x: &dyn A<u16>) -> i32 { x.get() }\nfn main() {\n    let v = 5u8;\n    let r = &v;\n    println!(\"{}\", f(&r));\n}\n");
    assert!(d.len() == 1 && d[0].1.contains("has no implementation"), "{d:?}");
}

#[test]
fn part17_review3() {
    // (what a `PeekMut`'s `deref_mut` gives borrows the guard: dropping the guard while it is written is E0505)
    let d = check("use std::collections::BinaryHeap;\nuse std::ops::DerefMut;\nfn main() {\n    let mut heap = BinaryHeap::from(vec![1, 5, 3]);\n    let mut g = heap.peek_mut().unwrap();\n    let r = g.deref_mut();\n    drop(g);\n    *r = 0;\n    println!(\"{:?}\", heap.peek());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
    // (two blanket impls of a generic trait that override its default: not told apart, so not the default either)
    let d = check("trait A<X> { fn get(&self) -> i32 { 0 } }\nimpl<T> A<u8> for T { fn get(&self) -> i32 { 8 } }\nimpl<T> A<u16> for T { fn get(&self) -> i32 { 16 } }\nfn f(x: &dyn A<u16>) -> i32 { x.get() }\nfn main() {\n    let v = 5u8;\n    let r = &v;\n    println!(\"{}\", f(&r));\n}\n");
    assert!(d.len() == 1 && d[0].1.contains("has no implementation"), "{d:?}");
    // (`&g` of a guard where a `&mut T` is expected: a shared borrow, no coercion makes it mutable (E0308))
    let d = check("use std::cell::RefCell;\nfn main() {\n    let c = RefCell::new(1);\n    let mut g2 = c.borrow_mut();\n    let r2: &mut i32 = &g2;\n    *r2 = 2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("mismatched types"), "{d:?}");
    assert_eq!(check("use std::cell::RefCell;\nfn bump(x: &mut i32) { *x += 1; }\nfn main() {\n    let c = RefCell::new(1);\n    let mut g = c.borrow_mut();\n    bump(&mut g);\n    let r: &i32 = &g;\n    println!(\"{}\", r);\n}\n"), vec![]);
}

#[test]
fn part17_review4() {
    // (`w.push(..)` of a `w: &mut W` with a `DerefMut` W is `(&mut **w).push(..)`: its loan of w lasts while the
    // arguments are evaluated, which assign `*w` (E0506))
    let d = check("use std::ops::{Deref, DerefMut};\nstruct W(Vec<usize>);\nimpl Deref for W { type Target = Vec<usize>; fn deref(&self) -> &Vec<usize> { &self.0 } }\nimpl DerefMut for W { fn deref_mut(&mut self) -> &mut Vec<usize> { &mut self.0 } }\nfn main() {\n    let mut x = W(vec![1]);\n    let w = &mut x;\n    w.push({ *w = W(vec![]); 2 });\n    println!(\"{}\", x.0.len());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("cannot assign to `*w`"), "{d:?}");
}

/// A `let` extends a `&` of a temporary within its initializer's extending expressions: a tuple struct's or a
/// variant's constructor's arguments, a tuple's, an array's or a struct literal's fields (`let x = Some(&[name]);`,
/// `let e = X(&temp());`, rustc since 1.79). They were a false E0716. A call of a fn, a method's receiver, are not
/// extended. A call through a fn pointer holds what its arguments borrow when its result holds references
#[test]
fn part18_let_extension_in_constructors() {
    assert_eq!(check("fn temp() -> String { String::from(\"x\") }\n#[derive(Debug)]\nstruct X<'a>(&'a String);\nfn main() {\n    let name = \"Foo\";\n    let a = Some(&[name]);\n    let b = Some(&temp());\n    let e = X(&temp());\n    let t = (&temp(), [&temp()]);\n    let o = Some(X(&temp()));\n    println!(\"{:?} {:?} {:?} {:?} {:?}\", a, b, e, t, o);\n}\n"), vec![]);
    temp_error("fn temp() -> String { String::from(\"x\") }\nfn main() {\n    let h = Some(temp().as_str());\n    println!(\"{:?}\", h);\n}\n", 3);
    temp_error("fn temp() -> String { String::from(\"x\") }\nfn id<T>(t: T) -> T { t }\nfn main() {\n    let f = id::<&String>;\n    let g = f(&temp());\n    println!(\"{:?}\", g);\n}\n", 5);
}

/// A capture of a closure reborrowed where a longer `&mut` is expected is reborrowed through the closure's
/// environment, which the closure has for no longer than a call: `|| doit(data)` with `data: &'static mut ()` and
/// `doit(&'static mut ())` is "lifetime may not live long enough" (nll/issue-42574-diagnostic-in-nested-closure.rs),
/// also for a `move` closure (FnMut). Moved out first (`let d = data; doit(d)`), or a shared reference copied, or a
/// callee that takes any lifetime, is fine
#[test]
fn part18_closure_capture_reborrows() {
    for c in ["|| doit(data)", "move || doit(data)", "|| doit(&mut *data)"] {
        let d = check(&format!("fn doit(_: &'static mut ()) {{}}\nfn f(data: &'static mut ()) {{\n    let mut c = {c};\n    c();\n}}\nfn main() {{}}\n"));
        assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("must outlive `'static`"), "{c}: {d:?}");
    }
    assert_eq!(check("fn doit(_: &'static mut ()) {}\nfn f(data: &'static mut ()) {\n    let c = || { let d = data; doit(d) };\n    c();\n}\nfn keep<'a>(_: &'a mut ()) {}\nfn g<'a>(data: &'a mut ()) {\n    let mut c = || keep(data);\n    c();\n}\nfn keep2(_: &'static ()) {}\nfn h(data: &'static ()) {\n    let c = || keep2(data);\n    c();\n}\nfn main() {}\n"), vec![]);
}

/// A `let` annotation with holes in a fn pointer type (`let f: fn(_) -> _ = foo;`) is the inferred instance: it was
/// "mismatched types: expected `fn(_) -> _`". Its regions are fresh ones the initializer flows into, so a result of
/// `foo<'a>(&'a u32) -> &'a u32` through it is still bounded by the argument (nll/mir_check_cast_reify.rs)
#[test]
fn part18_fn_pointer_annotation_holes() {
    assert_eq!(check("fn foo(x: u32) -> u32 { x + 1 }\nfn bar(x: &u32) -> &u32 { x }\nfn main() {\n    let f: fn(_) -> _ = foo;\n    let g: fn(_) -> _ = bar;\n    let h: fn(u32) -> _ = foo;\n    println!(\"{} {} {}\", f(1), g(&2), h(3));\n}\n"), vec![]);
    let d = check("fn foo<'a>(x: &'a u32) -> &'a u32 where 'a: 'a { x }\nfn bar<'a>(x: &'a u32) -> &'static u32 {\n    let f: fn(_) -> _ = foo;\n    f(x)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("must outlive `'static`"), "{d:?}");
}

/// The constructor of a tuple struct whose field has no size known (`struct Test([i32]);`), used as a value: no
/// fn value takes such a parameter (E0277), also where an annotation `fn(_) -> Test` with a hole types it
#[test]
fn part18_unsized_ctor_value() {
    for src in ["struct Test([i32]);\nfn main() {\n    let _x = Test;\n}\n", "struct Test([i32]);\nfn main() {\n    let _x: fn(_) -> Test = Test;\n}\n"] {
        let d = check(src);
        assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0277"), "{src}\n{d:?}");
    }
    assert_eq!(check("struct Test(i32);\nfn main() {\n    let f: fn(_) -> Test = Test;\n    let _ = f(1);\n}\n"), vec![]);
}

/// A scrutinee whose drop uses what it borrows, with an arm that takes the whole value (`x if flag => drop(x)`): that
/// arm moves it, the others keep it to the end of the statement (E0499 of `_ => heap.push(1)`)
#[test]
fn part18_scrutinee_drop_per_arm() {
    let d = check("use std::collections::BinaryHeap;\nfn main() {\n    let mut heap = BinaryHeap::from(vec![1, 2]);\n    let flag = std::env::args().count() > 5;\n    match heap.peek_mut() {\n        x if flag => drop(x),\n        _ => heap.push(1),\n    };\n    println!(\"{}\", heap.len());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
    let d = check("use std::collections::BinaryHeap;\nfn main() {\n    let mut heap = BinaryHeap::from(vec![1, 2]);\n    match heap.peek_mut() {\n        Some(_) => heap.push(1),\n        x => drop(x),\n    };\n    println!(\"{}\", heap.len());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
    assert_eq!(check("use std::collections::BinaryHeap;\nfn main() {\n    let mut heap = BinaryHeap::from(vec![1, 2]);\n    let flag = std::env::args().count() > 5;\n    match heap.peek_mut() {\n        x if flag => drop(x),\n        x => drop(x),\n    }\n    heap.push(3);\n    let n = match heap.peek_mut() {\n        x if flag => { drop(x); 1 }\n        _ => 2,\n    };\n    heap.push(n);\n    println!(\"{}\", heap.len());\n}\n"), vec![]);
}

/// A `&mut` read by value out of a place where a `let` with no type written is initialized from it: moved, not
/// reborrowed (no coercion site). Out of an array (E0508), through a raw pointer or a reference (E0507). Where a
/// `&mut` is expected (an argument, `let c: &mut i32 = a[0];`), it is reborrowed as before
#[test]
fn part18_mut_refs_moved_by_value() {
    let d = check("fn main() {\n    let mut x = 1;\n    let a = [&mut x];\n    let b = a[0];\n    *b = 2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("cannot move out of"), "{d:?}");
    let d = check("fn main() {\n    let mut x = 1;\n    let mut r = &mut x;\n    let p: *const &mut i32 = &r;\n    let y = unsafe { *p };\n    *y = 2;\n    let _ = &mut r;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0507"), "{d:?}");
    let d = check("fn main() {\n    let mut z = 5;\n    let mut q = &mut z;\n    let qq = &mut q;\n    let u = *qq;\n    *u = 3;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot move out of `*qq`"), "{d:?}");
    assert_eq!(check("fn take(r: &mut i32) { *r += 1; }\nfn main() {\n    let mut x = 1;\n    let mut y = 2;\n    let mut a = [&mut x, &mut y];\n    take(a[0]);\n    *a[1] = 5;\n    let c: &mut i32 = a[0];\n    *c = 3;\n    let v = &mut a;\n    take(v[1]);\n    println!(\"{}\", a.len());\n}\n"), vec![]);
}

/// A trait object's drop is not known: it may use what it borrows (rustc's "when `f` is dropped and runs the
/// destructor for type `Box<dyn FnMut()>`"). A `&dyn` is no owner
#[test]
fn part18_dyn_drops() {
    let d = check("fn main() {\n    let mut v = 1;\n    let mut f: Box<dyn FnMut()> = Box::new(|| v += 1);\n    f();\n    println!(\"{}\", v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
    let d = check("trait Foo { }\nimpl<'a> Foo for &'a isize { }\nfn main() {\n    let blah;\n    {\n        let x: isize = 1;\n        blah = Box::new(&x) as Box<dyn Foo>;\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("`x` does not live long enough"), "{d:?}");
    let d = check("trait Foo { fn get(&self) -> isize; }\nimpl<'a> Foo for &'a isize { fn get(&self) -> isize { **self } }\nfn main() {\n    let mut x: isize = 1;\n    let b: Box<dyn Foo> = Box::new(&x);\n    println!(\"{}\", b.get());\n    x = 2;\n    println!(\"{}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
    assert_eq!(check("trait Foo { }\nimpl<'a> Foo for &'a isize { }\nfn main() {\n    let x: isize = 1;\n    let b = Box::new(&x) as Box<dyn Foo>;\n    drop(b);\n    let mut v = vec![1];\n    let r: &dyn std::fmt::Debug = &v[0];\n    println!(\"{:?}\", r);\n    v.push(2);\n}\n"), vec![]);
}

#[test]
fn part18_review() {
    // (edition 2024: a block's tail is a statement of its own; a constructor's `&` argument there is a call's, not a
    // `let`'s to extend)
    let d = check_ed("fn temp() -> String { String::from(\"x\") }\nfn consume<T: std::fmt::Debug>(t: T) { println!(\"{:?}\", t); }\nfn main() {\n    consume({ (Some(&temp()),) });\n}\n", 2024);
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0716"), "{d:?}");
    // (a local fn pointer named in upper case is no constructor: its arguments are not extended)
    let d = check("fn temp() -> String { String::from(\"x\") }\nfn id(s: &String) -> &String { s }\n#[allow(non_snake_case)]\nfn main() {\n    let F: fn(&String) -> &String = id;\n    let r = F(&temp());\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0716"), "{d:?}");
    // (an or-pattern whose alternatives each take the whole value moves it)
    assert_eq!(check("struct G<'a>(&'a mut i32);\nimpl Drop for G<'_> { fn drop(&mut self) { *self.0 += 1; } }\nfn mk(n: &mut i32) -> Option<G<'_>> { Some(G(n)) }\nfn main() {\n    let mut n = 0;\n    match mk(&mut n) {\n        x @ Some(_) | x @ None => { drop(x); n = 1; }\n    };\n    println!(\"{}\", n);\n}\n"), vec![]);
    // (a lowercase constant in a pattern is compared with, not bound: the arm keeps the scrutinee's drop)
    let d = check("#![allow(non_upper_case_globals)]\n#[derive(PartialEq, Eq)]\nstruct W<'a>(Option<&'a i32>);\nimpl Drop for W<'_> { fn drop(&mut self) {} }\nconst none: Option<W<'static>> = None;\nfn mk(n: &i32) -> Option<W<'_>> { Some(W(Some(n))) }\nfn main() {\n    let mut n = 0;\n    match mk(&n) {\n        none => { n = 1; }\n        _ => {}\n    };\n    println!(\"{}\", n);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 10, "{d:?}");
    // (a fn pointer whose result is `'static`: it holds nothing of its arguments; an elided one does)
    assert_eq!(check("fn fixed(_: &i32) -> &'static i32 { &42 }\nfn main() {\n    let f: fn(&i32) -> &'static i32 = fixed;\n    let r;\n    {\n        let x = 1;\n        r = f(&x);\n    }\n    println!(\"{}\", r);\n}\n"), vec![]);
    let d = check("fn fixed(x: &i32) -> &i32 { x }\nfn main() {\n    let f: fn(&i32) -> &i32 = fixed;\n    let r;\n    {\n        let x = 1;\n        r = f(&x);\n    }\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7, "{d:?}");
}

#[test]
fn part18_review2() {
    // (a `&*` of a method's result in a `let`'s constructor argument or tuple field: the method's receiver is the
    // statement's, as for the `let`'s own `&`)
    let h = "struct H(String);\nimpl H { fn get(&self) -> &String { &self.0 } }\nfn temp() -> H { H(String::from(\"x\")) }\n";
    for init in ["Some(&*temp().get())", "(&*temp().get(),)", "Some(Some(&*temp().get()))"] {
        let d = check(&format!("{h}fn main() {{\n    let r = {init};\n    println!(\"{{:?}}\", r);\n}}\n"));
        assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0716"), "{init}: {d:?}");
    }
    assert_eq!(check(&format!("{h}fn t2() -> String {{ String::from(\"y\") }}\nfn main() {{\n    let a = Some(&temp().get().len());\n    let b = Some((&t2(), Some(&t2())));\n    let c = Some(&temp().0);\n    println!(\"{{:?}} {{:?}} {{:?}}\", a, b, c);\n}}\n")), vec![]);
    // (a binding named as another module's static binds the whole value)
    assert_eq!(check("mod m {\n    #[allow(non_upper_case_globals)]\n    pub static val: i32 = 1;\n}\nstruct G<'a>(&'a mut i32);\nimpl Drop for G<'_> { fn drop(&mut self) { *self.0 += 1; } }\nfn mk(n: &mut i32) -> Option<G<'_>> { Some(G(n)) }\nfn main() {\n    let mut n = 0;\n    match mk(&mut n) {\n        val => { drop(val); n = 1; }\n    };\n    println!(\"{} {}\", n, m::val);\n}\n"), vec![]);
}

/// A fn item passed where a fn pointer type that binds its regions is expected: it must do for every region the type
/// binds. `fn baz(x: &S) -> &S` (`for<'a> fn(&'a S) -> &'a S`) for a `for<'cx> fn(&'cx S) -> &'static S` is E0308 in
/// rustc ("lifetime may not live long enough" here); `foo(x: &S) -> &'static S` for a `for<'cx> fn(&'cx S) -> &'cx S`
/// is fine. A fn item's value elides its result's regions as its signature does
#[test]
fn part19_fn_item_subtyping() {
    let d = check("struct S;\nfn want(f: for<'cx> fn(&'cx S) -> &'static S) {}\nfn baz(x: &S) -> &S { x }\nfn main() { want(baz); }\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("struct S;\nfn want(f: fn(&S) -> &'static S) {}\nfn baz(x: &S) -> &S { x }\nfn main() { want(baz); }\n");
    assert!(d.len() == 1 && d[0].0 == 4, "{d:?}");
    assert_eq!(check("struct S;\nfn foo(x: &S) -> &'static S { panic!() }\nfn baz(x: &S) -> &S { x }\nfn want(f: for<'cx> fn(&'cx S) -> &'cx S) {}\nfn main() { want(foo); want(baz); }\n"), vec![]);
    // (a `let` annotation: the check takes the item's own tree; the local keeps fresh regions for its calls)
    let d = check("struct S;\nfn baz(x: &S) -> &S { x }\nfn main() { let g: for<'cx> fn(&'cx S) -> &'static S = baz; }\n");
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    assert_eq!(check("struct S;\nfn baz(x: &S) -> &S { x }\nfn f<'a>(x: &'a S) -> &'a S {\n    let g: for<'cx> fn(&'cx S) -> &'cx S = baz;\n    g(x)\n}\nfn main() { let s = S; f(&s); }\n"), vec![]);
}

/// A closure's call holds what the arguments its body makes its result of borrow (`|a| Some(a)`, `let some = Some;`),
/// as rustc infers the closure's one signature; not those its body only reads or calls with (`|k| m.get(k)`)
#[test]
fn part19_closure_call_results() {
    let t = "fn temp() -> String { String::from(\"Hello\") }\n";
    for body in ["let some = Some; let g = some(&temp());", "let some = |a| Some(a); let g = some(&temp());", "let some = |a| a; let g: &String = some(&temp());"] {
        let d = check(&format!("{t}fn main() {{\n    {body}\n    println!(\"{{g:?}}\");\n}}\n"));
        assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0716"), "{body}: {d:?}");
    }
    let d = check(&format!("{t}fn main() {{\n    let x = temp(); let some = |a| Some(a); let g = some(&x); drop(x);\n    println!(\"{{g:?}}\");\n}}\n"));
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("cannot move out of `x`"), "{d:?}");
    for body in [
        "let m = vec![temp()]; let look = |k: &String| m.iter().find(|x| *x == k); let g = look(&temp());",
        "let x = temp(); let f = |a: &String| a.len(); let g = f(&x); drop(x);",
        "let x = temp(); let f = |a: &String| -> String { a.clone() }; let g = f(&x); drop(x);",
        "let x = temp(); let f = |a: &String| -> &'static str { if a.is_empty() { \"e\" } else { \"n\" } }; let g = f(&x); drop(x);",
    ] {
        assert_eq!(check(&format!("{t}fn main() {{\n    {body}\n    println!(\"{{g:?}}\");\n}}\n")), vec![], "{body}");
    }
}

/// What a closure's body requires of its parameters (`let y: &'static u32 = x;`, `W::<&'static u32>(x)`) is required
/// of its calls' arguments, as rustc propagates a closure's requirements; and a body that writes a lifetime is checked
/// though nothing else asks for it (nll/user-annotations/adt-tuple-struct-calls.rs)
#[test]
fn part19_closure_requirements() {
    let w = "struct W<T>(T);\nfn id<T>(t: T) -> T { t }\n";
    for body in ["let f = |x| { let _y: &'static u32 = x; };", "let f = |x| W::<&'static u32>(x);", "let f = W::<&'static u32>;", "let f = |x| id::<&'static u32>(x);", "let f = Some::<&'static u32>;"] {
        let d = check(&format!("{w}fn main() {{\n    let c = 66;\n    {body}\n    f(&c);\n}}\n"));
        assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("does not live long enough"), "{body}: {d:?}");
    }
    for body in ["let c = 66; let f = W::<&u32>; f(&c);", "static C: u32 = 1; let f = |x| { let _y: &'static u32 = x; }; f(&C);"] {
        assert_eq!(check(&format!("{w}fn main() {{\n    {body}\n}}\n")), vec![], "{body}");
    }
    // (in a closure's body: `'static`, the function's 'a, and a constructor value that names it)
    let d = check("fn st() {\n    let _c = || {\n        let c = 66;\n        let _y: &'static u32 = &c;\n    };\n}\nfn named<'a>(x: &'a u32) -> &'a u32 {\n    let _c = || {\n        let c = 66;\n        let _y: &'a u32 = &c;\n    };\n    x\n}\nstruct S<T>(T);\nfn ctor<'a>(_: &'a u32) {\n    let _c = || {\n        let c = 66;\n        let f = S::<&'a u32>;\n        f(&c);\n    };\n}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![4, 10, 19], "{d:?}");
    assert_eq!(check("struct S<T>(T);\nfn ok<'a>(c: &'a u32) {\n    let _cl = || {\n        let f = S::<&'a u32>;\n        f(c);\n    };\n}\nfn ok2<'a>(c: &'a u32) -> S<&'a u32> {\n    let f = S::<&'a u32>;\n    f(c)\n}\nfn main() { ok(&1); ok2(&2); }\n"), vec![]);
}

/// An impl's constant whose type names the impl's lifetimes is of the regions its path writes on the self type
/// (nll/user-annotations/constant-in-expr-inherent-1.rs)
#[test]
fn part19_impl_const_regions() {
    let d = check("struct Foo<'a> { x: &'a u32 }\nimpl<'a> Foo<'a> {\n    const C: &'a u32 = &22;\n}\nfn foo<'a>(_: &'a u32) -> &'static u32 {\n    <Foo<'a>>::C\n}\nfn bar<'a>(_: &'a u32) -> &'static u32 {\n    Foo::<'a>::C\n}\nfn ok<'a>(_: &'a u32) -> &'a u32 {\n    <Foo<'a>>::C\n}\nfn ok2() -> &'static u32 {\n    <Foo<'static>>::C\n}\nfn ok3<'a>(_: &'a u32) -> &'static u32 {\n    Foo::C\n}\nfn main() { let f = Foo { x: &1 }; let _ = f.x; }\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![6, 9] && d.iter().all(|x| x.1.contains("lifetime may not live long enough")), "{d:?}");
}

/// The review of #109: a closure's body that writes a lifetime only in a method's turbofish is checked; a `let` that
/// shadows a parameter ends what the body requires of it; an impl's constant takes a lifetime written in a type
/// argument of its path (`Foo::<&'b u32>::C` with `impl<'a> Foo<&'a u32>`)
#[test]
fn part19_review() {
    let d = check("struct S;\nimpl S { fn take<T>(&self, _t: T) {} }\nfn main() {\n    let _c = || {\n        let c = 66;\n        S.take::<&'static u32>(&c);\n    };\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("static C: u32 = 1;\nfn main() {\n    let local = 5;\n    let f = |x| {\n        let x = &C;\n        let _y: &'static u32 = x;\n    };\n    f(&local);\n}\n"), vec![]);
    let d = check("struct Foo<T>(T);\nimpl<'a> Foo<&'a u32> {\n    const C: &'a u32 = &1;\n}\nfn f<'b>(_: &'b u32) -> &'b u32 {\n    Foo::<&'b u32>::C\n}\nfn g<'b>(_: &'b u32) -> &'static u32 {\n    Foo::<&'b u32>::C\n}\nfn main() { let x = 1; f(&x); g(&x); let _ = Foo(&x).0; }\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (a call through a fn pointer local takes the regions its type binds afresh: two calls relate neither argument
    // to the other; the result is still the argument's)
    for src in ["fn id(x: &u32) -> &u32 { x }\nfn f<'a, 'b>(a: &'a u32, b: &'b u32) -> (&'a u32, &'b u32) {\n    let g: fn(&u32) -> &u32 = id;\n    (g(a), g(b))\n}\nfn main() { let (x, y) = (1, 2); let _ = f(&x, &y); }\n", "fn id(x: &u32) -> &u32 { x }\nfn f<'a, 'b>(a: &'a u32, b: &'b u32) -> (&'a u32, &'b u32) {\n    let g = id;\n    (g(a), g(b))\n}\nfn h<'a, 'b>(a: &'a u32, b: &'b u32) -> (&'a u32, &'b u32) {\n    let g: for<'x> fn(&'x u32) -> &'x u32 = id;\n    (g(a), g(b))\n}\nfn main() { let (x, y) = (1, 2); let _ = f(&x, &y); let _ = h(&x, &y); }\n", "fn id<'x>(x: &'x u32) -> &'x u32 { x }\nfn f<'a, 'b>(a: &'a u32, b: &'b u32) -> (&'a u32, &'b u32) {\n    let g: fn(&u32) -> &u32 = id;\n    (g(a), g(b))\n}\nfn h<'a, 'b>(a: &'a u32, b: &'b u32) -> (&'a u32, &'b u32) {\n    let g = id;\n    (g(a), g(b))\n}\nfn main() { let (x, y) = (1, 2); let _ = f(&x, &y); let _ = h(&x, &y); }\n"] {
        assert_eq!(check(src), vec![], "{src}");
    }
    let d = check("fn id(x: &u32) -> &u32 { x }\nfn f1<'a>(x: &'a u32) -> &'static u32 {\n    let g: fn(&u32) -> &u32 = id;\n    g(x)\n}\nfn f2<'a>(x: &'a u32) -> &'static u32 {\n    let g = id;\n    g(x)\n}\nfn f3<'a>(x: &'a u32) -> &'a u32 {\n    let g = id;\n    let _ = g(&5);\n    g(x)\n}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![4, 8] && d.iter().all(|x| x.1.contains("must outlive `'static`")), "{d:?}");
    // (a method's turbofish in a closure's body is required of its calls' arguments too, the receiver's type known)
    let d = check("struct S;\nimpl S {\n    fn take<T>(&self, _: T) {}\n}\nfn main() {\n    let local = 1;\n    let f = |x| S.take::<&'static u32>(x);\n    f(&local);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("struct S;\nimpl S {\n    fn take<T>(&self, _: T) {}\n}\nfn main() {\n    let local = 1;\n    let s = S;\n    let f = |x| s.take::<&u32>(x);\n    f(&local);\n    static C: u32 = 2;\n    let g = |x| S.take::<&'static u32>(x);\n    g(&C);\n}\n"), vec![]);
    // (on a generic impl, the turbofish writes the method's own parameters, which come after the impl's)
    let d = check("struct S<A>(A);\nimpl<A> S<A> {\n    fn take<T>(&self, _: T) {}\n}\nfn main() {\n    let s = S(1u8);\n    let local = 1;\n    let f = |x| s.take::<&'static u32>(x);\n    f(&local);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 9 && d[0].1.contains("does not live long enough"), "{d:?}");
    assert_eq!(check("struct S<A>(A);\nimpl<A> S<A> {\n    fn take<T>(&self, _: T) {}\n}\nfn main() {\n    let s = S(1u8);\n    let local = 1;\n    let f = |x| s.take::<&u32>(x);\n    f(&local);\n}\n"), vec![]);
}

/// A closure whose result is tied to a parameter its bound binds, returning what it captures: the function's region
/// must outlive every region the bound binds ("lifetime may not live long enough", as rustc). Not where the bound has
/// the callee's lifetimes, through which the parameters' implied bounds relate them (closures/explicit-closure-lifetime-
/// issue-87290.rs)
#[test]
fn part20_closure_result_placeholder() {
    let d = check("fn take<F: FnMut(&isize) -> &isize>(_f: F) {}\nfn take2<F: for<'z> FnMut(&'z isize) -> &'z isize>(_f: F) {}\nfn t4<'x>(x: &'x isize) {\n    take(|_z| x);\n}\nfn t5<'x>(x: &'x isize) {\n    take2(|_z| x);\n}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![4, 7] && d.iter().all(|x| x.1.contains("lifetime may not live long enough")), "{d:?}");
    assert_eq!(check("fn take<F: FnMut(&isize) -> &isize>(_f: F) {}\nfn t<'x>(_x: &'x isize) {\n    take(|z| z);\n}\npub fn f<'a>(_: &'a i32, _: impl FnOnce(&mut &mut &'a i32)) {}\npub fn h<'a>(p: &'a i32) {\n    f(p, |x: &mut &mut &'a i32| {\n        let _: &mut &mut &'a i32 = x;\n    })\n}\nfn main() {}\n"), vec![]);
}

/// A closure boxed where a `Box<dyn Fn(..)>` is expected (a `let` annotation, a turbofish): the trait object is its
/// signature, as for `&|x| ..` where a `&dyn Fn(..)` is (lifetimes/closure-lifetime-bounds-10291.rs)
#[test]
fn part20_boxed_closure_signature() {
    let d = check("fn t1<'x>(x: &'x isize) {\n    let _f: Box<dyn for<'z> FnMut(&'z isize) -> &'z isize> = Box::new(|_z| x);\n}\nfn t2<'x>(x: &'x isize) {\n    drop::<Box<dyn for<'z> FnMut(&'z isize) -> &'z isize>>(Box::new(|_z| {\n        x\n    }));\n}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![2, 6], "{d:?}");
    assert_eq!(check("fn t<'x>(_x: &'x isize) {\n    let mut f: Box<dyn FnMut(&isize) -> isize> = Box::new(|z| *z + 1);\n    let g: Box<dyn Fn(&str) -> &str> = Box::new(|s| s.trim());\n    println!(\"{} {}\", f(&1), g(\" a \"));\n}\nfn main() { t(&1); }\n"), vec![]);
}

/// A region of a local of the function a closure is in, captured: from outside, so a region the closure's signature
/// binds must not flow into it ("borrowed data escapes outside of closure", E0521 in rustc: `ay = z` with `ay = &y`
/// captured; regions/regions-nested-fns.rs)
#[test]
fn part20_capture_escape() {
    let d = check("fn ignore<T>(_t: T) {}\nfn a<'x>(x: &'x isize) {\n    let y = 3;\n    let mut ay = &y;\n    ignore::<Box<dyn for<'z> FnMut(&'z isize)>>(Box::new(|z| {\n        ay = z;\n    }));\n    let _ = (x, ay);\n}\nfn b<'x>(x: &'x isize) {\n    let mut ay = x;\n    let mut f = |z: &isize| { ay = z; };\n    f(&1);\n    let _ = ay;\n}\nfn c(v: &mut Vec<&isize>) {\n    let mut f = |z: &isize| { v.push(z); };\n    f(&1);\n}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![6, 12, 17], "{d:?}");
    assert!(d[0].1.contains("escapes outside of closure") && d[1].1.contains("escapes outside of closure"), "{d:?}");
    assert_eq!(check("fn main() {\n    let y = 3;\n    let mut ay = &y;\n    let mut f = |z| { ay = z; };\n    f(&4);\n    let _ = ay;\n}\n"), vec![]);
}

/// A closure's parameter annotated with one of the function's lifetimes where its bound binds the region
/// (`|x: &'x u32|` for `F: Fn(&u32)`): the closure must take every region there ("lifetime may not live long enough",
/// closures/closure-expected-type/expect-region-supply-region-2.rs). Not the callee's lifetime (`F: Fn(&'a u32)`)
#[test]
fn part20_annotated_param_bound() {
    let d = check("fn take<F: Fn(&u32)>(_f: F) {}\nfn take2<'a, F: Fn(&'a u32)>(_: &'a u32, _f: F) {}\nfn take3<F: Fn(&u32) -> u32>(_f: F) {}\nfn g1<'x>(y: &'x u32) { take(|x: &'x u32| { let _ = (x, y); }); }\nfn g2<'x>(y: &'x u32) { take2(y, |x: &'x u32| { let _ = x; }); }\nfn g3() { take(|_x: &'static u32| {}); }\nfn g4() { take3(|x: &u32| *x); take(|_x: &'_ u32| {}); }\nfn main() { g1(&1); g2(&1); g3(); g4(); }\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![4, 6] && d.iter().all(|x| x.1.contains("lifetime may not live long enough")), "{d:?}");
}

#[test]
fn part20_closure_holds_call_args() {
    // (a closure made where nothing was expected, its parameter a reference: the region is its type's, so what its
    // calls' arguments borrow is held while the closure is used)
    let d = check("fn main() {\n    let g = |x| { let _: &u32 = x; };\n    let mut a = 1;\n    g(&a);\n    a = 2;\n    g(&a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("because it is borrowed"), "{d:?}");
    // (also through a reference to it, which the closure it points to holds them for)
    let d = check("fn main() {\n    let g = |x| { let _: &u32 = x; };\n    let mut a = 1;\n    let r = &g;\n    r(&a);\n    a = 2;\n    g(&a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("because it is borrowed"), "{d:?}");
    assert_eq!(check("fn main() {\n    let g = |x| { let _: &u32 = x; };\n    let mut a = 1;\n    let r = &g;\n    r(&a);\n    a = 2;\n    let _ = a;\n}\n"), vec![]);
    // (also called through parentheses: `(g)(&a)`)
    let d = check("fn main() {\n    let g = |x| { let _: &u32 = x; };\n    let mut a = 1;\n    (g)(&a);\n    a = 2;\n    (g)(&a);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let f = |_| {};\n    for a in [1] {\n        f(&a);\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("does not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let m = |x| { let _: &str = x; };\n    let st = String::new();\n    m(st.as_str());\n    drop(st);\n    m(\"b\");\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("because it is borrowed"), "{d:?}");
    // (not a closure whose parameter's type is written (one region for each call), nor one a bound gives its
    // signature, nor one not used after)
    assert_eq!(check("struct S<F: Fn(&u32)> {\n    f: F,\n}\nfn hold<F: Fn(&u32)>(f: F) -> F {\n    f\n}\nfn main() {\n    let k = |x: &u32| { let _ = x; };\n    let mut d = 1;\n    k(&d);\n    d = 2;\n    k(&d);\n    let s = S { f: |x| { let _ = x; } };\n    let mut b = 1;\n    (s.f)(&b);\n    b = 2;\n    (s.f)(&b);\n    let h = hold(|x| { let _ = x; });\n    let mut c = 1;\n    h(&c);\n    c = 2;\n    h(&c);\n    let g = |x| { let _: &u32 = x; };\n    let mut a = 1;\n    g(&a);\n    a = 2;\n    let _ = a;\n}\n"), vec![]);
}

#[test]
fn part20_closure_fnptr_annotated() {
    // (a closure where a fn pointer type binds the region of a parameter annotated with the function's lifetime or
    // 'static: that lifetime would have to be every region there)
    let d = check("fn test<'a>() {\n    let _: fn(&()) = |_: &'a ()| {};\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("fn main() {\n    let g: fn(&u32) = |_: &'static u32| {};\n    g(&1);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (passed to a parameter of such a type, a summary's or not)
    for src in ["fn want(_f: fn(&u32)) {}\nfn main() {\n    want(|_: &'static u32| {});\n}\n", "fn want<'b>(_f: for<'x> fn(&'x u32) -> &'x u32, _y: &'b u32) {}\nfn main() {\n    want(|x: &'static u32| x, &1);\n}\n"] {
        let d = check(src);
        assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{src}: {d:?}");
    }
    // (a closure cast to such a type, as in a `let`)
    let d = check("fn main() {\n    let f = (|_: &'static u32| {}) as fn(&u32);\n    f(&1);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (a struct with no lifetime parameters behind the reference (`&S`): its path's mark stands for no region)
    for (src, line) in [("struct S;\nfn main() {\n    let _: fn(&S) = |_: &'static S| {};\n}\n", 3), ("struct S;\nfn want<'b>(_f: fn(&S), _y: &'b u32) {}\nfn main() {\n    want(|_: &'static S| {}, &1);\n}\n", 4)] {
        let d = check(src);
        assert!(d.len() == 1 && d[0].0 == line && d[0].1.contains("lifetime may not live long enough"), "{src}: {d:?}");
    }
    assert_eq!(check("fn want(_f: fn(&u32)) {}\nfn want2<'a>(_f: fn(&'a u32), _x: &'a u32) {}\nfn want3(_f: fn(&'static u32)) {}\nstatic X: u32 = 1;\nfn main() {\n    want2(|_: &'static u32| {}, &X);\n    want3(|_: &'static u32| {});\n    want(|_: &u32| {});\n}\n"), vec![]);
    let d = check("fn b<'a>(_: &'a ()) {\n    let _: for<'x> fn(&'x ()) = |_: &'static ()| {};\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (not where the type names the lifetime too, nor an annotation that names none)
    assert_eq!(check("fn a<'a>(_: &'a ()) {\n    let _: fn(&'a ()) = |_: &'a ()| {};\n    let _: fn(&()) = |_: &()| {};\n    let _: fn(&'static ()) = |_: &'static ()| {};\n    let f: fn(&'a (), &()) = |_: &'a (), _| {};\n    f(&(), &());\n}\nfn main() {}\n"), vec![]);
}

/// Polonius Alpha (issue #1): a reborrow returned in one branch is live only where what holds it is live, an
/// assignment to a part of a local kills the loans through the reference it held, `for x in v.iter_mut()` with v a
/// reference reborrows `*v`, and a cast to a number holds nothing. Each was checked against rustc 1.102-nightly
/// (Polonius Alpha) and 1.97 (NLL)
#[test]
fn polonius_alpha() {
    let ok = |src: &str| {
        let d = check_ed(src, 2021);
        assert!(d.is_empty(), "{src}\n{d:?}");
    };
    // (a borrow conflict: E0499 / E0502 / E0506 and their kin, not another error)
    let err = |src: &str| {
        let d = check_ed(src, 2021);
        assert!(!d.is_empty() && d.iter().all(|x| x.1.contains("borrow")), "{src}\n{d:?}");
    };
    // (accepted by Alpha, rejected by NLL)
    ok("fn f<'a>(a: &'a mut i32) -> &'a mut i32 {\n    let b = &mut *a;\n    if true { b } else { a }\n}\nfn main() {}\n");
    ok("fn f<'a>(a: &'a mut Option<i32>) -> &'a mut i32 {\n    match a.as_mut() {\n        Some(v) => v,\n        None => {\n            *a = Some(0);\n            a.as_mut().unwrap()\n        }\n    }\n}\nfn main() {}\n");
    ok("fn f<'a>(v: &'a mut Vec<i32>) -> &'a mut i32 {\n    if let Some(x) = v.first_mut() {\n        return x;\n    }\n    v.push(0);\n    v.last_mut().unwrap()\n}\nfn main() {}\n");
    ok("fn g(list: &mut [Option<u8>]) -> &mut Option<u8> {\n    for i in list.iter_mut() {\n        if i.is_none() {\n            return i;\n        }\n    }\n    &mut list[0]\n}\nfn main() {}\n");
    // (accepted by both: assignments to a part of a local, and casts)
    let list = "struct List<T> { value: T, next: Option<Box<List<T>>> }\n";
    ok(&format!("{list}fn f<T>(mut list: Box<&mut List<T>>) -> Vec<&mut T> {{\n    let mut result = vec![];\n    loop {{\n        result.push(&mut list.value);\n        if let Some(n) = list.next.as_mut() {{\n            *list = n;\n        }} else {{\n            return result;\n        }}\n    }}\n}}\nfn main() {{}}\n"));
    ok(&format!("{list}fn f<T>(mut list: (&mut List<T>,)) -> Vec<&mut T> {{\n    let mut result = vec![];\n    loop {{\n        result.push(&mut list.0.value);\n        if let Some(n) = list.0.next.as_mut() {{\n            list.0 = n;\n        }} else {{\n            return result;\n        }}\n    }}\n}}\nfn main() {{}}\n"));
    ok(&format!("{list}fn f<'a>(mut list: &'a mut List<i32>, out: &mut Vec<&'a mut i32>) {{\n    loop {{\n        out.push(&mut list.value);\n        let n = list.next.as_mut().unwrap();\n        list = n;\n    }}\n}}\nfn main() {{}}\n"));
    ok("fn g(list: &mut [u8]) -> &mut u8 { &mut list[0] }\nfn main() {\n    let mut l = [1u8];\n    let v = g(&mut l);\n    assert_eq!(v as *mut u8 as usize, l.as_ptr() as usize);\n}\n");
    // (a returned borrow is not held by `self`, whose type names the region it is returned for: the loop takes a new
    // borrow of `self.b` each turn)
    ok(&"struct D { b: B }\nstruct B;\nimpl B {\n    fn fill(&mut self) -> &[u8] { &[] }\n}\nfn dec(_: &[u8]) -> Option<&str> { None }\nimpl D {\n    fn next<'a>(&'a mut self) -> &'a str {\n        loop {\n            let buf = self.b.fill();\n            if let Some(s) = dec(buf) {\n%s                return s;\n            }\n        }\n    }\n}\nfn main() {}\n".replace("%s", ""));
    // (rejected by both)
    err("fn f<'a>(a: &'a mut i32) -> &'a mut i32 {\n    let b = &mut *a;\n    *a += 1;\n    b\n}\nfn main() {}\n");
    err(&"struct D { b: B }\nstruct B;\nimpl B {\n    fn fill(&mut self) -> &[u8] { &[] }\n}\nfn dec(_: &[u8]) -> Option<&str> { None }\nimpl D {\n    fn next<'a>(&'a mut self) -> &'a str {\n        loop {\n            let buf = self.b.fill();\n            if let Some(s) = dec(buf) {\n%s                return s;\n            }\n        }\n    }\n}\nfn main() {}\n".replace("%s", "                self.b.fill();\n"));
    err("struct D<'a> { r: Option<&'a i32>, v: i32 }\nimpl<'a> D<'a> {\n    fn f(&'a mut self) -> &'a i32 {\n        let x = &self.v;\n        self.r = Some(x);\n        self.v = 1;\n        x\n    }\n}\nfn main() {}\n");
    err("fn f<'a>(a: &'a mut i32) -> &'a mut i32 {\n    let b = &mut *a;\n    let c = b;\n    *a += 1;\n    c\n}\nfn main() {}\n");
    err("fn f<'a>(v: &'a mut Vec<i32>) -> &'a i32 {\n    let r = &v[0];\n    v.push(1);\n    r\n}\nfn main() {}\n");
    err("fn id<T>(t: T) -> T { t }\nfn f<'a>(a: &'a mut i32) -> &'a mut i32 {\n    let r = id(&mut *a);\n    *a += 1;\n    r\n}\nfn main() {}\n");
    err("fn f<'a>(a: &'a mut i32) -> Option<&'a mut i32> {\n    let b = Some(&mut *a);\n    *a += 1;\n    b\n}\nfn main() {}\n");
    err("fn f<'a>(a: &'a mut i32, out: &mut Option<&'a i32>) {\n    let b = &*a;\n    *out = Some(b);\n    *a += 1;\n}\nfn main() {}\n");
    err("fn f(v: &mut Vec<i32>) {\n    for x in v.iter_mut() {\n        v.push(*x);\n    }\n}\nfn main() {}\n");
    err("fn f<'a>(a: &'a mut i32, n: i32) -> &'a mut i32 {\n    let mut i = 0;\n    loop {\n        let b = &mut *a;\n        if i == n {\n            return b;\n        }\n        i += 1;\n        *a += 1;\n        let _ = &b;\n    }\n}\nfn main() {}\n");
    // (rejected by both: two reborrows in one returned value, made in it or through a local read in it (PR #104's
    // review): a reborrow returned is live from where the returned value reads or makes it to the return)
    for body in [
        "(&mut *x, &mut *x)",
        "[&mut *x, &mut *x]",
        "(&mut *x, { *x += 1; &mut *x })",
        "{ let a = &mut *x; (a, &mut *x) }",
        "{ let a = &mut *x; let b = &mut *x; (a, b) }",
        "if true { (&mut *x, &mut *x) } else { panic!() }",
        "match 1 { 1 => (&mut *x, &mut *x), _ => panic!() }",
    ] {
        err(&format!("fn f<'a>(x: &'a mut i32) -> (&'a mut i32, &'a mut i32) {{\n    {body}\n}}\nfn main() {{}}\n").replace("[&mut *x, &mut *x]", "{ let [a, b] = [&mut *x, &mut *x]; (a, b) }"));
    }
    err("fn f<'a>(x: &'a mut i32) -> (&'a mut i32, &'a mut i32) {\n    return (&mut *x, &mut *x);\n}\nfn main() {}\n");
    err("fn f<'a>(x: &'a mut i32) -> Option<(&'a mut i32, &'a i32)> {\n    Some((&mut *x, &*x))\n}\nfn main() {}\n");
    // (the two reborrows are E0499 where rustc reports it, at the second)
    let d = check_ed("fn f<'a>(x: &'a mut i32) -> (&'a mut i32, &'a mut i32) {\n    (&mut *x, &mut *x)\n}\nfn main() {}\n", 2021);
    // (the second `&mut *x` reads `*x` first, which is where zustc reports the conflict with the first, at 2:11)
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("`*x` because it was mutably borrowed (at 2:11)"), "{d:?}");
    // (accepted by Alpha: the returned reborrow made in a loop, which goes on with a new borrow when it does not return)
    ok("fn f<'a>(x: &'a mut i32, n: i32) -> &'a mut i32 {\n    let mut i = 0;\n    loop {\n        let b = &mut *x;\n        if i == n {\n            return b;\n        }\n        i += 1;\n    }\n}\nfn main() {}\n");
}

#[test]
fn part21_fn_item_coerced_wf() {
    // (a fn item coerced to an annotation's fn pointer type: its signature's types are well formed (`&'a &'b ()` needs
    // 'b: 'a), a generic item's type arguments solved from the annotation, a hole's regions fresh and the other parts'
    // as written)
    for src in ["static UNIT: &'static &'static () = &&();\nfn foo<'a, 'b>(_: &'a &'b (), v: &'b u32) -> &'a u32 { v }\nfn bad<'a>(x: &'a u32) -> &'static u32 {\n    let f: fn(_, &'a u32) -> &'static u32 = foo;\n    f(UNIT, x)\n}\nfn main() {}\n", "static UNIT: &'static &'static () = &&();\nfn foo<'a, 'b, T>(_: &'a &'b (), v: &'b T) -> &'a T { v }\nfn bad<'a, T>(x: &'a T) -> &'static T {\n    let f: fn(&'static &'static (), &'a T) -> &'static T = foo;\n    f(UNIT, x)\n}\nfn main() {}\n"] {
        let d = check(src);
        assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("lifetime may not live long enough"), "{src}: {d:?}");
    }
    assert_eq!(check("static UNIT: &'static &'static () = &&();\nfn foo<'a, 'b, T>(_: &'a &'b (), v: &'b T) -> &'a T { v }\nfn id<T>(x: &T) -> &T { x }\nfn pair<T, U>(t: T, _u: U) -> T { t }\nfn ok1<'a, T>(x: &'a T) -> &'a T {\n    let f: fn(_, &'a T) -> &'a T = foo;\n    f(UNIT, x)\n}\nfn ok2<'a>(x: &'a u32) -> &'a u32 {\n    let f: fn(&u32) -> &u32 = id;\n    let g: for<'z> fn(&'z u32) -> &'z u32 = id;\n    let h: fn(&'a u32, u8) -> &'a u32 = pair;\n    let _ = g(x);\n    let _ = h(x, 1);\n    f(x)\n}\nfn ok3(x: &'static u32) -> &'static u32 {\n    let f: fn(&'static &'static (), &'static u32) -> &'static u32 = foo;\n    f(UNIT, x)\n}\nfn main() {\n    let x = 1;\n    ok1(&x);\n    ok2(&x);\n    ok3(&5);\n}\n"), vec![]);
    // (an annotation with holes whose result elides its region keeps the result its parameter's, as one binder: the
    // whole is fresh then, as before)
    let d = check("fn first<T>(x: &u32, _y: T) -> &u32 {\n    x\n}\nfn bad<'a>(x: &'a u32) -> &'static u32 {\n    let f: fn(&u32, _) -> &u32 = first::<u8>;\n    f(x, 0)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (not where the fn pointer type binds a region: rustc accepts it (#25860,
    // implied-bounds-on-nested-references-plus-variance-2.rs))
    assert_eq!(check("static UNIT: &'static &'static () = &&();\nfn foo<'a, 'b, T>(_: &'a &'b (), v: &'b T, _: &()) -> &'a T { v }\nfn bad<'a, T>(x: &'a T) -> &'static T {\n    let f: fn(_, &'a T, &()) -> &'static T = foo;\n    f(UNIT, x, &())\n}\nfn main() {}\n"), vec![]);
}

#[test]
fn part21_deref_coercion_copies_inner() {
    // (a deref coercion of `c: &'1 &'a U` to a `&U` is the reborrow `&**c`: through the shared `&'a U` only, so it
    // holds what `*c` holds, not the loan c is (rustc accepts these; main had false E0515 / "lifetime may not live
    // long enough"))
    assert_eq!(check("fn first<'a>(s: &'a String) -> Option<&'a str> {\n    let parts: Vec<&'a str> = s.split('|').collect();\n    if let [commit, ..] = &parts[..] { Some(commit) } else { None }\n}\nfn second<'a>(s: &'a String) -> &'a str {\n    let parts: Vec<&'a str> = s.split('|').collect();\n    let c: &&'a str = &parts[0];\n    c\n}\nfn main() {\n    let s = String::from(\"a|b\");\n    let _ = first(&s);\n    let _ = second(&s);\n}\n"), vec![]);
    assert_eq!(check("fn f<'a>(x: &'a mut &'static u32) -> &'static u32 { x }\nfn h<'a, 'b>(x: &'a &'b u32) -> &'b u32 { x }\nfn main() {}\n"), vec![]);
    // (not an unsizing to a trait object, which keeps the outer reference: `&'a &'static u32` as `&'static dyn Tr` needs
    // 'a: 'static)
    let d = check("trait Tr {}\nimpl<'x> Tr for &'x u32 {}\nfn bad<'a>(x: &'a &'static u32) -> &'static dyn Tr {\n    x\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (an owned value behind the inner reference is still the local's: E0515)
    let d = check("fn bad1<'a>(s: &'a String) -> &'a String {\n    let v: Vec<String> = vec![s.clone()];\n    let c: &String = &v[0];\n    let d: &&String = &c;\n    d\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5, "{d:?}");
}

#[test]
fn part21_ref_mut_let_invariant() {
    // (a `ref mut` binding of a place in a `let` with a type written: the place is of that type both ways, a `&mut`
    // being invariant (mir/field-projection-mutating-context2.rs); `(x.0)(&string)` is a deref coercion, no borrow of
    // string kept)
    let d = check("struct Foo<T>(T);\nfn foo<'a>(mut x: Foo<fn(&'a str)>, string: &'a str) {\n    let Foo(ref mut y): Foo<fn(&'static str)> = x;\n    *y = |_s| {};\n    (x.0)(&string);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("struct Foo<T>(T);\nfn ok1<'a>(mut x: Foo<fn(&'a str)>) {\n    let Foo(ref mut y): Foo<fn(&'a str)> = x;\n    let _ = y;\n    let mut t = (1u32, 2u32);\n    let (ref mut a, _b): (u32, u32) = t;\n    *a = 3;\n    let s: &'static u32 = &5;\n    let (ref mut r,): (&'a u32,) = (s,);\n    let _ = r;\n    let _ = &mut x;\n}\nfn bad2<'a, 'b: 'a>(mut x: Foo<&'b u32>) {\n    let Foo(ref mut y): Foo<&'a u32> = x;\n    let _ = y;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 14 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
}

#[test]
fn part22_projection_annotations() {
    // (a projection written as a `let`'s type whose associated type's value names lifetimes: of the regions the value
    // writes, the impl's lifetimes those the self type writes (nll/user-annotations/normalization.rs))
    let d = check("trait Foo { type Out; }\nimpl Foo for () { type Out = &'static u32; }\nimpl<'a> Foo for &'a () { type Out = &'a u32; }\nfn main() {\n    let a = 22;\n    let _: <() as Foo>::Out = &a;\n}\nfn h() {\n    let a = 22;\n    let _: <&'static () as Foo>::Out = &a;\n}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![6, 10] && d.iter().all(|x| x.1.contains("does not live long enough")), "{d:?}");
    let d = check("trait Foo { type Out; }\nimpl<'a> Foo for &'a () { type Out = &'a u32; }\nfn g<'a>(r: &'a u32) -> &'static u32 {\n    let x: <&'a () as Foo>::Out = r;\n    let y: <&'static () as Foo>::Out = x;\n    y\n}\nfn main() { let v = 1; g(&v); }\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("trait Foo { type Out; }\nimpl Foo for () { type Out = &'static u32; }\nimpl<'a> Foo for &'a () { type Out = &'a u32; }\nstatic S: u32 = 3;\nfn f<'a>(r: &'a u32) -> &'a u32 {\n    let x: <&'a () as Foo>::Out = r;\n    let a = 22;\n    let _: <&'_ () as Foo>::Out = &a;\n    let _: <() as Foo>::Out = &S;\n    let _: <() as Foo>::Out = &5;\n    x\n}\nfn main() { let v = 1; f(&v); }\n"), vec![]);
    // (a parameter of such a type: what its value implies is known (`'a: 'static` here); main had a false error)
    assert_eq!(check("trait Tr {\n    type Ty;\n}\nimpl<T: 'static> Tr for T {\n    type Ty = &'static T;\n}\nfn test<'a>(x: &'a u8, _wf: <&'a u8 as Tr>::Ty) -> &'static u8 { x }\nfn main() {\n    let _: &'static u8 = test(&3, &&3);\n}\n"), vec![]);
}

#[test]
fn part22_projection_review() {
    // (a `let` with no initializer: the projection's tree, which the assignment flows into)
    let d = check("trait Foo { type Out; }\nimpl Foo for () { type Out = &'static u32; }\nfn f(r: &u32) {\n    let x: <() as Foo>::Out;\n    x = r;\n    let _ = x;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("trait Foo { type Out; }\nimpl Foo for () { type Out = &'static u32; }\nstatic S: u32 = 3;\nfn f() -> u32 {\n    let x: <() as Foo>::Out;\n    x = &S;\n    *x\n}\nfn main() {}\n"), vec![]);
    // (an impl's type argument in its self type: the written type's part there, `&'static u32` of `W<&'static u32>`)
    let d = check("trait Foo { type Out; }\nstruct W<T>(T);\nimpl<T> Foo for W<T> { type Out = (&'static u32, T); }\nfn f(anchor: &'static u32) {\n    let local = 3;\n    let _: <W<&'static u32> as Foo>::Out = (anchor, &local);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("does not live long enough"), "{d:?}");
    // (an impl's lifetime next to a type argument with regions: matched where the self type has it)
    let d = check("trait Foo { type Out; }\nstruct W<'a, T>(&'a u8, T);\nimpl<'a, T> Foo for W<'a, T> { type Out = &'a u32; }\nfn f(r: &u32) {\n    let _: <W<'static, &'static u32> as Foo>::Out = r;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (an impl's lifetime in a fn pointer self type)
    let d = check("trait Foo { type Out; }\nimpl<'a> Foo for fn(&'a ()) { type Out = &'a u32; }\nfn f(r: &u32) {\n    let _: <fn(&'static ()) as Foo>::Out = r;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("trait Foo { type Out; }\nimpl<'a> Foo for fn(&'a ()) { type Out = &'a u32; }\nstatic S: u32 = 1;\nfn f<'b>(r: &'b u32) -> &'b u32 {\n    let _: <fn(&'static ()) as Foo>::Out = &S;\n    let x: <fn(&'b ()) as Foo>::Out = r;\n    x\n}\nfn main() {}\n"), vec![]);
    assert_eq!(check("trait Foo { type Out; }\nstruct W<T>(T);\nimpl<T> Foo for W<T> { type Out = (&'static u32, T); }\nstruct V<'a, T>(&'a u8, T);\nimpl<'a, T> Foo for V<'a, T> { type Out = &'a u32; }\nstatic S: u32 = 3;\nfn f<'b>(anchor: &'static u32, r: &'b u32) -> &'b u32 {\n    let _: <W<&'static u32> as Foo>::Out = (anchor, anchor);\n    let _: <W<&'b u32> as Foo>::Out = (anchor, r);\n    let _: <V<'static, &'b u32> as Foo>::Out = &S;\n    let x: <V<'b, &'static u32> as Foo>::Out = r;\n    x\n}\nfn main() {}\n"), vec![]);
}

#[test]
fn part22_closure_fnptr_named() {
    // (a fn pointer type that names a lifetime where a closure annotates its parameter: the type's region outlives
    // the annotation's, as the closure takes what the type gives. In a cast of a local, a `let`, a cast and a call)
    for (src, line) in [
        ("fn f<'a>(_: &'a u32) {\n    let c = |_: &'static u32| {};\n    let _ = c as fn(&'a u32);\n}\nfn main() {}\n", 3),
        ("fn f<'a>(_: &'a u32) {\n    let _: fn(&'a u32) = |_: &'static u32| {};\n}\nfn main() {}\n", 2),
        ("fn f<'a>(_: &'a u32) {\n    let _ = (|_: &'static u32| {}) as fn(&'a u32);\n}\nfn main() {}\n", 2),
        ("fn want<'a>(_: &'a u32, _f: fn(&'a u32)) {}\nfn f<'a>(x: &'a u32) {\n    want(x, |_: &'static u32| {});\n}\nfn main() {}\n", 3),
    ] {
        let d = check(src);
        assert!(d.len() == 1 && d[0].0 == line && d[0].1.contains("lifetime may not live long enough"), "{src}: {d:?}");
    }
    let g = |w: &str| format!("fn f<'a, 'b>(_: &'a u32, _: &'b u32) where {w} {{\n    let _: fn(&'a u32) = |_: &'b u32| {{}};\n}}\nfn g<'a, 'b>(_: &'a u32, _: &'b u32) where {w} {{\n    let c = |_: &'b u32| {{}};\n    let _ = c as fn(&'a u32);\n}}\nfn main() {{}}\n");
    let d = check(&g("'b: 'a"));
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![2, 6], "{d:?}");
    assert_eq!(check(&g("'a: 'b")), vec![]);
    assert_eq!(check("fn f<'a>(_: &'a u32) {\n    let c = |_: &'a u32| {};\n    let _ = c as fn(&'static u32);\n    let _: fn(&'a u32) = |_: &'a u32| {};\n}\nfn main() {}\n"), vec![]);
}

#[test]
fn part22_local_closure_cast() {
    // (a closure held by a local and cast to a fn pointer type: its annotated parameters against the regions the type
    // binds, reported at the cast (E0308 in rustc, there))
    let d = check("fn main() {\n    let c = |_: &'static u32| {};\n    let f = c as fn(&u32);\n    f(&1);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    assert_eq!(check("fn main() {\n    let c = |_: &u32| {};\n    let f = c as fn(&u32);\n    f(&1);\n    let d = |_: &'static u32| {};\n    let g = d as fn(&'static u32);\n    g(&1);\n}\n"), vec![]);
}

#[test]
fn part23_placeholder_outlived_from_outside() {
    // (a closure's region from outside exempts only a placeholder it is known to outlive: `&'b u32` beside `&'a u32`
    // implies nothing of 'b, so returning a capture for every 'b needs 'x: 'static)
    one_error("fn take<'a, F>(_x: &'a u32, _f: F) where F: for<'b> FnOnce(&'b u32, &'a u32) -> &'b u32 {}\nfn bad<'x>(x: &'x u32) {\n    take(x, |_, _| x);\n}\nfn main() {}\n", 3);
}

#[test]
fn part23_placeholder_bound_type_args() {
    // (a method of a placeholder's bound whose trait's type arguments name lifetimes: the call's argument is of the
    // bound's `&'a isize` (E0597), and the result of a `for<'a>` bound's holds what the argument borrows (E0506))
    let src = |b: &str| format!("trait FnLike<A, R> {{ fn call(&self, arg: A) -> R; }}\nfn c<'a, F: FnLike<{b}>>(f: F) {{\n    let x: isize = 3;\n    let y = f.call(&x);\n    drop(y);\n}}\nfn main() {{}}\n");
    for b in ["&'a isize, &'a isize", "&'a isize, u8"] {
        let d = check(&src(b));
        assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("does not live long enough"), "{b}: {d:?}");
    }
    let d = check("trait FnLike<A, R> { fn call(&self, arg: A) -> R; }\nfn call_repeatedly<F>(f: F) where F: for<'a> FnLike<&'a isize, &'a isize> {\n    let mut x = 3;\n    let y = f.call(&x);\n    x = 5;\n    drop(y);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot assign to `x` because it is borrowed"), "{d:?}");
    assert_eq!(check("trait FnLike<A, R> { fn call(&self, arg: A) -> R; }\nfn a<'a, F: FnLike<&'a isize, &'a isize>>(f: F, x: &'a isize) -> &'a isize {\n    f.call(x)\n}\nfn b<F>(f: F) where F: for<'a> FnLike<&'a isize, &'a isize> {\n    let mut x: isize = 3;\n    f.call(&x);\n    f.call(&x);\n    x = 5;\n    let _ = x;\n}\nfn c<'z, F: FnLike<&'z isize, u8>>(f: F) -> u8 {\n    static S: isize = 1;\n    f.call(&S)\n}\nfn take<'a, F>(_x: &'a u32, _f: F) where F: for<'b> FnOnce(&'b u32, &'a u32) -> &'b u32 {}\nfn d<'x>(x: &'x u32) {\n    take(x, |y, _| y);\n}\nfn e<'a>(_: &'a i32, _: impl FnOnce(&mut &mut &'a i32)) {}\nfn g<'a>(p: &'a i32) { e(p, |x: &mut &mut &'a i32| { let _: &mut &mut &'a i32 = x; }) }\nfn main() {}\n"), vec![]);
}

#[test]
fn part23_diverging_let_annotation_wf() {
    // (a `let` whose initializer diverges: its annotation's type is well formed all the same, a named binding and `_`
    // (regions/regions-outlives-projection-container-wc.rs))
    let d = check("struct S<'b>(&'b ());\nfn f<'a, 'b>() {\n    let _: &'a S<'b> = loop {};\n}\nstruct W<T>(T);\nfn g<'a, 'b>() {\n    let _: &'a W<S<'b>> = loop {};\n}\nfn h<'a, 'b>() {\n    let _x: &'a &'b () = panic!();\n}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![3, 7, 10] && d.iter().all(|x| x.1.contains("lifetime may not live long enough")), "{d:?}");
    assert_eq!(check("struct S<'b>(&'b ());\nfn f<'a, 'b: 'a>() {\n    let _: &'a S<'b> = loop {};\n}\nfn g<'a, 'b: 'a>() {\n    let _x: &'a &'b () = panic!();\n}\nfn main() {}\n"), vec![]);
}

#[test]
fn part23_alias_param_twice() {
    // (an annotation that is a type alias with a hole, its parameter twice in it: one tree for both occurrences
    // (nll/relate_tys/var-appears-twice.rs). Under `Cell` they are invariant: `&'z u32` must be `&'static u32`)
    let d = check("use std::cell::Cell;\ntype DoubleCell<A> = Cell<(A, A)>;\nfn make_cell<'b>(x: &'b u32) -> Cell<(&'static u32, &'b u32)> { panic!() }\nfn f<'z>(z: &'z u32) {\n    let _x: DoubleCell<_> = make_cell(z);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (a covariant alias, and the parameter written: accepted, as in rustc)
    assert_eq!(check("use std::cell::Cell;\ntype DoubleCell<A> = Cell<(A, A)>;\ntype DoublePair<A> = (A, A);\nfn make_cell<'b>(x: &'b u32) -> Cell<(&'static u32, &'b u32)> { panic!() }\nfn make_pair<'b>(x: &'b u32) -> (&'static u32, &'b u32) { panic!() }\nfn f<'z>(z: &'z u32) {\n    let b = 44;\n    let _y: DoublePair<_> = make_pair(&b);\n    let _w: DoubleCell<&'static u32> = make_cell(&22);\n    let _v: DoublePair<_> = make_pair(z);\n}\nfn main() {}\n"), vec![]);
}

#[test]
fn part23_review() {
    // (a diverging `let` whose annotated type needs Drop: well formed too)
    let d = check("struct S<'b>(&'b ());\nfn f<'a, 'b>() {\n    let _x: Box<&'a S<'b>> = loop {};\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (two bounds of one trait: the bound a call takes is not known, its arguments' regions are not taken (rustc
    // accepts it))
    assert_eq!(check("trait Take<A> { fn take(&self, a: A); }\nfn g<'a, 'b, F: Take<&'a u32> + Take<&'b u64>>(f: F, x: &'b u64) {\n    f.take(x);\n}\nfn main() {}\n"), vec![]);
    // (a bound whose argument binds its own lifetimes (`for<'b> fn(&'b u32)`), and a literal that meets the method's
    // types: accepted, as in rustc)
    assert_eq!(check("trait Get<A> { fn get(&self) -> A; }\nfn h<'z, F: Get<for<'b> fn(&'b u32) -> &'b u32>>(f: F, z: &'z u32) -> &'z u32 {\n    let p = f.get();\n    let a = 1;\n    let _ra = p(&a);\n    p(z)\n}\nfn k<'z, F: Get<&'z u32>>(f: F) {\n    let a = 1;\n    let r = f.get();\n    let _ = *r == a;\n}\nfn main() {}\n"), vec![]);
    // (and a trait whose method writes its parameter's type: the literal's local is of it; main had a false E0308)
    assert_eq!(check("trait Get<'a> { fn get(&self, arg: &'a isize) -> &'a isize; }\nfn c<F>(f: F) where F: for<'a> Get<'a> {\n    let x = 3;\n    let y = f.get(&x);\n    drop(y);\n}\nfn main() {}\n"), vec![]);
    // (`for` in a type's name (`Bufferfor<'a>`) binds nothing: the bound names 'a, and the fn's body is checked (its
    // double move, E0382 in rustc))
    let d = check("struct Bufferfor<'a>(&'a u32);\ntrait Get<A> { fn get(&self) -> A; }\nfn bad<'a, F: Get<Bufferfor<'a>>>(f: F) {\n    f.get();\n    let _x = f;\n    let _y = f;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("use of moved value"), "{d:?}");
}

#[test]
fn part24_static_bound_adt_wf() {
    // (a struct that bounds its type parameter by 'static: a type written in a generic fn nothing calls needs
    // T: 'static (E0310), in an annotation and in a path's type arguments
    // (regions/wf-bound-region-in-local-issue-115175.rs))
    let d = check("struct Static<T: 'static>(T);\nfn a<T>() {\n    let _: Option<Static<T>> = None;\n}\nfn b<T>() {\n    let _ = None::<Static<T>>;\n}\nfn c<T>() {\n    let _ = Vec::<Static<T>>::new();\n}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![3, 6, 9] && d.iter().all(|x| x.1.contains("E0310")), "{d:?}");
    assert_eq!(check("struct Static<T: 'static>(T);\nfn d<T: 'static>() {\n    let _ = None::<Static<T>>;\n    let _: Option<Static<T>> = None;\n}\nfn main() {}\n"), vec![]);
}

#[test]
fn part24_mut_binding_resets_ref_mode() {
    // (`mut m` where the default binding mode is by reference is by value again before edition 2024: a value that is
    // not Copy is moved out of the reference (E0507, rfc-2005-default-binding-mode/for.rs), and a Copy one is the
    // value's type (`m += 1` of a u8, not a `&u8`))
    let src = "struct Foo {}\nfn a() {\n    let tup = (Foo {}, Foo {});\n    let (_n, mut _m) = &tup;\n}\nfn b() {\n    let tups = vec![(Foo {}, Foo {})];\n    for (_n, mut _m) in &tups {}\n}\nfn main() {}\n";
    let d = check_ed(src, 2021);
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![4, 8] && d.iter().all(|x| x.1.contains("E0507")), "{d:?}");
    assert_eq!(check_ed("struct Foo {}\nfn c() {\n    let tup = (Foo {}, Foo {});\n    let (_n, _m) = &tup;\n}\nfn d() {\n    let tup = (1u8, 2u8);\n    let (_n, mut m) = &tup;\n    m += 1;\n    let _ = m;\n}\nfn main() {}\n", 2021), vec![]);
}

#[test]
fn part24_projection_impl_header_outlives() {
    // (a projection requires what the header of the impl it selects declares of the impl's lifetimes: `'d: 'a` of
    // `<&'a () as Visitor<'d>>`, as the result's type, a `let`'s and a parameter's (nll/normalization-bounds-error.rs))
    let src = "trait Visitor<'d> { type Value; }\nimpl<'a, 'd: 'a> Visitor<'d> for &'a () { type Value = (); }\nfn visit_seq<'d, 'a: 'd>() -> <&'a () as Visitor<'d>>::Value {}\nfn visit_let<'d, 'a: 'd>() { let _: <&'a () as Visitor<'d>>::Value = (); }\nfn visit_arg<'d, 'a: 'd>(_x: <&'a () as Visitor<'d>>::Value) {}\nfn main() {}\n";
    let d = check(src);
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![3, 4, 5] && d.iter().all(|x| x.1.contains("lifetime may not live long enough")), "{d:?}");
    // (the impl's where clause, and 'static in the trait path; what the function knows, or a lifetime the path
    // elides, meets it)
    let d = check("trait Visitor<'d> { type Value; }\nimpl<'a, 'd> Visitor<'d> for &'a () where 'd: 'a { type Value = (); }\nfn a1<'d: 'a, 'a>() -> <&'a () as Visitor<'d>>::Value {}\nfn a2<'d, 'a>() { let x: <&'a () as Visitor<'d>>::Value = (); let _ = x; }\nfn a3<'a>() { let _: <&'a () as Visitor<'_>>::Value = (); }\nfn a5<'a>() -> <&'a () as Visitor<'static>>::Value {}\nfn a6<'a>() -> <&'static () as Visitor<'a>>::Value {}\nfn main() {}\n");
    assert!(d.iter().map(|x| x.0).collect::<Vec<_>>() == vec![4, 7] && d.iter().all(|x| x.1.contains("lifetime may not live long enough")), "{d:?}");
}

#[test]
fn part24_review() {
    // (a method call's type arguments are well formed too: `r.use_ty::<Static<T>>()` requires T: 'static (E0310))
    let d = check("struct Static<T: 'static>(T);\nstruct Receiver;\nimpl Receiver { fn use_ty<U>(&self) {} }\nfn bad<T>(r: &Receiver) { r.use_ty::<Static<T>>(); }\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0310"), "{d:?}");
    // (the header's `'d: 'a` holds where the associated type's value is not followed (`<() as Base>::Out`), and of a
    // projection within the written type (`Option<<&'a () as Visitor<'d>>::Value>`))
    let d = check("trait Base { type Out; }\nimpl Base for () { type Out = (); }\ntrait Visitor<'d> { type Value; }\nimpl<'a, 'd: 'a> Visitor<'d> for &'a () { type Value = <() as Base>::Out; }\nfn visit_seq<'d, 'a: 'd>() -> <&'a () as Visitor<'d>>::Value {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    let d = check("trait Visitor<'d> { type Value; }\nimpl<'a, 'd: 'a> Visitor<'d> for &'a () { type Value = (); }\nfn visit_seq<'d, 'a: 'd>() -> Option<<&'a () as Visitor<'d>>::Value> { None }\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (a `mut` binding of a `&mut` through a shared reference moves it out (E0507))
    let d = check_ed("fn main() {\n    let mut n = 0;\n    let pair = (&mut n,);\n    let (mut r,) = &pair;\n    *r = 1;\n}\n", 2021);
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0507"), "{d:?}");
    // (accepted, as in rustc)
    let src = "struct Static<T: 'static>(T);\nstruct Receiver;\nimpl Receiver { fn use_ty<U>(&self) {} }\nfn good<T: 'static>(r: &Receiver) { r.use_ty::<Static<T>>(); }\ntrait Visitor<'d> { type Value; }\nimpl<'a, 'd: 'a> Visitor<'d> for &'a () { type Value = (); }\nfn ok<'d: 'a, 'a>() -> Option<<&'a () as Visitor<'d>>::Value> { None }\nfn ok2<'d: 'a, 'a>(_x: (u8, <&'a () as Visitor<'d>>::Value)) {}\nfn main() {\n    let t = (1u8, &2u8);\n    let (mut a, mut b) = &t;\n    a += 1;\n    b = &3;\n    let _ = (a, b);\n}\n";
    assert_eq!(check_ed(src, 2021), vec![]);
}

#[test]
fn part24_review_2() {
    // (a `mut` binding of a `&mut` within a Copy enum (`Option<&mut i32>`) through a shared reference moves it out
    // (E0507))
    let d = check_ed("fn main() {\n    let mut n = 0;\n    let t = ((Some(&mut n),),);\n    let ((mut x,),) = &t;\n    let _ = x.take();\n}\n", 2021);
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0507"), "{d:?}");
    // (a projection within a `let`'s annotation with no initializer requires its impl's header outlives)
    let d = check("trait Visitor<'d> { type Value; }\nimpl<'a, 'd: 'a> Visitor<'d> for &'a () { type Value = (); }\nfn bad<'d, 'a: 'd>() {\n    let _x: Option<<&'a () as Visitor<'d>>::Value>;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (accepted, as in rustc: a Copy struct and an `Option<&u8>`)
    assert_eq!(check_ed("#[derive(Clone, Copy)]\nstruct P(u8);\nfn main() {\n    let t = (P(1), Some(&2u8));\n    let (mut a, mut b) = &t;\n    a.0 += 1;\n    b = None;\n    let _ = (a, b);\n}\n", 2021), vec![]);
}

#[test]
fn part25_placeholder_assoc_type_outlives() {
    // (a placeholder's associated type is a type of its own: what it outlives is what the trait declares, what the
    // function's where clauses say, and what its parameter and the bound's lifetimes all outlive
    // (nll/ty-outlives/projection-where-clause-*.rs))
    let bar = "fn bar<'a, T>() -> &'a () where T: 'a { &() }\nfn main() {}\n";
    let d = check(&format!("trait MyTrait<'a> {{ type Output; }}\nfn foo<'a, T>() -> &'a () where T: MyTrait<'a> {{\n    bar::<T::Output>()\n}}\n{bar}"));
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("the associated type `<T as MyTrait<'a>>::Output` may not live long enough") && d[0].1.contains("E0309"), "{d:?}");
    let d = check(&format!("trait MyTrait {{ type Output; }}\nfn foo<'a, T>() -> &'a () where T: MyTrait {{\n    bar::<T::Output>()\n}}\n{bar}"));
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0309"), "{d:?}");
    // (a where clause of another lifetime that is not known to outlive 'a)
    let d = check(&format!("trait MyTrait<'a> {{ type Output; }}\nfn foo1<'a, 'b, T>() -> &'a () where T: MyTrait<'a>, <T as MyTrait<'a>>::Output: 'b {{\n    bar::<T::Output>()\n}}\n{bar}"));
    assert!(d.len() == 1 && d[0].0 == 3, "{d:?}");
    // (accepted: the where clause, with `'b: 'a`, or of 'a; the trait's `type Output: 'a`; `T: 'a` with the trait's
    // 'a; `T: 'static` and a trait with no lifetime)
    for src in [
        "trait MyTrait<'a> { type Output; }\nfn foo2<'a, 'b, T>() -> &'a () where T: MyTrait<'a>, <T as MyTrait<'a>>::Output: 'b, 'b: 'a {\n    bar::<T::Output>()\n}\n",
        "trait MyTrait<'a> { type Output; }\nfn foo<'a, T>() -> &'a () where T: MyTrait<'a>, T::Output: 'a {\n    bar::<T::Output>()\n}\n",
        "trait MyTrait<'a> { type Output: 'a; }\nfn foo<'a, T>() -> &'a () where T: MyTrait<'a> {\n    bar::<T::Output>()\n}\n",
        "trait MyTrait<'a> { type Output; }\nfn foo<'a, T>() -> &'a () where T: MyTrait<'a> + 'a {\n    bar::<T::Output>()\n}\n",
        "trait MyTrait { type Output; }\nfn foo<'a, T: 'static>() -> &'a () where T: MyTrait {\n    bar::<T::Output>()\n}\n",
    ] {
        assert_eq!(check(&format!("{src}{bar}")), vec![], "{src}");
    }
}

#[test]
fn part25_path_type_arg_ref_to_placeholder() {
    // (a type argument a path writes that is a reference to a placeholder's type is well formed: `wf::<&'x T>()`
    // requires T: 'x, which `&'x T::Foo` does not imply (regions/regions-implied-bounds-projection-gap-1.rs); `&'x T`
    // and `&'x T::Foo` as the parameter's type imply what their own use needs)
    let src = "trait Trait1<'x> { type Foo; }\nfn wf<T>() {}\nfn func<'x, T: Trait1<'x>>(_t: &'x T::Foo) {\n    wf::<&'x T>();\n}\nfn caller2<'x, T: Trait1<'x>>(_t: &'x T) {\n    wf::<&'x T::Foo>();\n}\nfn caller3<'x, T: Trait1<'x>>(_t: &'x T::Foo) {\n    wf::<&'x T::Foo>();\n}\nfn main() {}\n";
    let d = check(src);
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("the parameter type `T` may not live long enough"), "{d:?}");
    // (a where clause on a type the summaries do not read (a type alias of a projection): nothing is required of the
    // placeholder's associated types (nll/ty-outlives/issue-53789-1.rs))
    let src = "trait Tr<'a> { type S; }\ntype SFor<A> = <A as Tr<'static>>::S;\nfn need<K: 'static>() {}\nfn f<A: Tr<'static>>() where SFor<A>: 'static {\n    need::<A::S>();\n}\nfn main() {}\n";
    assert_eq!(check(src), vec![]);
}

#[test]
fn part25_where_clause_self_invariant() {
    // (a callee's `G: Get` filled by a type only the caller's where clause makes implement Get (`&'max G: Get`, no
    // impl for a reference): the type is the where clause's, and `Self` is invariant (variance/variance-*-self-trait-
    // match.rs))
    let src = "trait Get { fn get(&self); }\nfn get_min_from_max<'min, 'max, G>() where 'max: 'min, G: 'max, &'max G: Get {\n    impls_get::<&'min G>();\n}\nfn impls_get<G>() where G: Get {}\nfn main() {}\n";
    let d = check(src);
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (the same lifetime, or two known to be equal: accepted, as in rustc)
    let src = "trait Get { fn get(&self); }\nfn ok1<'a, G: 'a>() where &'a G: Get { impls_get::<&'a G>(); }\nfn ok2<'min, 'max, G: 'max>() where 'max: 'min, 'min: 'max, &'max G: Get { impls_get::<&'min G>(); }\nfn impls_get<G>() where G: Get {}\nfn main() {}\n";
    assert_eq!(check(src), vec![]);
}

#[test]
fn part25_assoc_placeholder_copy() {
    // (a placeholder's associated type is taken for Copy: its trait's where clause may make it so
    // (associated-type-bounds/assoc-type-bound-through-where-clause.rs, rustc accepts it))
    assert_eq!(check("trait Op where Self::Output: Copy { type Output; }\nfn duplicate<T: Op>(x: T::Output) -> (T::Output, T::Output) {\n    (x, x)\n}\nfn main() {}\n"), vec![]);
}

#[test]
fn part25_assoc_placeholder_as_typeck_finds_it() {
    // (an associated type an impl gives (`impl<T> Trait for T { type Assoc = T; }`) is that type, not a placeholder of
    // its own, in the region check as in typeck: the body's values are of it)
    assert_eq!(check("trait Trait { type Assoc; }\nimpl<T> Trait for T { type Assoc = T; }\nfn id<'a, T: Trait>(x: T::Assoc, _r: &'a u8) -> T::Assoc { let y = x; y }\nfn main() {}\n"), vec![]);
    // (the message names the placeholder's type by its parameter: no `#`)
    let d = check("trait MyTrait { type Output; }\nfn foo<'a, T>() -> &'a () where T: MyTrait {\n    bar::<T::Output>()\n}\nfn bar<'a, T>() -> &'a () where T: 'a { &() }\nfn main() {}\n");
    assert!(d.len() == 1 && !d[0].1.contains('#'), "{d:?}");
}

#[test]
fn part25_assoc_value_trait_bound() {
    // (an impl's associated type's value outlives what its trait's declaration bounds it by, with the trait's
    // lifetimes the impl's trait path writes (E0477, regions/regions-assoc-type-*-bound-in-trait-not-met.rs))
    let d = check("trait Foo<'a> { type Value: 'a; }\nimpl<'a> Foo<'static> for &'a i32 {\n    type Value = &'a i32;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0477"), "{d:?}");
    let d = check("trait Foo { type Value: 'static; }\nimpl<'a> Foo for &'a i32 {\n    type Value = &'a i32;\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("E0477"), "{d:?}");
    // (accepted: the same lifetime, one the header makes outlive it, 'static, a lifetime a `for<..>` binds)
    assert_eq!(check("trait Foo<'a> { type Value: 'a; }\nimpl<'a> Foo<'a> for &'a i16 { type Value = &'a i32; }\nimpl<'a: 'b, 'b> Foo<'b> for &'a i64 { type Value = &'a i32; }\nimpl<'a> Foo<'a> for &'a u8 { type Value = &'static i32; }\nimpl<'a> Foo<'a> for &'a u16 { type Value = for<'x> fn(&'x u8); }\nfn main() {}\n"), vec![]);
}

#[test]
fn part25_review() {
    // (a callee's `V::Out` is of the placeholder that fills V (`take::<U>(x)`), not the caller's parameter at V's
    // place: accepted, as in rustc; and a real requirement through it is still found)
    assert_eq!(check("trait Tr { type Out; }\nfn take<V: Tr>(_: &V::Out) {}\nfn f<'a, T: Tr, U: Tr>(x: &'a U::Out) {\n    take::<U>(x);\n}\nfn main() {}\n"), vec![]);
    let d = check("trait Tr { type Out; }\nfn take<'b, V: Tr>(_: &'b V::Out) -> &'b V::Out { loop {} }\nfn bad<'a, T: Tr, U: Tr>(x: &'a U::Out) -> &'static U::Out {\n    take::<U>(x)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (a type alias in the associated type's value is not read for E0477: `type Erased<'a> = ();` names a lifetime
    // its body does not use)
    assert_eq!(check("type Erased<'a> = ();\ntrait Foo { type Value: 'static; }\nimpl<'a> Foo for &'a i32 {\n    type Value = Erased<'a>;\n}\nfn main() {}\n"), vec![]);
    // (an impl in a fn body: E0477 at the line it is written on)
    let d = check("trait Foo { type Value: 'static; }\nfn main() {\n    let _x = 1;\n    impl<'a> Foo for &'a i32 {\n        type Value = &'a i32;\n    }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("E0477"), "{d:?}");
}

#[test]
fn part25_review_2() {
    // (two where clauses on the same type with the same trait, other arguments: which one meets a callee's bound is
    // not known from the trees, so neither is taken; one that fits alone still is)
    assert_eq!(check("trait Get<T> {}\nfn need8<X: Get<u8>>() {}\nfn need16<X: Get<u16>>() {}\nfn caller<'a, 'b, G: 'a + 'b>()\nwhere\n    &'a G: Get<u8>,\n    &'b G: Get<u16>,\n{\n    need8::<&'a G>();\n    need16::<&'b G>();\n}\nfn main() {}\n"), vec![]);
    let d = check("trait Get<T> {}\nfn need8<X: Get<u8>>() {}\nfn caller<'a, 'b: 'a, G: 'b>()\nwhere\n    &'b G: Get<u8>,\n{\n    need8::<&'a G>();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("lifetime may not live long enough"), "{d:?}");
    // (E0477: what the trait's type arguments in the impl's header imply is known (`&'a &'b ()`: 'b: 'a))
    assert_eq!(check("trait Foo<'a, T> { type Value: 'a; }\nimpl<'a, 'b> Foo<'a, &'a &'b ()> for () { type Value = &'b i32; }\nfn main() {}\n"), vec![]);
    let d = check("trait Foo<'a, T> { type Value: 'a; }\nimpl<'a, 'b> Foo<'a, &'b ()> for () { type Value = &'b i32; }\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("E0477"), "{d:?}");
    // (a value with a where clause of its own (`where Self: 'b`) is not checked: the clause is not read)
    assert_eq!(check("trait Foo<'a> { type Value: 'a where Self: 'a; }\nimpl<'a, 'b> Foo<'b> for &'a i32 { type Value = &'a i32 where Self: 'b; }\nfn main() {}\n"), vec![]);
    // (a self type whose declaration bounds its lifetime (`struct Static<'a: 'static>`) implies the bound: not
    // checked; one that does not is)
    assert_eq!(check("struct Static<'a: 'static>(&'a ());\ntrait Foo { type Value: 'static; }\nimpl<'a> Foo for Static<'a> {\n    type Value = &'a ();\n}\nfn main() {}\n"), vec![]);
    let d = check("struct W<'a>(&'a ());\ntrait Foo { type Value: 'static; }\nimpl<'a> Foo for W<'a> {\n    type Value = &'a ();\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("E0477"), "{d:?}");
}

#[test]
fn part26_mut_ref_passed_as_shared() {
    // (`let x = &mut v; f(x);` with f taking `&T`: x stays a `&mut T` (inference does not make it the `&T` it is
    // coerced to), so writing through it after is accepted, as in rustc; and the explicit and implicit self-reborrows
    // `y = &mut *y;` / `y = y;` (nll/self-assign-ref-mut.rs)
    assert_eq!(check("fn g(_: &i32) {}\nfn main() {\n    let mut r: i32 = 0;\n    let x = &mut r;\n    g(x);\n    *x = 1;\n}\n"), vec![], "a2f");
    assert_eq!(check("fn g(_: &Vec<i32>) {}\nfn main() {\n    let mut r = Vec::new();\n    r.push(1);\n    let x = &mut r;\n    g(x);\n    x.push(2);\n}\n"), vec![], "a2h");
    assert_eq!(check("fn g(_: &i32) {}\nfn main() {\n    let mut r: i32 = 0;\n    let x = &mut r;\n    *x = 2;\n    g(x);\n    *x = 1;\n}\n"), vec![], "b2");
    assert_eq!(check("fn main() {\n    let mut r = 0;\n    let x = &mut r;\n    let v: &i32 = x;\n    println!(\"{}\", v);\n    *x = 1;\n    println!(\"{}\", x);\n}\n"), vec![], "a2");
    assert_eq!(check("struct S<'a> { f: &'a i32 }\nfn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let s = S { f: x };\n    println!(\"{}\", s.f);\n    *x = 1;\n}\n"), vec![], "f4");
    assert_eq!(check("struct S<'a> { f: &'a mut i32 }\nfn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let s = S { f: x };\n    *s.f = 2;\n    *x = 1;\n}\n"), vec![], "g1");
    assert_eq!(check("fn main() {\n    let mut a = 0;\n    let mut b = 0;\n    let y = &mut a;\n    let mut x = &mut b;\n    *x = 0;\n    x = y;\n    *x = 2;\n    *y = 1;\n}\n"), vec![], "h2");
    assert_eq!(check("fn main() {\n    let mut a = 0;\n    let mut b = 0;\n    let mut x = &mut a;\n    let v: &i32 = x;\n    x = &mut b;\n    *x = 1;\n    println!(\"{}\", v);\n}\n"), vec![], "c1");
    assert_eq!(check("fn main() {\n    let mut x = 1;\n    {\n        let mut y = &mut x;\n        y = &mut *y;\n        y = &mut *y;\n        *y = 2;\n    }\n    x;\n}\n"), vec![], "i1");
    assert_eq!(check("fn main() {\n    let mut v = vec![1, 2];\n    let x = &mut v;\n    for _ in 0..2 {\n        let s: &[i32] = x;\n        println!(\"{}\", s.len());\n        x.push(3);\n    }\n}\n"), vec![], "c7");
    assert_eq!(check("fn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let v: &i32 = x;\n    let w: &i32 = x;\n    println!(\"{} {}\", v, w);\n    *x += 1;\n}\n"), vec![], "c9");
}

#[test]
fn part26_let_reborrow_of_places() {
    // (`let v: &T = p;` with p a place of a `&mut T` (a local, a field, through a Box or a guard): v reborrows `*p`)
    let d = check("fn main() {\n    let mut r = 0;\n    let x = &mut r;\n    let v: &i32 = x;\n    *x = 1;\n    println!(\"{}\", v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut r = 0;\n    let t = (&mut r, 0);\n    let v: &i32 = t.0;\n    *t.0 = 1;\n    println!(\"{}\", v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot assign to `*t.0` because it is borrowed"), "{d:?}");
    let d = check("use std::cell::RefCell;\nfn main() {\n    let mut r = 0;\n    let s = 0;\n    let x = RefCell::new((&mut r, s));\n    let val: &i32 = x.borrow().0;\n    println!(\"{}\", val);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("E0716"), "{d:?}");
    let d = check("fn main() {\n    let mut r = 0;\n    let t = (&mut r, 0);\n    let b = Box::new(t);\n    let val: &i32 = b.0;\n    drop(b);\n    println!(\"{}\", val);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot move out of `b` because it is borrowed"), "{d:?}");
}

#[test]
fn part26_reborrow_at_coercion_sites() {
    // (a constructor's field (`S { f: x }`, `Some(x)`) and an assignment's right side reborrow a place of a `&mut`)
    let d = check("struct S<'a> { f: &'a i32 }\nfn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let s = S { f: x };\n    *x = 1;\n    println!(\"{}\", s.f);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let v: Option<&mut i32> = Some(x);\n    *x = 1;\n    if let Some(r) = v { *r = 3; }\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let v: &i32;\n    v = x;\n    *x = 1;\n    println!(\"{}\", v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut a = 0;\n    let mut b = 0;\n    let y = &mut a;\n    let mut x = &mut b;\n    x = y;\n    *y = 1;\n    *x = 2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot assign to `*y` because it is borrowed"), "{d:?}");
}

#[test]
fn part26_call_arg_reborrows() {
    // (a call's `&mut` / `&` argument from a place of a `&mut` borrows `*x` at the call, also without the region
    // check; a result the signature says borrows from it holds the loan)
    let d = check("fn pick<'a>(a: &'a i32, _b: &i32) -> &'a i32 { a }\nfn main() {\n    let mut a = 0;\n    let b = 0;\n    let x = &mut a;\n    let r = pick(x, &b);\n    *x = 1;\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
    let d = check("fn take(_: &mut i32) {}\nfn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let v = &*x;\n    take(x);\n    println!(\"{}\", v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot borrow `*x` as mutable because it is also borrowed as immutable"), "{d:?}");
    let d = check("fn take(_: &mut i32) {}\nfn main() {\n    let mut a = 0;\n    let x = &mut a;\n    let v: &i32 = x;\n    take(x);\n    println!(\"{}\", v);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot borrow `*x` as mutable because it is also borrowed as immutable"), "{d:?}");
}

#[test]
fn part26_mut_ref_in_aggregate_moves() {
    // (a tuple holding a `&mut` is not Copy: `let t2 = t;` moves t)
    let d = check("fn main() {\n    let mut a = 0;\n    let t = (&mut a, 0);\n    let v = &*t.0;\n    let t2 = t;\n    println!(\"{} {}\", v, t2.1);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot move out of `t` because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut a = 0;\n    let t = (&mut a, 0);\n    let u = t;\n    *t.0 = 1;\n    println!(\"{}\", u.1);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("assign to part of moved value"), "{d:?}");
}

#[test]
fn part26_annotation_hole_mutability() {
    // (`let y: &mut [_] = &x[..];`: the annotation's holes are filled keeping the mutability it writes)
    let d = check("fn main() {\n    let x: &[i32] = &[1, 2];\n    let y: &mut [_] = &x[..];\n    let _ = y;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 3 && d[0].1.contains("mismatched types"), "{d:?}");
}

#[test]
fn part26_review() {
    // (a self-reborrow assigned to its local drops only the loans through a reference: a loan of an owned field
    // (`s = S { r: &s.n, .. }`) stays, so writing `s.n` while it lives is E0506)
    let d = check("struct S<'a> { r: &'a i32, n: i32 }\nfn main() {\n    let zero = 0;\n    let mut s = S { r: &zero, n: 0 };\n    s = S { r: &s.n, n: 1 };\n    s.n = 5;\n    println!(\"{}\", s.r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot assign to `s.n` because it is borrowed"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut x = 1;\n    let mut y = &mut x;\n    y = &mut *y;\n    *y = 2;\n    let mut v = vec![1];\n    let mut r = &mut v;\n    r = r;\n    r.push(2);\n}\n"), vec![]);
    // (a deref coercion through the reference a `&mut` points to (`&mut &i32` to `&i32`): the loan is of `**x`, which
    // overwriting `*x` does not touch; `&mut &i32` to `&&i32` borrows `*x`)
    assert_eq!(check("fn main() {\n    let a = 1;\n    let b = 2;\n    let mut slot = &a;\n    let x = &mut slot;\n    let y: &i32 = x;\n    *x = &b;\n    println!(\"{}\", y);\n}\n"), vec![]);
    let d = check("fn main() {\n    let a = 1;\n    let mut slot = &a;\n    let x = &mut slot;\n    let y: &&i32 = x;\n    *x = &a;\n    println!(\"{}\", y);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
    // (a fn pointer's result that holds references holds the reborrow of the argument's place, without the region
    // check too; one that holds none does not)
    let d = check("fn id(x: &i32) -> &i32 { x }\nfn main() {\n    let mut a = 0;\n    let f: fn(&i32) -> &i32 = id;\n    let x = &mut a;\n    let r = f(x);\n    *x = 1;\n    println!(\"{}\", r);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 7 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
    assert_eq!(check("fn id(x: &i32) -> &i32 { x }\nfn len(x: &i32) -> usize { *x as usize }\nfn main() {\n    let mut a = 0;\n    let f: fn(&i32) -> &i32 = id;\n    let g: fn(&i32) -> usize = len;\n    let x = &mut a;\n    let r = f(x);\n    println!(\"{}\", r);\n    let n = g(x);\n    *x = 1;\n    println!(\"{}\", n);\n}\n"), vec![]);
}

#[test]
fn part26_review_2() {
    // (a value that holds what its local holds, assigned to the local (`r = &mut *r;`, `r = r;`): it keeps the old
    // value's loans of what r points to, so `x = 1` while r lives is E0506; and once r is assigned another place, it
    // holds those no longer)
    let d = check("fn main() {\n    let mut x = 0;\n    let mut r = &mut x;\n    r = &mut *r;\n    x = 1;\n    *r = 2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot assign to `x` because it is borrowed"), "{d:?}");
    let d = check("fn main() {\n    let mut x = 0;\n    let mut r = &mut x;\n    r = r;\n    x = 1;\n    *r = 2;\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot assign to `x` because it is borrowed"), "{d:?}");
    assert_eq!(check("fn main() {\n    let mut x = 0;\n    let mut r = &mut x;\n    r = &mut *r;\n    *r = 2;\n    x = 1;\n}\n"), vec![]);
    assert_eq!(check("fn main() {\n    let mut x = 0;\n    let mut y = 0;\n    let mut r = &mut x;\n    r = &mut *r;\n    *r = 2;\n    r = &mut y;\n    x = 1;\n    *r = 3;\n    println!(\"{}\", x);\n}\n"), vec![]);
    // (toward a trait object: `&mut &i32` as a `&dyn Tr` unsizes the `&i32` itself, so the loan is of `*x`, and
    // `*x = &b` while y lives is E0506)
    let d = check("trait Tr { fn get(&self) -> i32; }\nimpl Tr for &i32 {\n    fn get(&self) -> i32 { **self }\n}\nfn main() {\n    let a = 1;\n    let b = 2;\n    let mut slot = &a;\n    let x: &mut &i32 = &mut slot;\n    let y: &dyn Tr = x;\n    *x = &b;\n    println!(\"{}\", y.get());\n}\n");
    assert!(d.len() == 1 && d[0].0 == 11 && d[0].1.contains("cannot assign to `*x` because it is borrowed"), "{d:?}");
}

#[test]
fn part27_two_phase_arg_reborrows() {
    // (a call's implicit `&mut` argument reborrow is a two-phase borrow, also without the region check: reserved (a
    // shared loan) until the arguments are evaluated, then taken. `f(x, *x)` and `f(x, x.len())` read `*x` while it
    // is reserved; `f(x, &*x)` conflicts when it is taken, `f(x, g(x))` while it is reserved; a `&` argument's reborrow
    // lives until the call (`f(x, x)` with `f(&i32, &mut i32)`)
    assert_eq!(check("fn f(_a: &mut i32, _b: i32) {}\nfn main() {\n    let mut v = 0;\n    let x = &mut v;\n    f(x, *x);\n}\n"), vec![], "m3");
    assert_eq!(check("fn f(_a: &mut Vec<i32>, _b: usize) {}\nfn main() {\n    let mut v = vec![1];\n    let x = &mut v;\n    f(x, x.len());\n}\n"), vec![], "m4");
    assert_eq!(check("fn main() {\n    let mut a = 1;\n    let mut b = 2;\n    let x = &mut a;\n    let y = &mut b;\n    std::mem::swap(x, y);\n    std::mem::swap(x, y);\n    println!(\"{} {}\", x, y);\n}\n"), vec![], "n2");
    assert_eq!(check("fn first<'a>(v: &'a mut Vec<i32>, _k: usize) -> &'a mut i32 { &mut v[0] }\nfn main() {\n    let mut v = vec![1, 2];\n    let x = &mut v;\n    let r = first(x, x.len());\n    *r = 5;\n    x.push(3);\n}\n"), vec![], "n4");
    assert_eq!(check("use std::collections::HashMap;\nfn ins(m: &mut HashMap<i32, i32>, k: i32, v: i32) { m.insert(k, v); }\nfn main() {\n    let mut m = HashMap::new();\n    let r = &mut m;\n    ins(r, 1, r.len() as i32);\n    let n = r.len();\n    ins(r, 2, n as i32);\n    let s = String::from(\"a\");\n    let t = &mut s.clone();\n    t.push_str(&s);\n    println!(\"{:?}\", m);\n}\n"), vec![], "n5");
    let d = check("fn f(_a: &mut i32, _b: &i32) {}\nfn main() {\n    let mut v = 0;\n    let x = &mut v;\n    f(x, &*x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot borrow `*x` as mutable because it is also borrowed as immutable"), "{d:?}");
    let d = check("fn f(_a: &i32, _b: &mut i32) {}\nfn main() {\n    let mut v = 0;\n    let x = &mut v;\n    f(x, x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("cannot borrow `*x` as mutable because it is also borrowed as immutable"), "{d:?}");
    let d = check("fn g(x: &mut i32) -> i32 { *x }\nfn f(_a: &mut i32, _b: i32) {}\nfn main() {\n    let mut v = 0;\n    let x = &mut v;\n    f(x, g(x));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot borrow `*x` as mutable more than once at a time"), "{d:?}");
}

#[test]
fn part27_ptr_swap() {
    // (`ptr::swap` takes two raw pointers, which may be the same: `ptr::swap(t, t)` is no two unique borrows (as
    // `mem::swap(x, x)` is, E0499), raw pointers can be passed, and a call outside `unsafe` is E0133)
    assert_eq!(check("use std::ptr;\nfn main() {\n    let mut a = 1;\n    let mut b = 2;\n    unsafe { ptr::swap(&mut a, &mut b); }\n    let pa: *mut i32 = &mut a;\n    let pb: *mut i32 = &mut b;\n    unsafe { std::ptr::swap(pa, pb); ptr::swap(pa, pa); }\n    let mut x = 5;\n    let mut y = 6;\n    std::mem::swap(&mut x, &mut y);\n    println!(\"{} {} {} {}\", a, b, x, y);\n}\n"), vec![], "o1");
    assert_eq!(check("use std::ptr;\nfn d(t: &mut i32) {\n    unsafe { ptr::swap(t, t); }\n}\nfn main() {\n    let mut a = 1;\n    d(&mut a);\n    println!(\"{}\", a);\n}\n"), vec![], "q3");
    let d = check("use std::ptr;\nfn main() {\n    let mut a = 1;\n    let mut b = 2;\n    ptr::swap(&mut a, &mut b);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("call to unsafe function `ptr::swap` is unsafe and requires unsafe block (E0133)"), "{d:?}");
    let d = check("fn main() {\n    let mut a = 1;\n    let x = &mut a;\n    std::mem::swap(x, x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 4 && d[0].1.contains("cannot borrow `*x` as mutable more than once at a time"), "{d:?}");
}

#[test]
fn part27_review() {
    // (#151's review: a program's own `ptr` type keeps its `swap` and `eq`; a call's result passed for a reference
    // holds its loans until the call, as a `&x` argument does: `f(x, id(&*x))` is E0502 when the first argument's
    // reborrow is taken. A result that is no reference (`f2(x, n(&*x))`), or that borrows something else, does not)
    assert_eq!(check("#[allow(non_camel_case_types)]\nstruct ptr;\nimpl ptr {\n    fn swap() -> i32 { 42 }\n    fn eq() -> i32 { 1 }\n}\nfn main() {\n    println!(\"{} {}\", ptr::swap(), ptr::eq());\n}\n"), vec![], "r1");
    assert_eq!(check("fn f(_: &mut i32, _: &i32) {}\nfn f2(_: &mut i32, _: i32) {}\nfn id<'a>(x: &'a i32, _y: &i32) -> &'a i32 { x }\nfn n(x: &i32) -> i32 { *x }\nstruct W(i32);\nimpl W { fn get(&self) -> &i32 { &self.0 } }\nfn main() {\n    let mut v = 1;\n    let w = 3;\n    let ww = W(4);\n    let x = &mut v;\n    f2(x, n(&*x));\n    f(x, id(&w, &*x));\n    f(x, ww.get());\n    println!(\"{}\", x);\n}\n"), vec![], "r4");
    let d = check("fn f(_: &mut i32, _: &i32) {}\nfn id(x: &i32) -> &i32 { x }\nfn main() {\n    let mut v = 1;\n    let x = &mut v;\n    f(x, id(&*x));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6 && d[0].1.contains("cannot borrow `*x` as mutable because it is also borrowed as immutable"), "{d:?}");
}

#[test]
fn part27_assoc_type_into_dyn() {
    // (a generic fn whose signature names a lifetime or `'static`, calling a bound's method that returns its `Self`'s
    // associated type: checked with placeholders, so `Box::new(v.into_item())` as a `Box<dyn X + 'static>` is E0310;
    // with `where T::Item: 'a` it is accepted)
    let d = check("trait X {}\ntrait Iter {\n    type Item: X;\n    fn into_item(self) -> Self::Item;\n}\nfn bad1<T: Iter>(v: T) -> Box<dyn X + 'static> {\n    let item = v.into_item();\n    Box::new(item)\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].0 == 8 && d[0].1.contains("the associated type `<T as Iter>::Item` may not live long enough (it must outlive `'static`) (E0310)"), "{d:?}");
    assert_eq!(check("trait X {}\ntrait Iter {\n    type Item: X;\n    fn into_item(self) -> Self::Item;\n}\nfn ok1<'a, T: Iter>(v: T) -> Box<dyn X + 'a>\n    where T::Item: 'a\n{\n    let item = v.into_item();\n    Box::new(item)\n}\nfn ok2<T: Iter>(v: T) -> Box<dyn X + 'static>\n    where T::Item: 'static\n{\n    Box::new(v.into_item())\n}\nfn main() {}\n"), vec![], "t3");
}

#[test]
fn part28_generic_dyn_args() {
    // (a trait object's type arguments that name a type parameter: in a generic type's field (`Box<dyn Fn(U) -> U>`,
    // `&dyn A<T>`), a generic fn's parameter and result. They are given as the type's or fn's arguments are; the
    // parameters' names were looked up where the type was used ("cannot find type `U`"), and nothing inferred them)
    assert_eq!(check("struct S<U> { f: Box<dyn Fn(U) -> U> }\nfn mk() -> S<i32> { S { f: Box::new(|x| x + 1) } }\nfn app<T>(s: &S<T>, x: T) -> T { (s.f)(x) }\nfn app2(s: &S<i32>, x: i32) -> i32 { (s.f)(x) * 2 }\nfn main() {\n    let s = mk();\n    println!(\"{} {}\", app(&s, 3), app2(&s, 3));\n}\n"), vec![], "u1");
    assert_eq!(check("trait A<T> { fn get(&self) -> T; }\nstruct B<'a, T: 'a>(&'a (dyn A<T> + 'a));\nimpl A<i32> for i32 { fn get(&self) -> i32 { *self } }\nfn f(v: &dyn A<i32>) -> i32 {\n    let b = B(v);\n    b.0.get()\n}\nfn g<'a>(v: &'a dyn A<i32>) -> B<'a, i32> { B(v) }\nfn main() { println!(\"{} {}\", f(&3), g(&4).0.get()); }\n"), vec![], "u2");
    assert_eq!(check("fn bar<'a, T: Clone + 'a>(t: T) -> Box<dyn FnMut() -> T + 'a> { Box::new(move || t.clone()) }\nfn main() {\n    let mut f = bar(42_u32);\n    assert_eq!(f(), 42);\n    let mut g = bar(\"forty-two\");\n    assert_eq!(g(), \"forty-two\");\n}\n"), vec![], "u3");
    assert_eq!(check("trait Clam<A> { fn chowder(&self, y: A); }\nstruct Foo<A> { x: A }\nimpl<A> Clam<A> for Foo<A> { fn chowder(&self, _y: A) {} }\nfn f<A>(x: Box<dyn Clam<A>>, a: A) { x.chowder(a); }\nfn main() {\n    let c = Foo { x: 42isize };\n    let d: Box<dyn Clam<isize>> = Box::new(c) as Box<dyn Clam<isize>>;\n    f(d, 3);\n}\n"), vec![], "u4");
    // (a closure where the parameter is not known yet: coerced to the trait object as written, as before)
    assert_eq!(check("fn apply<T>(f: Box<dyn Fn(T) -> T>, x: T) -> T { f(x) }\nfn twice<T: Copy>(f: &dyn Fn(T) -> T, x: T) -> T { f(f(x)) }\nfn each<T>(v: Vec<T>, f: &mut dyn FnMut(T)) { for x in v { f(x); } }\nfn main() {\n    let typed: Box<dyn Fn(i64) -> i64> = Box::new(|x| x * 3);\n    let mut s = 0;\n    each(vec![1, 2, 3], &mut |x| s += x);\n    println!(\"{} {} {} {}\", apply(Box::new(|x| x + 1), 3), apply(typed, 5), twice(&|x| x * 2, 3), s);\n}\n"), vec![], "u5");
    // (the region check of a generic type's trait object that refers to a local, which a type annotation error
    // hid: regions/regions-close-object-into-object-1.rs)
    let d = check("trait A<T> {}\nstruct B<'a, T: 'a>(&'a (dyn A<T> + 'a));\ntrait X {}\nimpl<'a, T> X for B<'a, T> {}\nfn f<'a, T: 'static, U>(v: Box<dyn A<T> + 'static>) -> Box<dyn X + 'static> {\n    Box::new(B(&*v)) as Box<dyn X>\n}\nfn g(v: Box<dyn A<i32> + 'static>) -> Box<dyn X + 'static> {\n    Box::new(B(&*v))\n}\nfn main() {}\n");
    assert!(d.len() == 2 && d[0].0 == 6 && d[1].0 == 9 && d.iter().all(|x| x.1.contains("cannot return a reference to a local variable: `*v` is dropped when the function returns")), "{d:?}");
}

#[test]
fn part28_review() {
    // (#154's review: a trait's name behind a pointer in a 2015 edition's signature and struct field is its trait
    // object (the edition is known when they are resolved); a closure for `Box<dyn Fn(T) -> U>` coerced where T is known
    // and U not yet; a program's own type named like a mark (`__S0`) is that type)
    let mut s = Session::new(Options { jobs: 1, no_cache: true, edition: 2015, ..Options::default() });
    let d: Vec<(usize, String)> = s.check("trait X { fn v(&self) -> i32; }\nimpl X for i32 { fn v(&self) -> i32 { *self } }\nstruct H<'a> { x: &'a X }\nfn take(x: &X) -> i32 { x.v() }\nfn main() { let h = H { x: &5 }; println!(\"{} {}\", take(&3), h.x.v()); }\n").into_iter().map(|d| (d.line, d.msg)).collect();
    assert_eq!(d, vec![], "v1");
    // (the `bare_trait_objects` lint denied: such an object is an error, at the line that names it)
    let mut s = Session::new(Options { jobs: 1, no_cache: true, edition: 2018, ..Options::default() });
    let d: Vec<(usize, String)> = s.check("#[deny(bare_trait_objects)]\nfn function(x: &SomeTrait) {\n    let _ = x;\n}\ntrait SomeTrait {}\nfn main() {}\n").into_iter().map(|d| (d.line, d.msg)).collect();
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("trait objects without an explicit `dyn` are deprecated"), "{d:?}");
    assert_eq!(check("fn f<T, U>(a: T, cb: Box<dyn Fn(T) -> U>, b: U) -> U { let _ = b; cb(a) }\nstruct S2<T, U> { a: T, cb: Box<dyn Fn(T) -> U>, b: U }\nfn main() {\n    println!(\"{}\", f(1i32, Box::new(|x: i32| x), 2i32));\n    let s = S2 { a: 1i32, cb: Box::new(|x: i32| x + 10), b: 2i32 };\n    println!(\"{} {}\", (s.cb)(s.a), s.b);\n}\n"), vec![], "v2");
    assert_eq!(check("struct A;\n#[allow(non_camel_case_types)]\nstruct __S0(i32);\nfn main() { let x: __S0 = __S0(4); let _a: A = A; println!(\"{}\", x.0); }\n"), vec![], "v3");
    // (a generic parameter named like a mark is that parameter; a name like a struct's mark that names nothing is
    // no type; a crate's `allow` after its `deny` of bare_trait_objects allows them)
    assert_eq!(check("#[allow(non_camel_case_types)]\nfn f<T: Copy, __zp0_U>(_a: T, x: __zp0_U) -> __zp0_U { x }\nfn main() { let y: u8 = f(1i32, 7u8); println!(\"{}\", y); }\n"), vec![], "v4");
    assert_eq!(check("#[allow(non_camel_case_types)]\ntrait A<X> { fn get(&self) -> X; }\nimpl A<u8> for u8 { fn get(&self) -> u8 { *self } }\n#[allow(non_camel_case_types)]\nfn f<T, __zp0_U>(_t: T, x: &dyn A<__zp0_U>) -> __zp0_U { x.get() }\nfn main() { let y: u8 = f(1i32, &7u8 as &dyn A<u8>); println!(\"{}\", y); }\n"), vec![], "v4b");
    let d = check("fn main() { let _x: __zs0 = todo!(); }\n");
    assert!(d.len() == 1 && d[0].1.contains("cannot find type `__zs0`"), "{d:?}");
    let mut s = Session::new(Options { jobs: 1, no_cache: true, edition: 2018, ..Options::default() });
    let d: Vec<(usize, String)> = s.check("#![deny(bare_trait_objects)]\n#![allow(bare_trait_objects)]\ntrait X {}\nimpl X for i32 {}\nfn take(_x: &X) {}\nfn main() { take(&1); }\n").into_iter().map(|d| (d.line, d.msg)).collect();
    assert_eq!(d, vec![], "v5");
    // (an item's deny of the lint with others before it; a trait object's argument `&mut i32` is no `&T`)
    let mut s = Session::new(Options { jobs: 1, no_cache: true, edition: 2018, ..Options::default() });
    let d: Vec<(usize, String)> = s.check("#[deny(unused, bare_trait_objects)]\nfn function(x: &SomeTrait) {\n    let _ = x;\n}\ntrait SomeTrait {}\nfn main() {}\n").into_iter().map(|d| (d.line, d.msg)).collect();
    assert!(d.len() == 1 && d[0].0 == 2 && d[0].1.contains("trait objects without an explicit `dyn` are deprecated"), "{d:?}");
    // (such an attribute inside a string or a comment is none)
    let mut s = Session::new(Options { jobs: 1, no_cache: true, edition: 2018, ..Options::default() });
    let d: Vec<(usize, String)> = s.check("const NOTE: &str = r\"#[deny(unused, bare_trait_objects)]\";\n// #[deny(bare_trait_objects)]\ntrait X {}\nimpl X for i32 {}\nfn take(_x: &X) {}\nfn main() { take(&1); println!(\"{}\", NOTE.len()); }\n").into_iter().map(|d| (d.line, d.msg)).collect();
    assert_eq!(d, vec![], "v6");
    let d = check("trait A<T> { fn get(&self) -> i32; }\nimpl<'a> A<&'a mut i32> for u8 { fn get(&self) -> i32 { 1 } }\nfn f<T>(x: &dyn A<&T>) -> i32 { x.get() }\nfn main() {\n    let b: &dyn A<&mut i32> = &1u8;\n    println!(\"{}\", f(b));\n}\n");
    assert!(d.len() == 1 && d[0].0 == 6, "{d:?}");
}

/// Each error of the program: its line, and a part of its message (one error for each fn)
fn errors_at(src: &str, exp: &[(usize, &str)], what: &str) {
    let d = check(src);
    assert!(d.len() == exp.len() && d.iter().zip(exp).all(|(d, e)| d.0 == e.0 && d.1.contains(e.1)), "{what}\n{d:?}");
}

#[test]
fn part29_drop_temporaries() {
    // (a temporary whose type implements `Drop` lives to the end of its statement, also where it is a field access's
    // base or a method's receiver (`D(&*v).0.len()`, `D(&*v).iter()`), and a loop's header's to the end of the loop
    // (`for x in D(&*v).iter()`, `while let Some(_) = D(&*v).0.first()`). The condition of an `if` / `while` and a
    // match guard end their temporaries: no false error for `if D(&*v).0.len() > 0 { v.push(1) }`. A temporary of the
    // tail of a block outlives the block's locals (E0597). Each case was checked with rustc 1.97: zustc reports each
    // fn's first error on rustc's line)
    errors_at(r#"struct D<'a>(&'a [i32]);
impl Drop for D<'_> { fn drop(&mut self) {} }
impl D<'_> { fn iter(&self) -> std::slice::Iter<'_, i32> { self.0.iter() } }
struct W<'a>(&'a [i32]);
impl W<'_> { fn iter(&self) -> std::slice::Iter<'_, i32> { self.0.iter() } }
fn a() { let mut v = vec![1, 2]; for x in D(&*v).iter() { v.push(*x); break; } }
fn b() { let mut v = vec![1, 2]; for x in W(&*v).iter() { v.push(*x); break; } }
fn c() { let mut v = vec![1, 2]; let n = D(&*v).0.len(); v.push(n as i32); }
fn d() { let mut v = vec![1, 2]; match D(&*v).0.len() { n => v.push(n as i32) } }
fn main() { a(); b(); c(); d(); }
"#, &[(6, r#"cannot borrow `v` as mutable because it is also borrowed as immutable"#), (9, r#"`v` does not live long enough"#)], "dr1");
    errors_at(r#"struct D<'a>(&'a [i32]);
impl Drop for D<'_> { fn drop(&mut self) {} }
fn e() { let mut v = vec![1, 2]; let _g = D(&*v); v.push(1); }
fn f() { let mut v = vec![1, 2]; { let _n = D(&*v).0.len(); v.push(1); } }
fn g() { let mut v = vec![1, 2]; if D(&*v).0.len() > 0 { v.push(1); } }
fn h() { let mut v = vec![1, 2]; while D(&*v).0.len() < 5 { v.push(1); } }
fn main() { e(); f(); g(); h(); }
"#, &[(3, r#"cannot borrow `v` as mutable because it is also borrowed as immutable"#)], "dr2");
    errors_at(r#"struct D<'a>(&'a [i32]);
impl Drop for D<'_> { fn drop(&mut self) {} }
impl D<'_> { fn iter(&self) -> std::slice::Iter<'_, i32> { self.0.iter() } fn len(&self) -> usize { self.0.len() } }
fn a1() { let mut v = vec![1, 2]; for x in D(&*v).0.iter() { v.push(*x); break; } }
fn a2() { let mut v = vec![1, 2]; for _ in 0..D(&*v).len() { v.push(1); break; } }
fn a3() { let mut v = vec![1, 2]; for x in D(&*v).iter() { v.push(*x); break; } }
fn a4() { let mut v = vec![1, 2]; while let Some(_) = D(&*v).0.first() { v.push(1); break; } }
fn main() { a1(); a2(); a3(); a4(); }
"#, &[(4, r#"cannot borrow `v` as mutable because it is also borrowed as immutable"#), (5, r#"cannot borrow `v` as mutable because it is also borrowed as immutable"#), (6, r#"cannot borrow `v` as mutable because it is also borrowed as immutable"#), (7, r#"cannot borrow `v` as mutable because it is also borrowed as immutable"#)], "dr3");
    errors_at(r#"struct D<'a>(&'a [i32]);
impl Drop for D<'_> { fn drop(&mut self) {} }
fn b1() { let mut v = vec![1, 2]; while let Some(_) = D(&*v).0.first() { v.push(1); } }
fn b2() { let mut v = vec![1, 2]; let n = D(&*v).0.len(); v.push(n as i32); }
fn b3() { let mut v = vec![1, 2]; match D(&*v).0.len() { n => { v.push(n as i32); } } }
fn b4() { let mut v = vec![1, 2]; if let Some(_) = D(&*v).0.first() { v.push(1); } }
fn main() { b1(); b2(); b3(); b4(); }
"#, &[(3, r#"cannot borrow `v` as mutable because it is also borrowed as immutable"#), (5, r#"`v` does not live long enough"#), (6, r#"`v` does not live long enough"#)], "dr4");
    // (the scopes that end such temporaries, where rustc ends them: a statement, a block's tail, the condition of an
    // `if` / `while`, a guard, a loop that is a statement or a tail; none of these is an error in rustc 1.97)
    errors_at(r#"struct D<'a>(&'a [i32]);
impl Drop for D<'_> { fn drop(&mut self) {} }
impl D<'_> { fn iter(&self) -> std::slice::Iter<'_, i32> { self.0.iter() } }
fn t1() { let mut v = vec![1]; for x in D(&*v).iter() { let _ = x; } v.push(1); }
fn t2() -> usize { let mut v = vec![1]; let n = { for x in D(&*v).iter() { let _ = x; } 1 }; v.push(1); n }
fn t3() { let v = vec![1]; while let Some(_) = D(&*v).0.first() { break; } }
fn t4() { let v = vec![1]; while D(&*v).0.len() > 5 { } }
fn t5() { let v = vec![1]; if D(&*v).0.len() > 5 { } }
fn t6() { let v = vec![1]; loop { if D(&*v).0.len() > 0 { break; } } }
fn t7() { let v = vec![1]; for x in D(&*v).iter() { let _ = x; } }
fn t8() { let v = vec![1]; match 1 { n if D(&*v).0.len() > n => {} _ => {} } }
fn t9() -> usize { let v = vec![1]; if D(&*v).0.len() > 5 { 1 } else { 2 } }
fn main() { t1(); t2(); t3(); t4(); t5(); t6(); t7(); t8(); t9(); }
"#, &[], "ts");
    errors_at(r#"struct D<'a>(&'a [i32]);
impl Drop for D<'_> { fn drop(&mut self) {} }
fn c1() { let v = vec![1, 2]; for x in D(&*v).0.iter() { let _ = x; } }
fn c2() { let v = vec![1, 2]; match D(&*v).0.len() { _ => {} } }
fn c3() { let v = vec![1, 2]; while let Some(_) = D(&*v).0.first() { break; } }
fn c4() { let v = vec![1, 2]; if let Some(_) = D(&*v).0.first() { } }
fn c5() -> usize { let v = vec![1, 2]; D(&*v).0.len() }
fn main() { c1(); c2(); c3(); c4(); c5(); }
"#, &[(4, r#"`v` does not live long enough"#), (6, r#"`v` does not live long enough"#), (7, r#"`v` does not live long enough"#)], "dr5");
}

#[test]
fn part29_returned_temporaries() {
    // (a temporary's loan that the returned value holds is E0515 where it is returned, as in rustc, not E0716 where it
    // goes away: `for ref x in xs { result.push(x) } result`, also where a loop over a `&mut Iter` makes the temporary
    // in its header, after a use (`println!`) or an assignment of the returned local. One that no return holds stays
    // E0716 (`*result[0]`, line 17))
    errors_at(r#"fn f(xs: Vec<i32>) -> Vec<&'static i32> {
    let mut result = Vec::new();
    for ref x in xs {
        result.push(x);
    }
    result
}
fn g<'a>(xs: &mut std::slice::Iter<'a, i32>) -> Vec<&'a &'a i32> {
    let mut result = Vec::new();
    for ref x in xs {
        result.push(x);
    }
    result
}
fn h(xs: Vec<i32>) -> i32 {
    let mut result = Vec::new();
    for ref x in xs {
        result.push(x);
    }
    *result[0]
}
fn main() {}
"#, &[(6, r#"cannot return value referencing temporary value (E0515)"#), (13, r#"cannot return value referencing temporary value (E0515)"#), (17, r#"temporary value dropped while borrowed"#)], "p28/fr1");
    errors_at(r#"fn f(xs: Vec<i32>) -> Vec<&'static i32> {
    let mut result = Vec::new();
    for ref x in xs {
        result.push(x);
    }
    println!("{:?}", result);
    result
}
fn g(xs: Vec<i32>) -> Vec<&'static i32> {
    let mut result = Vec::new();
    for ref x in xs {
        result.push(x);
    }
    let r2 = result;
    r2
}
fn main() {}
"#, &[(7, r#"cannot return value referencing temporary value (E0515)"#), (15, r#"cannot return value referencing temporary value (E0515)"#)], "p28/fr2");
    errors_at(r#"fn h<'a>(xs: &mut std::slice::Iter<'a, i32>) -> Vec<&'a &'a i32> {
    let mut result = Vec::new();
    for ref x in xs {
        result.push(x);
    }
    println!("{}", result.len());
    result = Vec::new();
    result
}
fn h2(xs: Vec<i32>) -> Vec<&'static i32> {
    let mut result = Vec::new();
    for ref x in xs {
        result.push(x);
    }
    println!("{}", result.len());
    result = Vec::new();
    result
}
fn main() {}
"#, &[(8, r#"cannot return value referencing temporary value (E0515)"#), (17, r#"cannot return value referencing temporary value (E0515)"#)], "p29t/g3");
}

#[test]
fn part29_positions() {
    // (an error's earlier borrow at a node the lowering made (`&*v` of a Vec, read as `&v[..]`) is at its operand,
    // not at the start of the fn ("(at 1:1)"), where rustc 1.97 points. A `&` has no position of its own, so `D(&v)`
    // and `&*b` of a Box are at their operand (4:43, 1:45), one column after rustc's `&`, as on main)
    errors_at(r#"struct D<'a>(&'a [i32]);
fn k() { let mut v = vec![1]; let r = &*v; v.push(1); let _ = r; }
fn k2() { let mut v = vec![1]; let r = D(&*v); v.push(1); let _ = r; }
fn k3() { let mut v = vec![1]; let r = D(&v); v.push(1); let _ = r; }
fn main() {}
"#, &[(2, r#"cannot borrow `v` as mutable because it is also borrowed as immutable (at 2:41)"#), (3, r#"cannot borrow `v` as mutable because it is also borrowed as immutable (at 3:44)"#), (4, r#"cannot borrow `v` as mutable because it is also borrowed as immutable (at 4:43)"#)], "pos1");
    errors_at(r#"fn k1() { let mut b = Box::new(5); let r = &*b; *b = 1; let _ = r; }
fn k2() { let mut v = vec![1]; let r = &v[0]; v.push(1); let _ = r; }
fn k3() { let mut v = vec![1]; let r = &*v; v.push(1); let _ = r; }
fn k4() { let mut s = String::new(); let r = &*s; s.push('a'); let _ = r; }
fn main() {}
"#, &[(1, r#"cannot assign to `*b` because it is borrowed (at 1:45)"#), (2, r#"cannot borrow `v` as mutable because it is also borrowed as immutable (at 2:41)"#), (3, r#"cannot borrow `v` as mutable because it is also borrowed as immutable (at 3:42)"#), (4, r#"cannot borrow `s` as mutable because it is also borrowed as immutable (at 4:48)"#)], "pos2");
}

#[test]
fn part29_review() {
    // (a temporary receiver that a method takes by value (`consume(self)`) holds nothing of what it borrowed after
    // the call: no E0506 for the assignment after it. One borrowed (`peek(&self)`) holds it to the end of the
    // statement, E0506 as in rustc 1.97)
    errors_at("struct D<'a>(&'a i32);\nimpl Drop for D<'_> { fn drop(&mut self) {} }\nimpl D<'_> { fn consume(self) -> i32 { *self.0 } }\nfn main() {\n    let mut x = 1;\n    let t = (D(&x).consume(), { x = 2; });\n    let _ = t;\n    println!(\"{}\", x);\n}\n", &[], "r3");
    errors_at("struct D<'a>(&'a i32);\nimpl Drop for D<'_> { fn drop(&mut self) {} }\nimpl D<'_> { fn peek(&self) -> i32 { *self.0 } }\nfn main() {\n    let mut x = 1;\n    let t = (D(&x).peek(), { x = 2; });\n    let _ = t;\n}\n", &[(6, "cannot assign to `x` because it is borrowed")], "r4");
    // (the call takes such a receiver after its arguments: an argument that writes what it borrows is E0506, as in
    // rustc 1.97; a write after the call is not)
    errors_at("struct D<'a>(&'a i32);\nimpl Drop for D<'_> { fn drop(&mut self) {} }\nimpl D<'_> {\n    fn consume(self, _: ()) -> i32 { *self.0 }\n}\nfn a() { let mut x = 1; let _ = D(&x).consume({ x = 2; }); }\nfn c() { let mut x = 1; let r = D(&x).consume(()); x = 2; println!(\"{} {}\", r, x); }\nfn main() { a(); c(); }\n", &[(6, "cannot assign to `x` because it is borrowed")], "r6");
    // (a returned local assigned after the use of a temporary it borrowed: rustc 1.97 reports E0515 at the return,
    // as zustc does)
    errors_at("fn f() -> &'static str {\n    let mut r = String::new().as_str();\n    println!(\"{}\", r);\n    r = \"ok\";\n    r\n}\nfn main() { println!(\"{}\", f()); }\n", &[(5, "cannot return value referencing temporary value (E0515)")], "r2");
}

#[test]
fn part30_pattern_moves() {
    // (a by-value binding's move out of what the scrutinee owns is at its arm, where rustc 1.97 reports a use after
    // it (8, 20); a move through a reference, which cannot be made, stays at the scrutinee (E0507, 12))
    errors_at(r#"fn array() -> [(String, String); 3] { Default::default() }
fn f() {
    let a = array();
    match a {
        [_, _, _x] => {}
    }
    match a {
        [.., _y] => {}
    }
}
fn g(x: &Option<String>) {
    match *x {
        Some(_s) => {}
        None => {}
    }
}
fn h() {
    let t = (String::new(), 1);
    let s = match t { (s, _) => s };
    let _u = t.0;
    let _ = s;
}
fn main() { f(); g(&None); h(); }
"#, &[(8, r#"use of moved value"#), (12, r#"cannot move out of"#), (20, r#"use of moved value"#)], "pm");
}

#[test]
fn part30_outlives_bounds() {
    // (an outlives bound on a type implies its parts': `Self: 'b` in an impl's method, `D<'a>: 'b`, `E<'a, T>: 'b`
    // and `&'a T: 'b` give 'a: 'b, so returning a `&'a` for a `&'b` is no error; without the bound it is (8). A caller
    // must meet it (4, 6). Lines as rustc 1.97's)
    errors_at(r#"struct D<'a>(&'a i32);
impl<'a> D<'a> {
    fn keep<'b>(self) -> &'b i32 where Self: 'b { self.0 }
}
struct E<'a, T>(&'a T);
fn f<'a, 'b>(x: D<'a>) -> &'b i32 where D<'a>: 'b { x.0 }
fn g<'a, 'b, T>(x: E<'a, T>) -> &'b T where E<'a, T>: 'b { x.0 }
fn h<'a, 'b>(x: D<'a>) -> &'b i32 { x.0 }
fn main() { let v = 1; let d = D(&v); println!("{}", d.keep()); println!("{} {} {}", f(D(&v)), g(E(&v)), h(D(&v))); }
"#, &[(8, r#"lifetime may not live long enough"#)], "w1");
    errors_at(r#"struct D<'a>(&'a i32);
fn f<'a, 'b>(x: D<'a>) -> &'b i32 where D<'a>: 'b { x.0 }
fn k<'a, 'b, T>(x: &'a T) -> &'b T where &'a T: 'b { x }
fn caller<'x>(d: D<'x>) -> &'static i32 { f(d) }
fn caller2<'x>(d: D<'x>) -> &'x i32 { f(d) }
fn caller3<'x>(v: &'x i32) -> &'static i32 { k(v) }
fn main() { let v = 1; println!("{} {}", caller2(D(&v)), k(&v)); }
"#, &[(4, r#"lifetime may not live long enough"#), (6, r#"lifetime may not live long enough"#)], "w2");
}

#[test]
fn part30_index_deref_and_box_self() {
    // (a field read through a type's own Index and then its Deref (`v[0].f` with `Output = MyPtr<Foo>`) type-checks,
    // and its borrow is checked: E0506 for the assignment of `v` while `&v[0].f` lives, at rustc 1.97's line. A
    // `self: Box<Self>` method called on a temporary Box (`Box::new(S).f()`, `mk().f()`) type-checks)
    errors_at(r#"use std::ops::{Deref, Index};
struct MyVec<T> { x: T }
impl<T> Index<usize> for MyVec<T> { type Output = T; fn index(&self, _: usize) -> &T { &self.x } }
struct MyPtr<T> { x: T }
impl<T> Deref for MyPtr<T> { type Target = T; fn deref(&self) -> &T { &self.x } }
struct Foo { f: usize }
fn ok() -> usize {
    let v = MyVec { x: MyPtr { x: Foo { f: 22 } } };
    let i = &v[0].f;
    *i + v[0].f
}
fn bad() {
    let mut v = MyVec { x: MyPtr { x: Foo { f: 22 } } };
    let i = &v[0].f;
    v = MyVec { x: MyPtr { x: Foo { f: 23 } } };
    let _ = *i;
    let _ = v;
}
fn main() { println!("{}", ok()); bad(); }
"#, &[(15, r#"cannot assign to `v` because it is borrowed"#)], "ix");
    errors_at(r#"struct S;
impl S { fn f(self: Box<Self>) -> i32 { 1 } fn g(self: Box<Self>, k: i32) -> i32 { k } }
struct D<'a>(&'a i32);
impl D<'_> { fn get(self: Box<Self>) -> i32 { *self.0 } }
fn mk() -> Box<S> { Box::new(S) }
fn main() {
    let x = 5;
    println!("{} {} {} {}", Box::new(S).f(), mk().f(), Box::new(S).g(2), Box::new(D(&x)).get());
}
"#, &[], "bxall");
}

#[test]
fn part30_trait_object_lifetime_bounds() {
    // (std's `Any` is `trait Any: 'static`: a field `Box<dyn Any + 'a>` is E0478, as for a trait of the program's with
    // a `'static` bound. An elided object lifetime defaults to the trait's own bound, which it then meets: `&'a dyn
    // MyAny` is no error, `&'a (dyn MyAny + 'a)` is (line 4). Lines as rustc 1.97's)
    errors_at(r#"struct H<'a> { v: Box<dyn std::any::Any + 'a> }
struct G { v: Box<dyn std::any::Any> }
struct K<'a> { v: &'a (dyn std::any::Any + 'static) }
fn main() {}
"#, &[(1, r#"lifetime bound not satisfied"#)], "any2");
    errors_at(r#"trait MyAny: 'static {}
impl MyAny for i32 {}
struct S<'a> { v: &'a dyn MyAny }
struct T<'a> { v: &'a (dyn MyAny + 'a) }
fn main() { let x = 1; let s = S { v: &x }; let _ = s.v; }
"#, &[(4, r#"lifetime bound not satisfied"#)], "any4");
}

#[test]
fn part30_review() {
    // (a bound on a type alias implies nothing of the parts written in it, which what it stands for may not have:
    // `Erased<'a>: 'static` with `type Erased<'a> = ();` is no `'a: 'static` (2), where `W<'a>: 'static` of a struct
    // is, and its caller must meet it (5). Lines as rustc 1.97's)
    errors_at(r#"type Erased<'a> = ();
fn f<'a>(x: &'a i32) -> &'static i32 where Erased<'a>: 'static { x }
struct W<'a>(&'a i32);
fn g<'a>(x: &'a i32) -> &'static i32 where W<'a>: 'static { x }
fn main() { let v = 1; let _ = (f(&v), g(&v)); }
"#, &[(2, "lifetime may not live long enough"), (5, "does not live long enough")], "a1");
    // (an `unsafe fn` with `self: Box<Self>` called on a temporary Box outside `unsafe` is E0133; inside it is not)
    // (a `self: Box<Self>` method's result that borrows through the Box's value is tied to what it borrows, on a
    // Box local and on a temporary one: E0515 at each return, as in rustc 1.97)
    errors_at(r#"struct S<'a>(&'a i32);
impl<'a> S<'a> { fn get(self) -> &'a i32 { self.0 } fn getb(self: Box<Self>) -> &'a i32 { self.0 } }
fn f<'x>() -> &'x i32 { let x = 5; S(&x).get() }
fn g<'x>() -> &'x i32 { let x = 5; let b = Box::new(S(&x)); b.getb() }
fn h<'x>() -> &'x i32 { let x = 5; Box::new(S(&x)).getb() }
fn main() { let _ = (f(), g(), h()); }
"#, &[(3, "E0515"), (4, "E0515"), (5, "E0515")], "b2");
    // (`Self: 'static` of an impl whose self type is a type alias that drops its lifetime (`Erased<'a>` with
    // `type Erased<'a> = ();`) implies nothing of 'a; of a struct's (`D<'a>`) it implies 'a: 'b)
    errors_at(r#"type Erased<'a> = ();
trait Leak<'a> { fn leak(x: &'a i32) -> &'static i32 where Self: 'static; }
impl<'a> Leak<'a> for Erased<'a> { fn leak(x: &'a i32) -> &'static i32 where Self: 'static { x } }
struct D<'a>(&'a i32);
impl<'a> D<'a> { fn keep<'b>(self) -> &'b i32 where Self: 'b { self.0 } }
fn main() {}
"#, &[(3, "lifetime may not live long enough")], "sa");
    // (what `Self: 'b` implies of the self type's lifetimes is dropped only for those inside a type alias's arguments:
    // not for an impl's type parameter `T` that shadows a `type T`, nor `D<'a, T>` naming the alias `T` beside 'a,
    // nor 'a also written outside `Erased<'a>`, nor a trait object's lifetime; `W<Erased<'a>>` is an error (9))
    errors_at(r#"type T = ();
struct D<'a, X>(&'a i32, X);
impl<'a, T> D<'a, T> { fn keep<'b>(&self, x: &'a i32) -> &'b i32 where Self: 'b { x } }
fn f<'a, 'b, T>(x: E<'a, T>) -> &'b i32 where E<'a, T>: 'b { x.0 }
struct E<'a, X>(&'a i32, X);
fn g<'a, 'b>(x: &'a i32) -> &'b i32 where D<'a, T>: 'b { x }
fn main() { let n = 1; let d = D(&n, ()); println!("{} {} {}", d.keep(&n), f(E(&n, 0u8)), g(&n)); let _: T = (); }
"#, &[], "sh");
    errors_at(r#"type T = ();
type Erased<'a> = ();
struct D<'a, X>(&'a i32, X);
impl<'a> D<'a, T> { fn k1<'b>(&self, x: &'a i32) -> &'b i32 where Self: 'b { x } }
impl<'a> D<'a, Erased<'a>> { fn k2<'b>(&self, x: &'a i32) -> &'b i32 where Self: 'b { x } }
trait Tr {}
impl<'a> dyn Tr + 'a { fn k3<'b>(&self, x: &'a i32) -> &'b i32 where Self: 'b { x } }
struct W<X>(X);
impl<'a> W<Erased<'a>> { fn k4<'b>(&self, x: &'a i32) -> &'b i32 where Self: 'b { x } }
fn main() {}
"#, &[(9, "lifetime may not live long enough")], "sh2");
    // (a bound on a generic associated type with lifetime arguments (`T::A<'a>: 'static`) is one component of its
    // own: it implies nothing of 'a)
    errors_at(r#"trait Tr { type A<'x>; }
fn bad<'a, T: Tr>(x: &'a i32) -> &'static i32 where T::A<'a>: 'static { x }
fn main() {}
"#, &[(2, "lifetime may not live long enough")], "g1");
    // (an inherent `unsafe fn` with `self: Box<Self>` is chosen before a trait's method of the name, as in rustc:
    // E0133 outside `unsafe`)
    errors_at(r#"struct S;
impl S { unsafe fn f(self: Box<Self>) -> i32 { 1 } }
trait T { fn f(self: Box<Self>) -> i32; }
impl T for S { fn f(self: Box<Self>) -> i32 { 2 } }
fn main() { println!("{}", Box::new(S).f()); }
"#, &[(5, "call to unsafe function")], "un2");
    errors_at(r#"struct S;
impl S { unsafe fn f(self: Box<Self>) -> i32 { 1 } }
fn main() { let a = unsafe { Box::new(S).f() }; let b = Box::new(S).f(); println!("{} {}", a, b); }
"#, &[(3, "call to unsafe function")], "u1");
}

#[test]
fn part31_closure_error_position() {
    // (an error with no position of its own in a closure's body (a move out of a reference by its parameter's
    // pattern) is at the closure, on rustc 1.97's line, not at the line of the fn it is in)
    errors_at("use std::collections::HashMap;\n\nfn main() {\n    let _ = HashMap::<String, i32>::new().iter().filter(|&(&_k, &_v)| { true });\n}\n", &[(4, "cannot move out of")], "cp");
}

#[test]
fn part31_const_local_borrow() {
    // (a const's or a static's initializer block that ends in a borrow of one of its own locals is E0597, the value
    // being 'static: at rustc 1.97's line, also for one written inside a fn body (hoisted); a borrow of a literal is
    // promoted and is not)
    errors_at("const Z: &'static isize = {\n    let p = 3;\n    &p\n};\nfn main() { println!(\"{}\", Z); }\n", &[(3, "`p` does not live long enough")], "c1");
    errors_at("fn main() {\n    static Y: &'static isize = { let q = 4; &q };\n    println!(\"{}\", Y);\n}\n", &[(2, "`q` does not live long enough")], "c2");
    errors_at("const OK: &'static i32 = { let _p = 3; &5 };\nfn main() { println!(\"{}\", OK); }\n", &[], "c3");
}

#[test]
fn part31_collect_and_temporary_scrutinee() {
    // (a `collect` into a written `Vec<_>` yields the iterator's items (`&str`), as rustc selects `FromIterator` at the
    // `let`: `&parts[0]` returned as a `&'a str` is a `&&str` coerced, no E0515 (borrowck/
    // cannot-return-ref-to-fn-param-in-filter-map.rs's make_entry). Returning `&parts[0]` itself is E0515 (6))
    errors_at(r#"fn e1<'a>(s: &'a String) -> Option<&'a str> {
    let parts: Vec<_> = s.split('|').collect();
    if let [commit, ..] = &parts[..] { Some(commit) } else { None }
}
fn e2<'a>(s: &'a String) -> &'a str { let parts: Vec<_> = s.split('|').collect(); let c = &parts[0]; c }
fn bad<'a>(s: &'a String) -> &'a &'a str { let parts: Vec<_> = s.split('|').collect(); &parts[0] }
fn main() { let s = "1|2".to_string(); let _ = (e1(&s), e2(&s), bad(&s)); }
"#, &[(6, "cannot return")], "cl");
    // (a temporary scrutinee matched by a `&` pattern that binds by value moves out through the reference: E0508 /
    // E0507 at the scrutinee (2, 3, 4), as of a local; not by `ref` (1), nor a `Copy` binding (5). Lines as rustc 1.97's)
    errors_at(r#"fn f1(l1: &[String], l2: &[String]) { match (l1, l2) { (&[], &[ref hd, ..]) => { let _ = hd; } _ => {} } }
fn f2(l1: &[String], l2: &[String]) { match (l1, l2) { (&[], &[hd, ..]) => { drop(hd); } _ => {} } }
fn f3(a: &String, b: &String) { match (a, b) { (_, &s) => { drop(s); } } }
fn f4(a: &Option<String>) { match (1, a) { (_, &Some(s)) => { drop(s); } _ => {} } }
fn f5(a: &Option<u32>) -> u32 { match (1, a) { (_, &Some(n)) => n, _ => 0 } }
fn main() {}
"#, &[(2, "cannot move out of"), (3, "cannot move out of"), (4, "cannot move out of")], "ts");
    // (of the uses of what one move moved, the one rustc reports: after the loop the move is in, before its next
    // iteration's, also of a loop over a range and of nested loops)
    errors_at(r#"fn a(n: usize) {
    let s = String::new();
    for _ in 0..n { if s.len() > 0 { drop(s); continue; } }
    drop(s);
}
fn b(v: Vec<String>, w: Vec<&str>) {
    for foo in v { for bar in &w { if foo == *bar {
        drop(foo);
        continue;
    } }
    drop(foo);
    }
}
fn main() {}
"#, &[(4, "use of moved value"), (11, "use of moved value")], "mv");
}

#[test]
fn part31_review() {
    // (a `collect` in parentheses yields the iterator's items at its `let` too: no false E0515)
    errors_at(r#"fn e2<'a>(s: &'a String) -> &'a str { let parts: Vec<_> = (s.split('|').collect()); let c = &parts[0]; c }
fn main() { let s = "1|2".to_string(); println!("{}", e2(&s)); }
"#, &[], "pc");
    // (a `&&&u8` passed as a `&&u8` keeps its type: `***c` is a `u8`)
    errors_at(r#"fn take(_: &&u8) {}
fn main() { let x = 1u8; let c = &&&x; take(c); let y: u8 = ***c; println!("{}", y); }
"#, &[], "dd");
    // (a const's block that borrows through a local reference (`let p = &DATA; &p.0`) borrows DATA: no E0597 (2);
    // of a local tuple's field it is E0597 (3), as in rustc 1.97)
    errors_at(r#"static DATA: (i32,) = (3,);
const OK: &'static i32 = { let p = &DATA; &p.0 };
const BAD: &'static i32 = { let p = (3,); &p.0 };
fn main() { println!("{} {}", OK, BAD); }
"#, &[(3, "does not live long enough")], "cr");
    // (nor through a local whose written type is an alias of a reference (3): the summary does not know aliases, so a
    // written path counts as the local's own storage only where its value is built there (5))
    errors_at(r#"static DATA: (i32,) = (3,);
type Alias = &'static (i32,);
const OK: &'static i32 = { let p: Alias = &DATA; &p.0 };
type Pair = (i32,);
const BAD: &'static i32 = { let p: Pair = (4,); &p.0 };
fn main() { println!("{} {}", OK, BAD); }
"#, &[(5, "does not live long enough")], "ca");
    // (a move out through a `&mut` pattern of a temporary scrutinee is out of a mutable reference)
    errors_at(r#"fn f(a: &mut Option<String>) { match (1, a) { (_, &mut Some(s)) => { drop(s); } _ => {} } }
fn main() {}
"#, &[(1, "cannot move out of a mutable reference")], "mr");
}

#[test]
fn part32_shared_ref_captures_by_deref() {
    // (edition 2021: a closure that uses a shared reference it captures (`h: &T`, `&self`) only through a deref of it
    // (`h.mk()`, `self.n()`, `o.name.cmp(..)`) captures `*h` by reference, the same as a copy of h: collected from a
    // `filter_map` or boxed, it may outlive the fn (no E0373), as in rustc 1.97. A local String it borrows (10) is
    // E0373; using `h` itself (`let x = h`, 11) is rustc's E0373 too, not reported yet)
    errors_at(r#"struct H<'a>(&'a str);
impl<'a> H<'a> {
    fn mk(&self) -> Option<&str> { Some(self.0) }
    fn n(&self) -> usize { self.0.len() }
    fn all<'s>(&'s self, v: &[u8]) -> Vec<&'s str> { v.iter().filter_map(|_| self.mk()).collect() }
    fn boxed<'s>(&'s self) -> Box<dyn Fn() -> usize + 's> { Box::new(|| self.n()) }
}
fn f4<'r>(h: &'r H<'r>, v: Vec<u8>) -> Vec<&'r str> { v.iter().filter_map(|_| h.mk()).collect() }
fn f5<'r>(h: &'r H<'r>) -> Box<dyn Fn() -> usize + 'r> { Box::new(|| h.n()) }
fn f6<'r>(h: &'r H<'r>) -> Box<dyn Fn() -> usize + 'r> { let s = String::new(); Box::new(|| s.len() + h.n()) }
fn f7<'r>(h: &'r H<'r>) -> Box<dyn Fn() -> &'r H<'r> + 'r> { Box::new(|| { let x = h; x }) }
fn main() { let h = H("ab"); println!("{:?} {} {:?} {} {}", h.all(&[1, 2]), h.boxed()(), f4(&h, vec![1]), f5(&h)(), f7(&h)().0); let _ = f6(&h); }
"#, &[(10, "closure may outlive")], "dr");
    errors_at(r#"use std::cmp::Ordering;
struct Task { pri: u32, name: String }
fn c1(s: &Task, o: &Task) -> Ordering { s.pri.cmp(&o.pri).then_with(|| o.name.cmp(&s.name)) }
fn c2(s: &Task, o: &Task) -> bool { let f = || o.name.len() > s.name.len(); f() }
fn c3(s: &Task) -> usize { let f = || s.name.len(); f() }
fn main() { let t = Task { pri: 1, name: "a".into() }; println!("{:?} {} {}", c1(&t, &t), c2(&t, &t), c3(&t)); }
"#, &[], "dc");
    // (a `for` over an Option's `iter()` binds its pattern at the iterable: a move out through `&a` is E0507 at
    // `x.iter()` (3), as in rustc 1.97, not at the fn's line)
    errors_at(r#"fn main() {
    let x: Option<Box<i32>> = Some(Box::new(1));
    for &a in x.iter() {
        let _ = a;
    }
    let y = Some(String::new());
    for s in y.iter() { println!("{}", s); }
}
"#, &[(3, "cannot move out of a shared reference")], "fo");
    // (a `&mut T` wanted of a `&mut &T`: the deref coercion borrows mutably through a `&`, E0596 where the value is
    // coerced (2; rustc also reports the `let .. else` at 6, zustc a fn's first error); not of a `&mut &mut T` (4))
    errors_at(r#"fn main() {
    let a: &mut Option<i32> = &mut &Some(5i32);
    let mut o = Some(1);
    let b: &mut Option<i32> = &mut &mut o;
    let _ = (a, b);
    let Some(n): &mut Option<i32> = &mut &Some(5i32) else { return };
    *n += 1;
}
"#, &[(2, "E0596")], "ms");
}

#[test]
fn part32_const_nested_block_borrow() {
    // (a block in a const's initializer whose value borrows one of its own locals, also inside a call in an `unsafe`
    // block: E0597 (1), as in rustc 1.97; a block whose value copies its local out is not (2, 3))
    // (of a raw pointer's type, a borrow of the block's own local is no E0597 but a dangling pointer, at the item)
    errors_at("const FOO: *const u32 = { let x = 42; &x };\nfn main() {}\n", &[(1, "encountered dangling pointer")], "dp");
    // (a local with no value: E0381 at the borrow, whatever the const's type (consts/issue-78655.rs))
    errors_at("const FOO: *const u32 = {\n    let x;\n    &x\n};\nfn main() {}\n", &[(3, "E0381")], "ui");
    errors_at(r#"const A: &'static [i32; 3] = unsafe { std::mem::transmute({ let _ = 1; { let s = [0i32; 3]; &s } }) };
const B: usize = { let v = [1, 2]; v.len() };
const C: &'static i32 = &{ let q = 5; q };
fn main() { println!("{:?} {} {}", A, B, C); }
"#, &[(1, "`s` does not live long enough")], "cn");
}

#[test]
fn part32_closure_pattern_captures() {
    // (edition 2021: a closure's pattern that tests nothing captures nothing: the unit variant of an enum of one
    // variant (5), a unit struct (6); a struct pattern captures the fields it names (7). An enum of two variants is
    // read (8: E0505, as in rustc 1.97))
    errors_at(r#"enum SingleVariant { A }
enum Two { A, B }
struct Unit;
struct SStruct { a: u32, b: u32 }
fn a() { let sv = SingleVariant::A; let c = || { match sv { SingleVariant::A => () } }; let _m = sv; c(); }
fn b(mut x: (Unit, u32)) { let r = &mut x.0; let _ = || { let (Unit, a) = x; a }; let _ = *r; }
fn c(mut x: SStruct) { let r = &mut x.a; let _ = || { let SStruct { b, .. } = x; b }; let _ = *r; }
fn d() { let t = Two::A; let c = || { match t { Two::A => (), Two::B => () } }; let _m = t; c(); }
fn main() { a(); }
"#, &[(8, "cannot move out of `t` because it is borrowed")], "pc");
}

#[test]
fn part32_review() {
    // (review of #159, each case checked with rustc 1.97: a `let` of a raw pointer's type in a const coerces its
    // block's tail before the block ends (1: no error), a reference's does not (4: E0597))
    errors_at("const C: () = {\n    let _: *const u32 = { let x = 42; &x };\n};\nconst E: () = {\n    let _: *const u32 = { let y: &u32 = { let x = 42; &x }; y };\n};\nfn main() { let _ = C; let _ = E; }\n", &[(5, "`x` does not live long enough")], "rp");
    // (a closure's pattern that binds a `&mut` field by value moves it (6, 12: E0382 after the call; the closure is
    // `FnOnce`, 17), one that binds a Copy field reads it (22: no error))
    errors_at(r#"struct S<'a> { p: &'a mut i32, q: i32 }
struct T<'a>(&'a mut i32);
fn a(mut n: i32) {
    let s = S { p: &mut n, q: 1 };
    let c = || { let S { p, .. } = s; p };
    c(); let _ = &s.p;
}
fn b(mut n: i32) {
    let s = T(&mut n);
    let c = || { let T(p) = s; p };
    c();
    let _ = &s.0;
}
fn d(mut n: i32) {
    let s = (&mut n, 1);
    let mut c = || { let (p, _) = s; *p += 1; };
    c(); c();
}
fn e(mut n: i32) {
    let s = S { p: &mut n, q: 1 };
    let c = || { let S { q, .. } = s; q };
    c(); let _ = &s.p;
}
fn main() {}
"#, &[(6, "use of moved value: `s.p`"), (12, "use of moved value: `s.0`"), (17, "use of moved value: `c`")], "rm");
    // (a constant of an enum of one variant in a closure's pattern compares the value: it is read (7: E0506); a
    // union's pattern captures the union (12: E0506 for a write to another field), and names one field)
    errors_at(r#"#[derive(PartialEq, Eq)]
enum E { V(u32) }
impl E { const ZERO: E = E::V(0); }
union U { a: u32, b: u32 }
fn f() {
    let mut e = E::V(1);
    let c = || match e { E::ZERO => 1, _ => 0 };
    e = E::V(2);
    c();
}
fn g() {
    let mut u = U { a: 1 };
    let c = || unsafe { let U { a: ref x } = u; };
    u.b = 2;
    c();
}
fn h() -> u32 { let u = U { a: 1 }; unsafe { let U { b } = u; b } }
fn main() {}
"#, &[(8, "cannot assign to `e` because it is borrowed"), (14, "cannot assign to `u.b` because it is borrowed")], "rc");
    // (second review: a union's pattern borrows the union's memory, so a write to the field it binds or to another one
    // is E0506 while the binding lives (6, 14); a pattern that binds nothing of the union (`U { a: _ }`) captures
    // nothing (21: no error))
    errors_at(r#"union U { a: u32, b: u32 }
fn f() {
    let mut u = U { a: 1 };
    unsafe {
        let U { a: ref x } = u;
        u.b = 2;
        println!("{}", x);
    }
}
fn g() {
    let mut u = U { a: 1 };
    unsafe {
        let U { a: ref x } = u;
        u.a = 2;
        println!("{}", x);
    }
}
fn h() {
    let mut u = U { a: 1 };
    let c = || { let U { a: _ } = u; };
    u.b = 2;
    c();
}
fn main() {}
"#, &[(6, "cannot assign to `u.b` because it is borrowed"), (14, "cannot assign to `u.a` because it is borrowed")], "ru");
    // (a constant of another type with the variant's name (`Other::V`) compares the value: it is read (10: E0506); the
    // variant itself (`E::V`) is not (16: no error))
    errors_at(r#"#[derive(PartialEq, Eq)]
enum E { V }
struct Other;
impl Other {
    const V: E = E::V;
}
fn f() {
    let mut e = E::V;
    let c = || match e { Other::V => 1, _ => 0 };
    e = E::V;
    c();
}
fn g() {
    let mut e = E::V;
    let c = || match e { E::V => 1 };
    e = E::V;
    c();
}
fn main() {}
"#, &[(10, "cannot assign to `e` because it is borrowed")], "ro");
}

#[test]
fn part33_pattern_moves_and_guards() {
    // (each case checked with rustc 1.97: zustc reports each fn's first error on rustc's line)
    // (a `ref mut` binding where the default binding mode is by shared reference: E0596 at the binding (2, 5); an
    // explicit `&` pattern's binding through a `&` is the borrow check's (8); through a `&mut`, accepted (11))
    errors_at(r#"enum Foo { Bar(i32) }
fn a(arr: Vec<Foo>) { if let (Some(Foo::Bar(ref mut val)), _) = (&arr.get(0), 0) { *val = 1; } }
fn b(o: &Option<i32>) {
    match o {
        Some(ref mut v) => *v = 1,
        None => {}
    }
}
fn c(o: &(i32, i32)) { let &(ref mut x, _) = o; *x = 1; }
fn d(o: &mut Option<(i32, String)>) { if let Some((ref mut x, ref mut s)) = o { *x = 1; s.push('x'); } }
fn main() {}
"#, &[(2, "cannot borrow data in a `&` reference as mutable"), (5, "cannot borrow data in a `&` reference as mutable"), (9, "as mutable")], "rm");
    // (a pattern that moves a field reached through an overloaded deref: E0507 at the scrutinee, also where the
    // binding needs no drop (6, 9); a Copy field and a `ref` binding are accepted (12, 15); a `Box`'s field moves (18))
    errors_at(r#"use std::ops::Deref;
struct Wrap<T>(T);
struct NonCopy(());
struct St { field: Option<NonCopy>, n: Option<i32>, s: Option<String> }
impl<T> Deref for Wrap<T> { type Target = T; fn deref(&self) -> &T { &self.0 } }
fn a(w: Wrap<St>) -> usize { if let Some(_v) = w.field { 1 } else { 0 } }
fn b(w: Wrap<St>) -> usize {
    match w.s {
        Some(v) => v.len(),
        None => 0,
    }
}
fn c(w: Wrap<St>) -> i32 { if let Some(n) = w.n { n } else { 0 } }
fn d(w: Wrap<St>) -> usize {
    match w.s { Some(ref v) => v.len(), None => 0 }
}
fn e(w: Box<St>) -> usize {
    match w.s { Some(v) => v.len(), None => 0 }
}
fn main() {}
"#, &[(6, "cannot move out of dereference of `Wrap<St>`"), (8, "cannot move out of dereference of `Wrap<St>`")], "od");
    // (a move out of what a reference that is no place points to: E0507 (2, 3, 5); a borrow of it, a Copy part, a
    // method call through it are accepted (6))
    errors_at(r#"struct A(String);
fn a(x: A) { let _y = *&x; }
fn b(v: Vec<String>) -> String { *v.first().unwrap() }
fn id<T>(x: &T) -> &T { x }
fn c(v: String) -> String { *id(&v) }
fn d(v: Vec<String>) -> usize { let r = &*v.first().unwrap(); let n = (*v.first().unwrap()).len(); r.len() + n }
fn main() {}
"#, &[(2, "cannot move out of a shared reference"), (3, "cannot move out of a shared reference"), (5, "cannot move out of a shared reference")], "nr");
    // (in a match guard the arm's bindings are seen by shared reference: a move (3), an assignment (9), a mutable
    // borrow (14), one through a `&mut` binding (19) are E0507 / E0594 / E0596; reads, clones and the arm's own uses
    // are accepted (23))
    errors_at(r#"fn a(x: Option<String>) -> usize {
    match x {
        Some(v) if { drop(v); false } => 1,
        _ => 0,
    }
}
fn b(x: Option<i32>) -> usize {
    match x {
        Some(mut y) if { y += 1; false } => 1,
        _ => 0,
    }
}
fn c(x: Option<Vec<i32>>) -> usize {
    match x { Some(v) if { v.push(1); true } => 1, _ => 0 }
}
fn d(o: Option<&mut Vec<i32>>) -> usize {
    match o {
        Some(r) if r.len() > 1 => { r.push(1); r.len() }
        Some(r) if { r.push(1); true } => 0,
        _ => 0,
    }
}
fn e(x: Option<String>) -> usize {
    match x { Some(v) if { let w = v.clone(); w.len() > 1 } => v.len(), Some(mut v) => { v.push('a'); v.len() }, None => 0 }
}
fn main() {}
"#, &[(3, "cannot move out of `v` in pattern guard"), (9, "cannot assign to `y`, as it is immutable for the pattern guard"), (14, "cannot borrow `v` as mutable, as it is immutable for the pattern guard"), (19, "cannot borrow `*r` as mutable, as it is immutable for the pattern guard")], "pg");
}

#[test]
fn part33_receivers_returned_closures_and_aggregates() {
    // (each case checked with rustc 1.97: zustc reports each fn's first error on rustc's line)
    // (a `self: Box<Self>` or `self: Pin<&mut Self>` receiver moves into the call: a use after is E0382 (10, 16))
    errors_at(r#"use std::pin::Pin;
struct Foo(i32);
impl Foo {
    fn bar(self: Box<Self>) {}
    fn baz(self: Pin<&mut Self>) {}
}
fn a() {
    let b = Box::new(Foo(1));
    b.bar();
    b.bar();
}
fn c() {
    let mut f = Foo(1);
    let p = Pin::new(&mut f);
    p.baz();
    p.baz();
}
fn main() {}
"#, &[(10, "use of moved value: `b`"), (16, "use of moved value: `p`")], "rv");
    // (a closure that borrows a local, held in a local returned as an `impl Trait`: rustc's E0597 at the use in its
    // body of what it captures (7, the `p` that `println!` names, not `p.x`); returned itself or in a `Box<dyn ..>`,
    // E0373 at the closure (13))
    errors_at(r#"#[derive(Debug)]
struct Point { x: i32, y: i32 }
fn foo() -> impl FnMut() {
    let mut p = Point { x: 1, y: 2 };
    let c = || {
        p.x += 5;
        println!("{:?}", p);
    };
    c
}
fn bar() -> Box<dyn Fn() -> i32> {
    let p = Point { x: 1, y: 2 };
    let d = || p.x;
    Box::new(d)
}
fn main() {}
"#, &[(7, "`p` does not live long enough"), (13, "closure may outlive the current function")], "rc");
    // (a tuple, an array or a struct's literal returned holds what its parts borrow: E0515 also where the signature
    // has one region (1, 2, 3))
    errors_at(r#"fn a(x: &str) -> (&str, &&str) { (x, &x) }
fn b(x: &i32) -> [&i32; 1] { let y = *x; [&y] }
struct W<'a> { r: &'a i32 }
fn c(x: &i32) -> W<'_> { let y = *x; W { r: &y } }
fn d(x: &i32) -> (&i32, u8) { (x, 1) }
fn main() {}
"#, &[(1, "cannot return"), (2, "cannot return"), (4, "cannot return")], "ag");
}

#[test]
fn part33_review_of_159() {
    // (the minor findings of #159's third review, each checked with rustc 1.97: a variant written through a type alias
    // of the enum (`A::V`) captures nothing (5: no error); a guard's write to another field of a union the pattern
    // tests is E0510 (13), as one to the field itself (21))
    errors_at(r#"enum E { V }
type A = E;
fn f() {
    let mut e = E::V;
    let c = || match e { A::V => 1 };
    e = E::V;
    c();
}
union U { a: u32, b: u32 }
fn g(mut u: U) {
    unsafe {
        match u {
            U { a: 0 } if { u.b = 1; true } => (),
            _ => (),
        }
    }
}
fn h(mut u: U) {
    unsafe {
        match u {
            U { a: 0 } if { u.a = 1; true } => (),
            _ => (),
        }
    }
}
fn main() {}
"#, &[(13, "cannot assign `u.b` in match guard"), (21, "cannot assign `u.a` in match guard")], "r3");
}

#[test]
fn part33_review_of_160() {
    // (#160's review, each checked with rustc 1.97: a unit struct's name in a `let` binds and moves nothing, through a
    // Deref too (no error); a struct update whose base gives only fields that hold no reference keeps none of the
    // base's loans (no E0515 at 13), one whose base gives a reference field does (E0515 at 19))
    errors_at(r#"use std::ops::Deref;
#[derive(Debug)]
struct Unit;
struct W(Unit);
impl Deref for W { type Target = Unit; fn deref(&self) -> &Unit { &self.0 } }
fn a() { let w = W(Unit); let Unit = *w; }
fn b() { let u = Unit; let Unit = u; println!("{:?}", u); }
struct S<'a> { r: &'a i32, n: usize }
fn c<'a>(x: &'a i32) -> S<'a> {
    let y = 5;
    let s = S { r: &y, n: 1 };
    let _ = s.r;
    S { r: x, ..s }
}
struct V<'a> { r: &'a i32, q: &'a i32 }
fn d<'a>(x: &'a i32) -> V<'a> {
    let y = 5;
    let s = V { r: &y, q: x };
    V { r: x, ..s }
}
fn main() { a(); b(); let z = 3; c(&z); d(&z); }
"#, &[(19, "`y` is dropped when the function returns")], "rv1");
    // (where a written field may hold a reference too, the base's loan may be in its region only: kept for a struct
    // of one lifetime that a given field has (E0515 at 6), not for one of two (2), a field of 'static (4) or type
    // parameters (8))
    errors_at(r#"struct V<'a, 'b> { r: &'a i32, q: &'b i32 }
fn d<'a, 'b>(x: &'a i32, z: &'b i32) -> V<'a, 'b> { let y = 5; let s = V { r: &y, q: z }; V { r: x, ..s } }
struct S<'a> { st: &'static str, r: &'a i32 }
fn a<'a>(x: &'a i32) -> S<'a> { let y = 5; let s = S { st: "x", r: &y }; S { r: x, ..s } }
struct T2<'a> { r: &'a i32, v: Vec<&'a i32> }
fn b<'a>(x: &'a i32) -> T2<'a> { let y = 5; let s = T2 { r: &y, v: vec![] }; T2 { r: x, ..s } }
struct G<A, B> { a: A, b: B }
fn c<'a>(x: &'a i32) -> G<&'a i32, &'static i32> { let y = 5; let s = G { a: &y, b: &3 }; G { a: x, ..s } }
fn main() { let z = 3; d(&z, &z); a(&z); b(&z); c(&z); }
"#, &[(6, "`y` is dropped when the function returns")], "rv3");
    // (in a guard, a closure that borrows an arm's binding mutably (E0596 at 4), and a write to an outer arm's binding
    // in a guard nested in its guard (E0594 at 12))
    errors_at(r#"fn e() {
    let x = Some(3);
    match x {
        Some(mut y) if { let mut c = || { y += 1; }; c(); true } => { let _ = y; }
        _ => {}
    }
}
fn f() {
    let x = Some(3);
    match x {
        Some(mut y) if match 0 {
            _ if { y += 1; true } => true,
            _ => false,
        } => { let _ = y; }
        _ => {}
    }
}
fn main() { e(); f(); }
"#, &[(4, "cannot borrow `y` as mutable, as it is immutable for the pattern guard"), (12, "cannot assign to `y`, as it is immutable for the pattern guard")], "rv2");
}

#[test]
fn part34_static_bounds_of_impl_args_and_fields() {
    // (an `impl Trait + 'static` argument's bound reaches the caller, as a written `T: 'static` does: E0310 at 2,
    // named `impl Debug` as rustc names it; one bounded so itself passes it on (4). Each checked with rustc 1.97)
    errors_at(r#"use std::fmt::Debug;
fn foo(d: impl Debug) { bar(d); }
fn bar(d: impl Debug + 'static) { let _ = d; }
fn ok(d: impl Debug + 'static) { bar(d); }
fn main() {}
"#, &[(2, "the parameter type `impl Debug` may not live long enough")], "sb1");
    // (a field that passes a type parameter to a type of the program that bounds it by 'static: rustc does not infer
    // `U: 'static` from it (RFC 2093), E0310 at the field; also in an enum's variant, through a Vec)
    errors_at(r#"struct Bar<T: 'static> { x: T }
struct Foo<U> { bar: Bar<U> }
fn main() {}
"#, &[(2, "the parameter type `U` may not live long enough")], "sb2");
    errors_at(r#"struct Bar<T: 'static> { x: T }
enum E<U> { A(Vec<Bar<U>>) }
fn main() {}
"#, &[(2, "the parameter type `U` may not live long enough")], "sb3");
    // (declared inline, in a where clause, through a trait that implies it (Any), or of 'static data: accepted; a
    // fn's parameter type is implied well formed)
    errors_at(r#"struct Bar<T: 'static> { x: T }
struct Ok1<U: 'static> { bar: Bar<U> }
struct Ok2<U> where U: 'static { bar: Bar<U> }
struct Ok3<U: std::any::Any> { bar: Bar<U> }
struct Ok4 { b: Bar<&'static str> }
fn f<T>(x: Bar<T>) { let _ = x; }
fn main() {}
"#, &[], "sb4");
}

#[test]
fn part34_array_element_coercions_and_cell_stores() {
    // (an array's and a repeat's element coerced by deref to the element type the context gives: `[&x, x]` with x:
    // `&[u8]` as `[&[u8]; 2]`, `[&s, "b"]` of a String as `[&str; 2]`. Checked with rustc 1.97)
    errors_at(r#"fn g(x: &[u8]) -> [&[u8]; 2] { [&x, x] }
fn h(x: &[u8]) -> [&[u8]; 1] { [&x; 1] }
fn main() { let s = String::from("a"); let a: [&str; 2] = [&s, "b"]; let _ = a; let v = vec![1u8]; g(&v); h(&v); }
"#, &[], "ac1");
    // (a Cell's `set` stores what its argument borrows in the cell: a value whose Drop may see it, holding a borrow
    // of one dropped before it (6), or of itself (10), also through a Vec's element (15), is E0597 at the borrow; a
    // type with no Drop is accepted (19-23))
    errors_at(r#"use std::cell::Cell;
struct D<'a>(Cell<Option<&'a D<'a>>>);
impl Drop for D<'_> { fn drop(&mut self) {} }
fn a() {
    let (d1, d2) = (D(Cell::new(None)), D(Cell::new(None)));
    d1.0.set(Some(&d2));
}
fn b() {
    let d = D(Cell::new(None));
    d.0.set(Some(&d));
}
struct C<'a> { v: Vec<D<'a>> }
fn c() {
    let (c1, c2) = (C { v: vec![D(Cell::new(None))] }, C { v: vec![] });
    c1.v[0].0.set(Some(&c2.v[0]));
}
struct N<'a>(Cell<Option<&'a N<'a>>>);
fn n() {
    let d = N(Cell::new(None));
    d.0.set(Some(&d));
    let (n1, n2) = (N(Cell::new(None)), N(Cell::new(None)));
    n1.0.set(Some(&n2));
    n2.0.set(Some(&n1));
}
fn main() { a(); b(); c(); n(); }
"#, &[(6, "`d2` does not live long enough"), (10, "`d` does not live long enough"), (15, "`c2` does not live long enough")], "cs1");
}

#[test]
fn part34_review_of_161() {
    // (#161's review, each checked with rustc 1.97: an `impl Trait + 'static` inside a parameter's type requires what
    // the argument borrows to be 'static, as a top-level one does (E0597 at 5, the fn's first); `let _ = d;` is no drop,
    // so a value with no Drop holding a loan of itself is accepted there (13); a move of a Drop value on one path does
    // not hide a loan of itself on another (E0597 at 20))
    errors_at(r#"fn take(_: Option<impl Copy + 'static>) {}
fn take2(_: impl Copy + 'static) {}
fn a() {
    let x = 1;
    take(Some(&x));
    take2(&x);
}
use std::cell::Cell;
struct N<'a>(Cell<Option<&'a N<'a>>>);
fn b() {
    let d = N(Cell::new(None));
    d.0.set(Some(&d));
    let _ = d;
}
struct D<'a>(Cell<Option<&'a D<'a>>>);
impl Drop for D<'_> { fn drop(&mut self) {} }
fn c(k: bool) {
    let d = D(Cell::new(None));
    if k {
        d.0.set(Some(&d));
    } else {
        drop(d);
    }
}
fn main() { a(); b(); c(true); }
"#, &[(5, "`x` does not live long enough"), (20, "`d` does not live long enough")], "rv1");
    // (a cell reached through a deref (`Rc<Cell<..>>`) stores the borrow in the local it is rooted at (E0597 at 7);
    // an array's element coerced from a `&mut &mut i32` reborrows `**rr`, which the array holds (E0506 at 13))
    errors_at(r#"use std::rc::Rc;
use std::cell::Cell;
fn a() {
    let c: Rc<Cell<Option<&i32>>> = Rc::new(Cell::new(None));
    {
        let x = 1;
        c.set(Some(&x));
    }
    let _ = c.get();
}
fn b(rr: &mut &mut i32) {
    let a: [&mut i32; 1] = [rr];
    **rr = 1;
    println!("{}", *a[0]);
}
fn main() { a(); let mut v = 0; let mut r = &mut v; b(&mut r); }
"#, &[(7, "`x` does not live long enough"), (13, "cannot assign to `**rr` because it is borrowed")], "rv2");
}

#[test]
fn part35_elided_lifetimes_in_impl_headers_and_async_fns() {
    // (an impl header writes a path's lifetimes: `impl MyTrait for Foo` with `struct Foo<'a>` is E0726 at Foo (4), as
    // `impl Lt for u32` with `trait Lt<'a>` is at Lt; `Foo<'_>` and the impl's own are accepted (5-7). Each checked
    // with rustc 1.97)
    errors_at(r#"trait MyTrait {}
trait Lt<'a> {}
struct Foo<'a>(&'a u8);
impl MyTrait for Foo {}
impl MyTrait for &Foo<'_> {}
impl<'a> Lt<'a> for Foo<'a> {}
impl Lt<'_> for u32 {}
fn main() {}
"#, &[(4, "implicit elided lifetime not allowed here (E0726)")], "el1");
    errors_at(r#"trait Lt<'a> {}
impl Lt for u32 {}
fn main() {}
"#, &[(2, "implicit elided lifetime not allowed here (E0726)")], "el2");
    // (an `async fn`'s parameter too: its future captures the lifetime; a fn that is not async elides it (6))
    errors_at(r#"struct HasLifetime<'a>(&'a bool);
async fn error(lt: HasLifetime) {
    if *lt.0 {}
}
async fn ok(lt: HasLifetime<'_>) { if *lt.0 {} }
fn no_error(lt: HasLifetime) { if *lt.0 {} }
fn main() {}
"#, &[(2, "implicit elided lifetime not allowed here (E0726)")], "el3");
}

#[test]
fn part35_drop_of_a_type_that_holds_no_reference() {
    // (#161's last finding, checked with rustc 1.97: the `Drop` of a type that holds no reference (`Token`) sees no
    // loan, so a tuple with it that holds a borrow of its own field (5), or of a local gone before it (11), is accepted;
    // a `Drop` type that holds the borrow still sees it (E0597 at 21))
    errors_at(r#"use std::cell::Cell;
struct Token;
impl Drop for Token { fn drop(&mut self) {} }
fn a() {
    let d = (Cell::new(None), 1, Token);
    d.0.set(Some(&d.1));
}
fn b() {
    let mut d = (None, Token);
    {
        let x = 1;
        d.0 = Some(&x);
    }
}
struct D<'a>(Option<&'a i32>);
impl Drop for D<'_> { fn drop(&mut self) {} }
fn c() {
    let mut d = D(None);
    {
        let x = 1;
        d.0 = Some(&x);
    }
}
fn main() { a(); b(); c(); }
"#, &[(21, "`x` does not live long enough")], "dk1");
}

#[test]
fn part36_fn_bounded_parameters_boxed_as_trait_objects() {
    // (zustc check of a generic fn nothing calls boxes a placeholder bounded by `FnMut(..)` into a `Box<dyn FnMut(..)>`:
    // it was "not a closure", so the fn went unchecked. Each was checked with rustc 1.97: f is moved (E0382 at 3), F
    // must outlive `'static` for a `Box<dyn FnMut(bool)>` (E0310 at 6), and the body's other errors are found (E0382
    // at 11); d and e are accepted)
    errors_at(r#"fn a<F: FnMut(bool)>(f: F) {
    let b: Box<dyn FnMut(bool)> = Box::new(f);
    drop(f);
}
fn b<F: FnMut(bool)>(f: F) -> Box<dyn FnMut(bool)> {
    Box::new(f)
}
fn c<F: FnMut(bool) + 'static>(f: F) -> Box<dyn FnMut(bool)> {
    let s = String::new();
    drop(s);
    drop(s);
    Box::new(f)
}
fn d<F: Fn() + 'static>(f: F) -> Box<dyn Fn()> {
    Box::new(f)
}
fn e<'a, F: FnOnce() -> u8 + 'a>(f: F) -> Box<dyn FnOnce() -> u8 + 'a> {
    Box::new(f)
}
fn main() {}
"#, &[(3, "use of moved value: `f`"), (6, "the parameter type `F` may not live long enough"), (11, "use of moved value: `s`")], "fb1");
}

#[test]
fn part36_items_in_const_and_static_initializers() {
    // (a fn written in a `const` / `static` initializer's block is hoisted as one in a fn body is, so it is checked:
    // E0507 at 3 and E0596 at 9, as in rustc 1.97 (borrowck/borrowck-fn-in-const-a.rs, span/borrowck-fn-in-const-b.rs);
    // the two `broken` are two fns)
    errors_at(r#"const MOVE: fn(&String) -> String = {
    fn broken(x: &String) -> String {
        return *x
    }
    broken
};
static MUTATE: fn(&Vec<String>) = {
    fn broken(x: &Vec<String>) {
        x.push(String::new());
    }
    broken
};
const OK: fn() -> i32 = {
    fn fine() -> i32 { 1 }
    fine
};
fn main() { let _ = OK(); }
"#, &[(3, "cannot move out of `*x`"), (9, "cannot borrow `*x` as mutable")], "ci1");
}

#[test]
fn part36_bindings_after_at() {
    // (rustc's check of `x @ sub` patterns, and the moves of their bindings, each checked with rustc 1.97 at the same
    // position (the parameter's, at its fn): a by-move x with a `ref` in sub (5, 12), a `ref` x with a move in sub
    // (6, 13), `ref mut` with `ref` (7, 9) and with `ref mut` (8), and x moved after sub moved a part of it (10) or all
    // of it (11). Copy parts, two `ref`s, `ref x @ copy` and default binding modes are accepted, as is `ref x` as a
    // parameter)
    errors_at(r#"struct U;
#[derive(Clone, Copy)]
struct C;
struct NC<A, B>(A, B);
fn a() { let a @ ref b = U; }
fn b() { let ref a @ b = U; }
fn c() { let ref mut a @ ref b = U; }
fn d() { let ref mut a @ ref mut b = U; }
fn e() { let ref a @ ref mut b = U; }
fn f() { let a @ NC(b, c @ NC(d, e)) = NC(C, NC(C, C)); }
fn g() { let a @ b = U; }
fn h(x: Option<String>) { match x { s @ Some(ref t) => {} None => {} } }
fn i(ref a @ b: U) {}
fn ok() {
    let a @ NC(b, c) = NC(C, C);
    let ref a @ ref b = U;
    let ref a @ b = C;
    match &Some(U) { a @ Some(b) => {} None => {} }
}
fn ok2(ref x: U) -> usize { let _ = x; 0 }
fn main() { ok(); ok2(U); }
"#, &[
        (5, "borrow of moved value"),
        (6, "cannot move out of value because it is borrowed"),
        (7, "cannot borrow value as immutable because it is also borrowed as mutable"),
        (8, "cannot borrow value as mutable more than once at a time"),
        (9, "cannot borrow value as mutable because it is also borrowed as immutable"),
        (10, "use of partially moved value: `a`"),
        (11, "use of moved value: `a`"),
        (12, "borrow of moved value"),
        (13, "cannot move out of value because it is borrowed"),
    ], "at1");
}

#[test]
fn part36_lifetime_bounds_of_impl_methods() {
    // (checked with rustc 1.97: a trait method's lifetime that a where clause bounds a type by (`where T: 'a`) is
    // early-bound, so the impl's, which nothing bounds, does not match it (E0195 at 9); a type parameter of an impl's
    // method bounded by `'static` where the trait's is bounded by its `'a` is stricter (E0276 at 7), not where the
    // impl gives the trait `'static` (C); a GAT's `where 'b2: 'a2` for the trait's `where 'a1: 'b1` is stricter (E0276
    // at 7, the bound), its `'a2: 'b2` is not)
    errors_at(r#"use std::fmt::Debug;
struct State;
trait Foo<T> {
    fn foo<'a>(&self, state: &'a State) -> &'a T
    where
        T: 'a;
}
impl<T> Foo<T> for Vec<T> {
    fn foo<'a>(&self, state: &'a State) -> &'a T {
        &self[0]
    }
}
fn main() {}
"#, &[(9, "lifetime parameters or bounds on method `foo` do not match the trait declaration")], "lb1");
    errors_at(r#"use std::fmt::Debug;
trait A<'a> {
    fn put<F>(&mut self, f: F) where F: Debug + 'a;
}
struct B;
impl<'a> A<'a> for B {
    fn put<F>(&mut self, f: F) where F: Debug + 'static {}
}
struct C;
impl A<'static> for C {
    fn put<F>(&mut self, f: F) where F: Debug + 'static {}
}
fn main() {}
"#, &[(7, "impl has stricter requirements than trait")], "lb2");
    errors_at(r#"trait G {
    type T<'a1, 'b1>
    where
        'a1: 'b1;
}
impl G for () {
    type T<'a2, 'b2> = () where 'b2: 'a2;
}
impl G for u8 {
    type T<'a2, 'b2> = () where 'a2: 'b2;
}
fn main() {}
"#, &[(7, "impl has stricter requirements than trait")], "lb3");
}

#[test]
fn part36_moves_of_at_patterns_after_a_return() {
    // (by-move-sub-pat-unreachable.rs: the moves of `a @ (b, c)` are the borrow check's, which rustc does not give the
    // code after `return;`; its check of the pattern itself still holds there (a by-move `a @ ref b`))
    errors_at(r#"fn f() {
    return;
    struct S;
    let a @ (b, c) = (S, S);
}
fn g() {
    return;
    struct S;
    let a @ ref b = S;
}
fn main() { f(); g(); }
"#, &[(9, "borrow of moved value")], "at2");
}

#[test]
fn part36_review_of_182() {
    // (#182's review, each checked with rustc 1.97: a `'static` bound the trait's signature implies (`x: &'static T`)
    // is not stricter (rv1); a GAT's outlives that follows through the trait's own lifetime (`'a: 't, 't: 'b`) is not
    // compared (rv2); a comment in a bound is no bound (rv3); a `;` in an array type ends no item, so the fn in a
    // `const _: [(); 1]` block is checked (rv4) and one in a fn with an array parameter is hoisted (rv7); a `rest @ ..`
    // is one of the bindings of an `x @ sub` (rv5); the verdict of a method's bounds is by its impl's header too (rv6))
    errors_at(r#"trait Tr {
    fn f<T>(x: &'static T);
}
impl Tr for () {
    fn f<T: 'static>(x: &'static T) {}
}
fn main() {}
"#, &[], "rv1");
    errors_at(r#"trait Tr<'t> {
    type A<'a, 'b> where 'a: 't, 't: 'b;
}
impl<'t> Tr<'t> for () {
    type A<'a, 'b> = () where 'a: 't, 't: 'b, 'a: 'b;
}
fn main() {}
"#, &[], "rv2");
    errors_at(r#"trait G {
    type T<'a1, 'b1>
    where
        'a1: /* c */ 'b1;
}
impl G for () {
    type T<'a2, 'b2> = () where 'a2: 'b2;
}
fn main() {}
"#, &[], "rv3");
    errors_at(r#"const _: [(); 1] = {
    fn bad<'a>(x: &'a u8) -> &'static u8 { x }
    [()]
};
fn main() {}
"#, &[(2, "lifetime may not live long enough")], "rv4");
    errors_at(r#"fn f() {
    return;
    let ref mut a @ [ref b @ ..] = [1u8];
}
fn g() {
    let ref mut a @ [ref b @ ..] = [1u8];
}
fn h() {
    let ref a @ [ref b @ ..] = [1u8];
}
fn main() { f(); g(); h(); }
"#, &[(3, "cannot borrow value as immutable because it is also borrowed as mutable"), (6, "cannot borrow value as immutable because it is also borrowed as mutable")], "rv5");
    errors_at(r#"trait Tr<'a> { fn f<T: 'a>(); }
struct A;
struct B;
impl Tr<'static> for A { fn f<T: 'static>() {} }
impl<'a> Tr<'a> for B { fn f<T: 'static>() {} }
fn main() {}
"#, &[(5, "impl has stricter requirements than trait")], "rv6");
    errors_at(r#"fn foo(a: [u8; 4]) -> u8 {
    fn inner() -> u8 { 1 }
    inner() + a[0]
}
fn main() { println!("{}", foo([1, 2, 3, 4])); }
"#, &[], "rv7");
}

#[test]
fn part36_second_review_of_182() {
    // (#182's second review, checked with rustc 1.97: a `'static` in the trait's signature implies nothing of a type
    // parameter it does not name (`x: &'static u8`), so the impl's `T: 'static` is stricter (rv8); `mut b @ ..` where
    // the default binding mode is by reference binds by value before edition 2024, so it borrows nothing beside the
    // `ref mut a` around it (rv9))
    errors_at(r#"trait Tr {
    fn f<T>(x: &'static u8);
}
impl Tr for () {
    fn f<T: 'static>(x: &'static u8) {}
}
fn main() {}
"#, &[(5, "impl has stricter requirements than trait")], "rv8");
    errors_at(r#"fn main() {
    let ref mut a @ [mut b @ ..] = &[1u8];
    let _ = (a, b);
}
"#, &[], "rv9");
}

#[test]
fn part36_third_review_of_182() {
    // (#182's review of 68c598c, checked with rustc 1.97: a `'static` implies a type parameter's bound only in the
    // type of a parameter or of the result that names both: `x: T, s: &'static str` does not, so `T: 'static` is
    // stricter (rv10); `x: W<'static, T>` with `struct W<'a, T: 'a>` may, and is accepted, as in rustc (rv11))
    errors_at(r#"trait Tr {
    fn f<T>(x: T, s: &'static str);
}
impl Tr for () {
    fn f<T: 'static>(x: T, s: &'static str) {}
}
fn main() {}
"#, &[(5, "impl has stricter requirements than trait")], "rv10");
    errors_at(r#"struct W<'a, T: 'a>(&'a T);
trait Tr {
    fn f<T>(x: W<'static, T>);
}
impl Tr for () {
    fn f<T: 'static>(x: W<'static, T>) {}
}
fn main() {}
"#, &[], "rv11");
}

#[test]
fn part37_impl_where_clauses_through_the_header() {
    // (an impl's method's where clause against the trait's, read through the impl header's trait arguments, each
    // checked with rustc 1.97: the trait's `T: 'a` is the impl's `U: 'a`, so `V: 'a` is stricter (E0276 at the bound,
    // 5:23, compare-method/region-unrelated.rs), as is `'a: 'b` that nothing gives (5:24, region-extra.rs). What the
    // trait's and the impl's headers give, what follows from the trait's outlives, and what a signature's type
    // implies (`x: &'a T`) are accepted; so are a `'static` the trait's signature implies by its type (`W<'static, T>`,
    // `&'static T` beside `[u8; 1]`))
    errors_at(r#"trait Master<'a, T, U> { fn foo() where T: 'a; }
impl<'a, U, V> Master<'a, U, V> for () { fn foo() where U: 'a {} }
trait Tr2<'a, 'b: 'a> { fn foo(); }
impl<'a, 'b: 'a> Tr2<'a, 'b> for () { fn foo() where 'b: 'a {} }
trait Tr3<'a, T> { fn foo(x: &'a T); }
impl<'a, T> Tr3<'a, T> for () { fn foo(x: &'a T) where T: 'a {} }
trait Tr4<'a> { fn foo() where Self: 'a; }
impl<'a, T> Tr4<'a> for Vec<T> { fn foo() where Vec<T>: 'a {} }
trait Tr5<'a, T> { fn foo(); }
impl<'a, T: 'a> Tr5<'a, T> for () { fn foo() where T: 'a {} }
trait Tr7<'a, 'b, 'c> { fn foo() where 'a: 'b, 'b: 'c; }
impl<'x, 'y, 'z> Tr7<'x, 'y, 'z> for () { fn foo() where 'x: 'z {} }
trait Tr8<'a> { fn foo(); }
impl<'a> Tr8<'a> for &'a u8 { fn foo() where 'a: 'a {} }
trait Tr11<'a, 'b> { fn foo(x: &'b &'a u8); }
impl<'a, 'b> Tr11<'a, 'b> for () { fn foo(x: &'b &'a u8) where 'a: 'b {} }
trait S4 { fn f<T>(x: W<'static, T>); }
struct W<'a, T: 'a>(&'a T);
impl S4 for () { fn f<T: 'static>(x: W<'static, T>) {} }
trait S5 { fn f<T>(a: [u8; 1], x: &'static T); }
impl S5 for () { fn f<T: 'static>(a: [u8; 1], x: &'static T) {} }
fn main() {}
"#, &[], "iw1");
    errors_at(r#"trait Master<'a, T: ?Sized, U> {
    fn foo() where T: 'a;
}
impl<'a, U, V> Master<'a, U, V> for () {
    fn foo() where V: 'a { }
}
fn main() {}
"#, &[(5, "impl has stricter requirements than trait")], "iw2");
    errors_at(r#"trait Master<'a, 'b> {
    fn foo();
}
impl<'a, 'b> Master<'a, 'b> for () {
    fn foo() where 'a: 'b { }
}
fn main() {}
"#, &[(5, "impl has stricter requirements than trait")], "iw3");
    // (#182's last finding: `(T, &'static str)` implies nothing of T, so `T: 'static` is stricter, 5:13)
    errors_at(r#"trait Tr {
    fn f<T>(x: (T, &'static str));
}
impl Tr for () {
    fn f<T: 'static>(x: (T, &'static str)) {}
}
fn main() {}
"#, &[(5, "impl has stricter requirements than trait")], "iw4");
}

#[test]
fn part37_closures_by_reference_and_moved_fields() {
    // (checked with rustc 1.97: `&func` is an `Fn` only where func is, so an `FnMut` closure passed by reference where
    // `Fn` is wanted is E0525 at the closure (5:20, closures/fnmut-shared-reference-requires-fn.rs); `&mut func` where
    // `FnMut` is wanted and `&g` of an `Fn` closure are accepted, as is a move closure that moves a field of what it
    // captures out (`let t = s.1;`), which was "not supported")
    errors_at(r#"fn requires_fn<F: Fn()>(_: F) {}
fn requires_fnmut<F: FnMut()>(_: F) {}
fn a() {
    let mut value = 0;
    let mut func = || value += 1;
    requires_fn(&func);
}
fn b() {
    let mut value = 0;
    let mut func = || value += 1;
    requires_fnmut(&mut func);
    requires_fnmut(&mut func);
}
fn c() {
    let s = String::new();
    let g = || drop(&s);
    requires_fn(&g);
}
struct S(String, String);
fn d() {
    let s = S(String::new(), String::new());
    let c = move || {
        let t = s.1;
        drop(t);
    };
    c();
}
fn main() { a(); b(); c(); d(); }
"#, &[(5, "expected a closure that implements the `Fn` trait, but this closure only implements `FnMut`")], "cl1");
}

#[test]
fn part37_mut_rest_bindings_by_value() {
    // (before edition 2024, `mut rest @ ..` where the default binding mode is by reference binds by value: an array
    // of the elements between (`let [mut b @ ..] = &[1u8];` makes b a `[u8; 1]`), as rustc 1.97 does; a `ref` rest
    // stays a slice)
    errors_at(r#"fn main() {
    let [mut b @ ..] = &[1u8];
    let _: [u8; 1] = b;
    let [x, mut rest @ .., y] = &[1u8, 2, 3, 4];
    rest[0] = 9;
    println!("{} {:?} {}", x, rest, y);
    let arr = [10u32, 20, 30];
    if let [_, ref tail @ ..] = arr {
        println!("{:?}", tail);
    }
    let r = &[5i32, 6];
    let [ref a @ ..] = r;
    println!("{:?}", a);
}
"#, &[], "rb1");
}

#[test]
fn part37_where_clauses_with_the_parts_of_a_type() {
    // (the first cycle's regression, traits/elaborate-type-region.rs (run-pass): the trait's `T: 'a` is the impl's
    // `[U]: 'a`, `&'b U: 'a`, `Bar<'b, T>: 'a` or `fn(T): 'a`, which give what their parts outlive (RFC 1214); a
    // projection's (`U::Item: 'a`) gives nothing of U. And the impl's `Self` is its self type. Each as rustc 1.97)
    errors_at(r#"trait Master<'a, T: ?Sized> {
    fn foo() where T: 'a;
}
impl<'a, U> Master<'a, [U]> for () {
    fn foo() where U: 'a { }
}
impl<'a, 'b, U> Master<'a, &'b U> for () {
    fn foo() where 'b: 'a, U: 'a { }
}
struct Bar<'a, T: 'a> { x: &'a T }
impl<'a, 'b, T> Master<'a, Bar<'b, T>> for () {
    fn foo() where 'b: 'a, T: 'a { }
}
impl<'a, T> Master<'a, fn(T)> for () {
    fn foo() where T: 'a { }
}
trait Leak<'a> { fn leak() where Self: 'a; }
struct D<'a>(&'a i32);
impl<'a, 'b> Leak<'a> for D<'b> { fn leak() where Self: 'a {} }
fn main() {}
"#, &[], "ep1");
}

#[test]
fn part37_review_impl_where_clauses() {
    // (#221's review, each checked with rustc 1.97. Accepted: a trait's method whose array type holds a `;`, a method
    // named `f` with its own parameter, an impl's `where` on its own line, nested references (`&'b &'a u8` gives
    // `'a: 'b`), what a signature's type gives through `Self`, a defaulted trait argument left out, and `&F` where the
    // bound is on F)
    errors_at(r#"trait Tr<'a, T> { fn run(_: [u8; 1]) where T: 'a; }
impl<'a, T> Tr<'a, T> for () { fn run(_: [u8; 1]) where T: 'a {} }
trait Tf<'a> { fn f<T>() where T: 'a; }
impl<'a> Tf<'a> for () { fn f<U>() where U: 'a {} }
trait Tw<'a> { fn f() where Self: 'a; }
struct W<T>(T);
impl<'a, T> Tw<'a> for W<T>
where T: Copy {
    fn f() where W<T>: 'a {}
}
trait Tn<'a, 'b> { fn run(_: &'b &'a u8); }
impl<'a, 'b> Tn<'a, 'b> for () { fn run(_: &'b &'a u8) where 'a: 'b {} }
trait Ts<'a> { fn foo(x: &'a Self); }
impl<'a, T> Ts<'a> for W<T> { fn foo(x: &'a W<T>) where T: 'a {} }
trait Td<'a, T = ()> { fn run() where T: 'a; }
impl<'a, U> Td<'a> for W<U> { fn run() where (): 'a {} }
fn inspect<F: FnMut()>(_: &F) {}
fn main() {
    let mut n = 0;
    let c = || n += 1;
    inspect(&c);
}
"#, &[], "rw1");
    // (stricter: the impl header's types give nothing here (`for &'a T`, `Tr<'a, &'a T>`), two parts of a tuple give
    // no order of their lifetimes, a trait's method's bound of its own parameter is not the impl's of that name, and a
    // defaulted argument left out is the default; at the failing predicate's bound, on the line an impl in a fn body
    // is written on)
    let e = "impl has stricter requirements than trait";
    errors_at("trait Tr8<'a> { fn foo(); }\nimpl<'a, T> Tr8<'a> for &'a T { fn foo() where T: 'a {} }\nfn main() {}\n", &[(2, e)], "rw2");
    errors_at("trait Tr<'a, X> { fn foo(); }\nimpl<'a, T> Tr<'a, &'a T> for () { fn foo() where T: 'a {} }\nfn main() {}\n", &[(2, e)], "rw3");
    errors_at("trait Tr<'a, 'b> { fn run(_: (&'a u8, &'b u8)); }\nimpl<'a, 'b> Tr<'a, 'b> for () { fn run(_: (&'a u8, &'b u8)) where 'a: 'b {} }\nfn main() {}\n", &[(2, e)], "rw4");
    errors_at("struct W<U>(U);\ntrait Tr<'a> { fn run<U>() where U: 'a; }\nimpl<'a, U> Tr<'a> for W<U> { fn run<V>() where V: 'a, U: 'a {} }\nfn main() {}\n", &[(3, e)], "rw5");
    errors_at("struct W<U>(U);\ntrait Tr<'a, T = ()> { fn run(); }\nimpl<'a, U> Tr<'a> for W<U> { fn run() where U: 'a {} }\nfn main() {}\n", &[(3, e)], "rw6");
    errors_at("trait Tr<'a, T> { fn f() where T: 'a; }\nimpl<'a, T, U> Tr<'a, T> for (U,) {\n    fn f() where T: 'a,\n        U: 'a {}\n}\nfn main() {}\n", &[(4, e)], "rw7");
    errors_at("fn main() {\n    trait Tr<'a, T> { fn f() where T: 'a; }\n    impl<'a, T, U> Tr<'a, T> for (U,) {\n        fn f() where U: 'a {}\n    }\n}\n", &[(4, e)], "rw8");
    // (`&c` where the parameter is `&F` and F: Fn: c, which mutates what it captures, is only an FnMut (E0525 at the
    // closure, 4:13))
    errors_at("fn inspect<F: Fn()>(_: &F) {}\nfn main() {\n    let mut n = 0;\n    let c = || n += 1;\n    inspect(&c);\n}\n", &[(4, "expected a closure that implements the `Fn` trait")], "rw9");
}

#[test]
fn part37_review_impl_where_clauses_2() {
    // (#221's second review, each checked with rustc 1.97. Accepted: a `b']'` in an array length before the
    // declaration's `;`, a `:` in a default (`T = fn(x: u8)`), a bound that holds for every lifetime (`fn(u8): 'a`))
    errors_at(r#"trait Tr<'a, T> { fn f(_: [u8; b']' as usize]) where T: 'a; }
impl<'a, T> Tr<'a, T> for () { fn f(_: [u8; b']' as usize]) where T: 'a {} }
trait Td<'a, T = fn(x: u8)> { fn f() where T: 'a; }
impl<'a, U> Td<'a, U> for () { fn f() where U: 'a {} }
trait Tz<'a, T = fn(x: u8)> { fn f(); }
impl<'a> Tz<'a> for () { fn f() where fn(u8): 'a {} }
fn main() {}
"#, &[], "rw10");
    // (stricter: `&'a <T as Has>::Item` gives nothing of T, the trait's method's own `T` is not the impl header's,
    // and a header whose `for` starts a line is compared too)
    let e = "impl has stricter requirements than trait";
    errors_at("trait Has { type Item; }\ntrait Tr<'a, T: Has> { fn f(_: &'a <T as Has>::Item); }\nimpl<'a, T: Has> Tr<'a, T> for () { fn f(_: &'a <T as Has>::Item) where T: 'a {} }\nfn main() {}\n", &[(3, e)], "rw11");
    errors_at("trait Tr<'a> { fn f<T>(_: &'a T); }\nimpl<'a, T> Tr<'a> for (T,) { fn f<U>(_: &'a U) where T: 'a {} }\nfn main() {}\n", &[(2, e)], "rw12");
    errors_at("trait Tr<'a, T> { fn f(); }\nimpl<'a, T> Tr<'a, T>\nfor () { fn f() where T: 'a {} }\nfn main() {}\n", &[(3, e)], "rw13");
    errors_at("trait Tr<'a, T> { fn f(); }\nimpl<'a, T> Tr<'a, for<'b> fn(&'b T)>\nfor () { fn f() where T: 'a {} }\nfn main() {}\n", &[(3, e)], "rw14");
}

#[test]
fn part37_review_impl_where_clauses_3() {
    // (#221's third review, each checked with rustc 1.97. Accepted: a `b'['` in an array length before the method's
    // body, and a bound that a signature's type gives with the header's outlives (`x: &'b T` and `'b: 'a` give
    // `T: 'a`))
    errors_at(r#"trait Tr<'a, T> {
    fn f(_: [u8; b'[' as usize]) where T: 'a;
}
impl<'a, T> Tr<'a, T> for () {
    fn f(_: [u8; b'[' as usize]) where T: 'a {}
}
trait Tb<'a, 'b: 'a, T> {
    fn f(_: &'b T);
}
impl<'a, 'b: 'a, T> Tb<'a, 'b, T> for () {
    fn f(_: &'b T) where T: 'a {}
}
fn main() {}
"#, &[], "rw15");
    // (stricter: a default with a `b'>'` in it is read as the default, so the impl's `U: 'a` is compared, 7:21)
    errors_at(r#"trait Has<A> {}
impl Has<[u8; b'>' as usize]> for () {}
trait Tr<'a, T: Has<[u8; b'>' as usize]> = ()> {
    fn f();
}
impl<'a, U> Tr<'a> for (U,) {
    fn f() where U: 'a {}
}
fn main() {}
"#, &[(7, "impl has stricter requirements than trait")], "rw16");
}

#[test]
fn part38_methods_of_impls_for_trait_objects() {
    // (#169: methods of `impl dyn Tr { .. }` and of a trait's impl for the object, `impl X for dyn Tr`, found through
    // the object and called with it as `&self`; each checked with rustc 1.97. A header's object not behind a reference
    // is `dyn Tr + 'static` (a written `+ '_` is not), so the call needs the receiver's object to be 'static, rustc's
    // E0521: none here, where it is or the impl says `'_`)
    errors_at(r#"trait Tr { fn name(&self) -> u8; }
impl dyn Tr { fn m(&self) -> u8 { self.name() + 1 } }
struct S;
impl Tr for S { fn name(&self) -> u8 { 1 } }
trait ObjectTrait {}
trait MyTrait { fn use_self(&self) -> &() { panic!() } }
impl MyTrait for dyn ObjectTrait + '_ {}
trait Other { fn other(&self) -> &(); }
impl Other for dyn ObjectTrait + '_ { fn other(&self) -> &() { panic!() } }
fn a<'a>(val: &'a dyn ObjectTrait) -> &'a () { val.use_self() }
fn b<'a>(val: &'a dyn ObjectTrait) -> &'a () { val.other() }
fn c<'a>(val: &'a (dyn Tr + 'static)) -> u8 { val.m() }
fn main() {
    let r: &dyn Tr = &S;
    let x: Box<dyn Tr> = Box::new(S);
    let _ = (r.m(), (*x).m(), x.m());
}
"#, &[], "dy1");
    let e = "lifetime may not live long enough";
    // (an inherent method of `impl dyn ObjectTrait`, the trait's method for the object, its default, and the default
    // named as `MyTrait::use_self(val)`: each requires 'a: 'static)
    errors_at("trait ObjectTrait {}\nimpl dyn ObjectTrait { fn use_self(&self) -> &() { panic!() } }\nfn use_it<'a>(val: &'a dyn ObjectTrait) -> &'a () {\n    val.use_self()\n}\nfn main() {}\n", &[(4, e)], "dy2");
    errors_at("trait ObjectTrait {}\ntrait MyTrait { fn use_self(&self) -> &(); }\nimpl MyTrait for dyn ObjectTrait { fn use_self(&self) -> &() { panic!() } }\nfn use_it(val: &dyn ObjectTrait) -> &() {\n    val.use_self()\n}\nfn main() {}\n", &[(5, e)], "dy3");
    errors_at("trait ObjectTrait {}\ntrait MyTrait { fn use_self(&self) -> &() { panic!() } }\nimpl MyTrait for dyn ObjectTrait {}\nfn use_it<'a>(val: &'a dyn ObjectTrait) -> &'a () {\n    val.use_self()\n}\nfn main() {}\n", &[(5, e)], "dy4");
    errors_at("trait ObjectTrait {}\ntrait MyTrait { fn use_self(&self) -> &() { panic!() } }\nimpl MyTrait for dyn ObjectTrait {}\nfn use_it<'a>(val: &'a dyn ObjectTrait) -> &'a () {\n    MyTrait::use_self(val)\n}\nfn main() {}\n", &[(5, e)], "dy5");
    // (a generic impl for an object of a generic trait, its parameter from the object's arguments, and a method's
    // turbofish)
    errors_at("trait ObjectTrait<T> {}\ntrait MyTrait<T> { fn use_self<K>(&self) -> &(); }\nimpl<T> MyTrait<T> for dyn ObjectTrait<T> { fn use_self<K>(&self) -> &() { panic!() } }\nfn use_it<'a>(val: &'a dyn ObjectTrait<u8>) -> &'a () {\n    val.use_self::<u16>()\n}\nfn main() {}\n", &[(5, e)], "dy6");
    // (an `impl Trait` result whose type is such a method's: `&'a ()` for `impl OtherTrait<'a> + 'a`)
    errors_at("trait OtherTrait<'a> {}\nimpl<'a> OtherTrait<'a> for &'a () {}\ntrait ObjectTrait {}\nimpl dyn ObjectTrait { fn use_self(&self) -> &() { panic!() } }\nfn use_it<'a>(val: &'a dyn ObjectTrait) -> impl OtherTrait<'a> + 'a {\n    val.use_self()\n}\nfn main() {}\n", &[(6, e)], "dy7");
    // (two traits whose defaults of the name the object has: ambiguous, E0034)
    errors_at("trait ObjectTrait {}\ntrait A { fn f(&self) {} }\ntrait B { fn f(&self) {} }\nimpl A for dyn ObjectTrait + '_ {}\nimpl B for dyn ObjectTrait + '_ {}\nfn g(val: &dyn ObjectTrait) {\n    val.f()\n}\nfn main() {}\n", &[(7, "multiple applicable items in scope")], "dy8");
}

#[test]
fn part38_review_methods_of_impls_for_trait_objects() {
    // (#237's review, each checked with rustc 1.97. Accepted: an object whose trait takes a lifetime (`trait Obj<'a>:
    // 'a`, the default its own bound), an impl for `dyn Obj<u8> + '_` (the object's arguments in the lookup), and
    // `Ext::pick(x)` beside the object's own `pick`, which picks Ext's)
    errors_at(r#"trait Obj<'a>: 'a {}
impl<'a> dyn Obj<'a> { fn get(&self) -> u8 { 1 } }
fn f<'a>(x: &'a dyn Obj<'a>) -> u8 { x.get() }
trait O2<T> {}
trait Ext { fn e(&self) -> u8 { 1 } }
impl Ext for dyn O2<u8> + '_ {}
fn g<'a>(x: &'a dyn O2<u8>) -> u8 { x.e() }
trait O3 { fn pick(&self) -> u8 { 1 } }
trait E3 { fn pick(&self) -> u8 { 2 } }
impl E3 for dyn O3 {}
fn h(x: &(dyn O3 + 'static)) -> u8 { E3::pick(x) }
fn main() {}
"#, &[], "rv1");
    // (rejected: a `&self` result borrowed from the receiver, not only its object (3:50); an unsafe method outside
    // `unsafe` (E0133); a `&mut self` method through `&` (E0596); a local Box's borrow returned (E0515))
    errors_at("trait Tr {}\nimpl dyn Tr { fn get(&self) -> &u8 { panic!() } }\nfn leak(x: &(dyn Tr + 'static)) -> &'static u8 { x.get() }\nfn main() {}\n", &[(3, "lifetime may not live long enough")], "rv2");
    errors_at("trait Obj {}\nimpl dyn Obj { unsafe fn dangerous(&self) -> u8 { 1 } }\nfn f(x: &(dyn Obj + 'static)) -> u8 { x.dangerous() }\nfn main() {}\n", &[(3, "E0133")], "rv3");
    errors_at("trait Obj {}\nimpl dyn Obj { fn change(&mut self) {} }\nfn f(x: &(dyn Obj + 'static)) { x.change() }\nfn main() {}\n", &[(3, "E0596")], "rv4");
    errors_at("trait Tr {}\nimpl dyn Tr { fn get<'a>(&'a self) -> &'a u8 { panic!() } }\nstruct S;\nimpl Tr for S {}\nfn g() -> &'static u8 { let b: Box<dyn Tr> = Box::new(S); b.get() }\nfn main() {}\n", &[(5, "E0515")], "rv5");
    // (an explicit deref of the Box (`(*b).get()`), through `impl dyn Tr` and through the vtable: its content
    // borrowed as `b.get()`'s (E0515); of `b: &Box<dyn Tr>` or a field's Box through `&W` a reborrow, not a loan; and
    // `(*x).get()` of `x: &'a (dyn Tr + 'static)` borrowed for `'a`, not `'static` (9:54))
    errors_at(r#"trait Tr {}
impl dyn Tr { fn get<'a>(&'a self) -> &'a u8 { panic!() } }
struct S; impl Tr for S {}
fn f() -> &'static u8 { let b: Box<dyn Tr> = Box::new(S); (*b).get() }
fn g(b: &Box<dyn Tr>) -> &u8 { (*b).get() }
fn h(b: &Box<dyn Tr>) -> &u8 { (**b).get() }
struct W { b: Box<dyn Tr> }
fn k(w: &W) -> &u8 { (*w.b).get() }
fn l<'a>(x: &'a (dyn Tr + 'static)) -> &'static u8 { (*x).get() }
fn main() {}
"#, &[(4, "E0515"), (9, "lifetime may not live long enough")], "rv6");
    errors_at(r#"trait Tr { fn get<'a>(&'a self) -> &'a u8; }
fn f(b: Box<dyn Tr>) -> &'static u8 { (*b).get() }
fn g(b: &Box<dyn Tr>) -> &u8 { (*b).get() }
fn h(b: &Box<dyn Tr>) -> &u8 { (**b).get() }
fn main() {}
"#, &[(2, "E0515")], "rv7");
    // (#237's second review, accepted by rustc 1.97: an object among a user type's arguments takes that type's bound
    // (`impl<'a> W<'a, dyn Obj>` with `T: ?Sized + 'a`), not `'static`; a result type solved from the type expected
    // and, in an `impl Trait` body's inference, from the object's arguments)
    errors_at(r#"struct W<'a, T: ?Sized + 'a>(&'a T);
trait Obj {}
impl<'a> W<'a, dyn Obj> { fn get(&self) -> u8 { 1 } }
fn f<'a>(w: W<'a, dyn Obj + 'a>) -> u8 { w.get() }
trait Tr {}
impl dyn Tr { fn make<T: Default>(&self) -> T { T::default() } }
fn g(x: &(dyn Tr + 'static)) -> u8 { x.make() }
trait O<T> {}
trait Get<T> { fn get(&self) -> T; }
impl<T: Default> Get<T> for dyn O<T> + '_ { fn get(&self) -> T { T::default() } }
fn h(x: &dyn O<u8>) -> impl std::fmt::Display { x.get() }
fn main() {}
"#, &[], "rv8");
    // (the third review's, accepted by rustc 1.97: a user type named as a std one (`RwLock<'a, T: ?Sized + 'a>`) gives
    // its object its bound, not `'static`; a method's turbofish in an `impl Trait` body's inference)
    errors_at(r#"struct RwLock<'a, T: ?Sized + 'a>(&'a T);
trait Obj {}
impl<'a> RwLock<'a, dyn Obj> { fn get(&self) -> u8 { 1 } }
fn f<'a>(w: RwLock<'a, dyn Obj + 'a>) -> u8 { w.get() }
impl dyn Obj { fn make<T: Default>(&self) -> T { T::default() } }
fn g(x: &(dyn Obj + 'static)) -> impl std::fmt::Display { x.make::<u8>() }
fn main() {}
"#, &[], "rv9");
    // (the fourth review's, checked with rustc 1.97: a default's trait parameters from the impl's trait arguments
    // (`impl Ext<u8> for dyn Obj + '_`, `x.value()` with no annotation); an object that writes `'static` asks for it,
    // as the default does (4:41, E0521 for rustc))
    errors_at(r#"trait Obj {}
trait Ext<T> { fn value(&self) -> Option<T> { None } }
impl Ext<u8> for dyn Obj + '_ {}
fn f(x: &dyn Obj) -> Option<u8> { let v = x.value(); v }
trait Obj2<T> {}
trait Ext2<T> { fn get(&self) -> Vec<T> { Vec::new() } }
impl<T> Ext2<T> for dyn Obj2<T> + '_ {}
fn g(y: &dyn Obj2<u16>) -> usize { let w = y.get(); w.len() }
fn main() {}
"#, &[], "rv10");
    errors_at("trait Obj {}\ntrait Ext { fn e(&self) -> u8 { 1 } }\nimpl Ext for dyn Obj + 'static {}\nfn f<'a>(x: &'a (dyn Obj + 'a)) -> u8 { x.e() }\nfn main() {}\n", &[(4, "lifetime may not live long enough")], "rv11");
}

#[test]
fn part39_dyn_error_is_deferred_lets_closures_as_fn_pointers_trait_results() {
    // (each checked with rustc 1.97. `<dyn Error + 'static>::is` asks for a `'static` object (E0521 for rustc), and
    // accepts one)
    errors_at("use std::error::Error;\nfn err_is<T: Error + 'static>(err: &dyn Error) -> bool {\n    err.is::<T>()\n}\n#[derive(Debug)]\nstruct A;\nimpl std::fmt::Display for A { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, \"A\") } }\nimpl Error for A {}\nfn ok(e: &(dyn Error + 'static)) -> bool { e.is::<A>() }\nfn main() {}\n", &[(3, "lifetime may not live long enough")], "p1");
    // (a `let` assigned later through an overloaded deref: its type is the target's, and the borrow of `b` escapes
    // its block (E0597))
    errors_at(r#"use std::cell::RefCell;
fn main() {
    let m = RefCell::new(0);
    let p;
    {
        let b = m.borrow();
        p = &*b;
    }
    let _ = *p;
}
"#, &[(7, "does not live long enough")], "p2");
    // (a closure held as `fn(_, _) -> _`: its parameters from how the pointer is used, and its value, the second
    // parameter, flows into its result: returned as `fn(&'a u32, &'b u32) -> &'a u32`, `'b: 'a` (3:5))
    errors_at("fn bar<'a, 'b>() -> fn(&'a u32, &'b u32) -> &'a u32 {\n    let g: fn(_, _) -> _ = |_x, y| y;\n    g\n}\nfn main() {}\n", &[(3, "lifetime may not live long enough")], "p3");
    errors_at(r#"fn bar<'a, 'b>() -> fn(&'a u32, &'b u32) -> &'a u32 {
    let g: fn(_, _) -> _ = |x, _y| x;
    g
}
fn baz<'a>() -> fn(&'a u32, &'a u32) -> &'a u32 {
    let g: fn(_, _) -> _ = |_x, y| y;
    g
}
fn quux<'a, 'b: 'a>() -> fn(&'a u32, &'b u32) -> &'a u32 {
    let g: fn(_, _) -> _ = |_x, y| y;
    g
}
fn main() {}
"#, &[], "p4");
    // (a trait method's elided result is the receiver's borrow: through the vtable (a local Box's returned, E0515),
    // and a default through `impl Ext for dyn Tr {}` returned as `&'static` (4:50))
    errors_at("trait Tr { fn get(&self) -> &u8; }\nfn f() -> &'static u8 { let b: Box<dyn Tr> = Box::new(S(1)); b.get() }\nstruct S(u8); impl Tr for S { fn get(&self) -> &u8 { &self.0 } }\nfn main() {}\n", &[(2, "E0515")], "p5");
    errors_at("trait Tr {}\ntrait Ext { fn get(&self) -> &u8 { &0 } }\nimpl Ext for dyn Tr {}\nfn leak(x: &(dyn Tr + 'static)) -> &'static u8 { x.get() }\nfn main() {}\n", &[(4, "lifetime may not live long enough")], "p6");
    errors_at("trait Tr { fn get(&self) -> &u8; }\nfn h(t: &dyn Tr) -> &u8 { t.get() }\nfn k(t: &(dyn Tr + 'static)) -> &'static u8 { t.get() }\nfn main() {}\n", &[(3, "lifetime may not live long enough")], "p7");
}

#[test]
fn part40_static_bound_constructors_called_generic_fns_elided_self_types() {
    // (each checked with rustc 1.97. A tuple struct's or an enum's value built in a generic fn, of a type that bounds
    // its parameter by 'static: the type is well formed only with T: 'static (E0310))
    errors_at("struct St<T: 'static>(Option<T>);\nfn h1<T>(x: T) {\n    let _ = St(Some(x));\n}\nfn main() {}\n", &[(3, "may not live long enough")], "q1");
    errors_at("enum En<T: 'static> { A(T), B }\nfn h3<T>(x: T) {\n    let _ = En::A(x);\n}\nfn main() {}\n", &[(3, "may not live long enough")], "q2");
    errors_at("enum En<T: 'static> { A(T), B }\nfn h4<T>() {\n    let _ = En::<T>::B;\n}\nfn main() {}\n", &[(3, "may not live long enough")], "q3");
    errors_at("struct St<T: 'static>(Option<T>);\nfn g5<T>(x: T) {\n    let _ = St::<T>(Some(x));\n}\nfn main() {}\n", &[(3, "may not live long enough")], "q4");
    // (and where the parameter is 'static: by its bound, by an impl's, by a type of the signature)
    errors_at(r#"use std::marker::PhantomData;
struct St<T: 'static>(Option<T>);
enum En<T: 'static> { A(T), B }
struct Ph<T: 'static>(PhantomData<T>);
impl<T: 'static> St<T> {
    fn new(x: T) -> Self { Self(Some(x)) }
}
fn f2<T: 'static>(x: T) -> En<T> {
    if true { En::A(x) } else { En::<T>::B }
}
fn f3<T>(s: St<T>) -> usize {
    let f = || { let _ = Ph::<T>(PhantomData); };
    f();
    s.0.is_some() as usize
}
fn main() {
    let _ = St::new(1u8);
    let _ = f2(2u8);
    let _ = f3(St(Some(3u8)));
}
"#, &[], "q5");
    // (a generic fn that is called is checked with placeholders too when its body names such a type: an instance
    // does not tell, with `&u8` or `u8` for T)
    errors_at("struct St<T: 'static>(Option<T>);\nfn g4<T>() {\n    let _: Option<St<T>> = None;\n}\nfn main() {\n    g4::<&u8>();\n    g4::<u8>();\n}\n", &[(3, "may not live long enough")], "q6");
    errors_at(r#"use std::cell::OnceCell;
use std::fmt::Display;
use std::marker::PhantomData;
use std::rc::Rc;
type Storage = Rc<OnceCell<Box<dyn Display + 'static>>>;
struct Inner<T: Display + 'static>(PhantomData<T>);
struct Outer<T, U> { input: Option<T>, output: Storage, _p: PhantomData<U> }
fn extend<T: Display>(x: T) -> Box<dyn Display + 'static> {
    let storage = Rc::new(OnceCell::new());
    {
        let _ = Outer::<T, Inner<T>> { input: Some(x), output: storage.clone(), _p: PhantomData };
    }
    Rc::into_inner(storage).unwrap().into_inner().unwrap()
}
fn main() {
    let data = String::from("abc");
    let _ = extend::<&String>(&data);
}
"#, &[(11, "may not live long enough")], "q7");
    // (OnceCell is not Sync)
    errors_at("fn require_sync<T: Sync>() {}\nfn main() {\n    require_sync::<std::cell::OnceCell<()>>();\n}\n", &[(3, "Sync")], "q8");
    // (a receiver that writes the impl's self type with an elided lifetime: one of the method's own, not the impl's
    // (E0308))
    errors_at("struct Pair<T, V>(T, V);\nimpl Pair<&str, isize> {\n    fn say(self: &Pair<&str, isize>) {}\n}\nfn main() {}\n", &[(3, "mismatched `self` parameter type")], "q9");
    errors_at("struct Foo<'a>(&'a ());\nimpl<'a> Foo<'a> {\n    fn f(self: Foo<'_>) {}\n}\nfn main() {}\n", &[(3, "mismatched `self` parameter type")], "q10");
    errors_at("struct Foo<'a>(&'a ());\nimpl<'a> Foo<'a> {\n    fn f(self: Foo) {}\n}\nfn main() {}\n", &[(3, "mismatched `self` parameter type")], "q11");
    errors_at("use std::rc::Rc;\nstruct W<T>(T);\ntrait Tr { fn f(self: Rc<Self>); }\nimpl Tr for W<&u8> {\n    fn f(self: Rc<W<&u8>>) {}\n}\nfn main() {}\n", &[(5, "mismatched `self` parameter type")], "q12");
    errors_at(r#"struct Pair<T, V>(T, V);
impl<'a> Pair<&'a str, isize> {
    fn say(self: &Pair<&'a str, isize>) {}
}
struct W<T>(T);
impl W<fn(&u8)> {
    fn f(self: &W<fn(&u8)>) {}
}
impl W<Box<dyn Fn(&u8)>> {
    fn g(self: &W<Box<dyn Fn(&u8)>>) {}
}
struct Foo<'a>(&'a ());
impl<'a> Foo<'a> {
    fn f(self: &Self) {}
    fn g(self: Box<Self>) {}
    fn h(&self) -> Foo<'_> { Foo(&()) }
}
impl W<&'static u8> {
    fn k(self: &W<&'static u8>) {}
}
fn main() {}
"#, &[], "q13");
    // (the review's: an impl lifetime that the self type does not have (`impl<'a> S`) is not the receiver's; a
    // constructor in a returned position is checked too; `St<Option<T>>` in the signature implies T: 'static in a
    // closure; a closure of a fn whose T is 'static builds St and En of it)
    errors_at("struct S;\nimpl<'a> S {\n    fn f(self: S, _: &'a u8) {}\n}\nfn main() { S.f(&1); }\n", &[], "q14");
    errors_at("struct S<T: 'static>(T);\nfn bad<T>(x: T) -> impl Sized {\n    return S(x);\n}\nfn main() {}\n", &[(3, "may not live long enough")], "q15");
    errors_at(r#"struct St<U: 'static>(U);
fn needs_static<T: 'static>() {}
fn f<T>(_: St<Option<T>>) {
    let c = || needs_static::<T>();
    c();
}
fn main() { f::<u8>(St(None)); }
"#, &[], "q16");
    errors_at(r#"struct St<T: 'static>(T);
enum En<T: 'static> { A(T), B }
fn c<T: 'static + Copy>(x: T) -> usize {
    let f = move || {
        let _ = St(x);
        En::A(x)
    };
    let _ = f();
    1
}
fn main() { let _ = c(3u8); let _ = En::<u8>::B; }
"#, &[], "q17");
}

#[test]
fn part41_drop_impls_specialized_or_requiring_more_ref_bindings_of_owned_places() {
    // (each checked with rustc 1.97. A Drop impl's self type writes the impl's own parameters, each once (E0366))
    errors_at("struct P<T>(*const T);\nimpl Drop for P<i8> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "cannot be specialized (E0366)")], "r1");
    errors_at("struct N<'n>(&'n i8);\nimpl Drop for N<'static> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "cannot be specialized (E0366)")], "r2");
    errors_at("struct V<A, B>(*const A, *const B);\nimpl<One> Drop for V<One, One> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "cannot be specialized (E0366)")], "r3");
    errors_at("struct W<'a, 'b>(&'a i8, &'b u8);\nimpl<'l> Drop for W<'l, 'l> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "cannot be specialized (E0366)")], "r4");
    errors_at("struct X<const C: usize>;\nimpl Drop for X<3> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "cannot be specialized (E0366)")], "r5");
    // (and requires of them only what the definition does, or its fields imply (E0367))
    errors_at("struct K<'a, 'b>(&'a i8, &'b u8);\nimpl<'a, 'b: 'a> Drop for K<'a, 'b> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "requires `'b: 'a`")], "r6");
    errors_at("struct L<'a, 'b>(&'a i8, &'b u8);\nimpl<'a, 'b> Drop for L<'a, 'b>\nwhere\n    'b: 'a,\n{\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "requires `'b: 'a`")], "r7");
    errors_at("trait Bound {}\nstruct Q<T>(*const T);\nimpl<T: Bound> Drop for Q<T> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(3, "requires `T: Bound` but the struct")], "r8");
    errors_at("struct Wrapper<'a, T>(&'a T)\nwhere\n    T: 'a;\nimpl<'a, T> Drop for Wrapper<'a, T>\nwhere\n    T: 'static,\n{\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(6, "requires `T: 'static`")], "r9");
    errors_at("struct G<T: ?Sized>(*const T);\nimpl<T> Drop for G<T> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(2, "requires `T: Sized`")], "r10");
    errors_at("trait Tr { type Assoc; }\nstruct Foo<T: Tr, U: ?Sized>(T, U);\nimpl<T: Tr<Assoc = U>, U: ?Sized> Drop for Foo<T, U> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(3, "requires `<T as Tr>::Assoc == U`")], "r11");
    errors_at("trait Bound {}\nenum E<T> { A(T) }\nimpl<T: Bound> Drop for E<T> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(3, "but the enum it is implemented for")], "r12");
    errors_at("trait Bound {}\nunion U<T: Copy> { f: T }\nimpl<T: Copy + Bound> Drop for U<T> {\n    fn drop(&mut self) {}\n}\nfn main() {}\n", &[(3, "but the union it is implemented for")], "r13");
    // (what the definition gives: its bounds, supertraits, the outlives its fields imply, transitively)
    errors_at(r#"use std::fmt::{Debug, Display};
struct A<T>(T);
impl<T> Drop for A<T> { fn drop(&mut self) {} }
struct B<'a, T: ?Sized + 'a>(&'a T);
impl<'a, T: ?Sized> Drop for B<'a, T> { fn drop(&mut self) {} }
struct D<T: Ord>(T);
impl<T: Ord + PartialOrd + PartialEq> Drop for D<T> { fn drop(&mut self) {} }
struct E<T> where T: Clone + Debug { t: T }
impl<T: Clone + Debug> Drop for E<T> where T: Debug { fn drop(&mut self) {} }
struct G<'a, 'b: 'a, T: 'b>(&'a &'b T);
impl<'x, 'y: 'x, U: 'y> Drop for G<'x, 'y, U> { fn drop(&mut self) {} }
struct H<'a>(&'a u8);
impl Drop for H<'_> { fn drop(&mut self) {} }
trait Tr: Debug {}
struct J<T: Tr>(T);
impl<T: Tr + Debug> Drop for J<T> { fn drop(&mut self) {} }
struct K<'a, T>(&'a T);
impl<'a, T: 'a> Drop for K<'a, T> { fn drop(&mut self) {} }
struct L<'a, T>(&'a T);
impl<'a, T> Drop for L<'a, T> where 'static: 'a { fn drop(&mut self) {} }
struct R<T>(*const T);
impl<'r, T: 'r> Drop for R<T> { fn drop(&mut self) {} }
struct M<'a, 'b: 'a, 'c: 'b>(&'a u8, &'b u8, &'c u8);
impl<'a, 'b, 'c> Drop for M<'a, 'b, 'c> where 'c: 'a { fn drop(&mut self) {} }
struct S<T: Display>(T);
impl<T: Display> Drop for S<T> { fn drop(&mut self) {} }
fn main() {}
"#, &[], "r14");
    // (a `ref` binding of a place the function owns borrows it: a closure's parameter bound `ref` (a loan of its
    // parameter) returned through an impl's lifetime is E0515)
    errors_at(r#"pub struct History<'a> { pub s: &'a str }
fn make_entry<'x, 'a>(_h: &'x History<'a>, _s: &'a String) -> Option<&'x str> { None }
impl<'a> History<'a> {
    fn me(&self, _s: &'a String) -> Option<&str> { None }
    pub fn page(&self) -> usize {
        let f = |ref line: String| make_entry(self, line);
        f(String::new()).is_some() as usize
    }
    pub fn page2(&self) -> usize {
        let g = |ref line: String| self.me(line);
        g(String::new()).is_some() as usize
    }
}
fn main() {}
"#, &[(6, "cannot return value referencing function parameter (E0515)"), (10, "cannot return value referencing function parameter (E0515)")], "r15");
    errors_at("fn mk(s: &String) -> &str { s.as_str() }\nfn f() -> usize {\n    let g = |ref s: String| mk(s);\n    g(String::new()).len()\n}\nfn main() {}\n", &[(3, "referencing function parameter (E0515)")], "r16");
    // (and is fine where the borrow does not escape)
    errors_at(r#"pub struct History<'a> { pub s: &'a str }
impl<'a> History<'a> {
    fn me(&self, _s: &String) -> usize { self.s.len() }
    pub fn page(&self) -> usize {
        let f = |ref line: String| self.me(line) + line.len();
        let (ref a, b) = (String::from("x"), 1);
        f(String::new()) + a.len() + b
    }
}
fn main() {
    let h = History { s: "abc" };
    let v: Vec<usize> = vec![String::from("ab")].into_iter().map(|ref s| s.len()).collect();
    let _ = (h.page(), v);
}
"#, &[], "r17");
    // (a `ref` binding through a reference borrows what it points to, not the place matched: `|&ref ints| ints`,
    // `match self { Imp::Owned(ref x) => x }` of `self: &Imp`)
    errors_at(r#"fn with<'a, F>(_: F) where F: FnOnce(&'a Vec<isize>) -> &'a Vec<isize> {}
enum Imp<'a> { Borrowed(&'a [u8]), Owned(Box<[u8]>) }
impl<'a> Imp<'a> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Imp::Owned(ref x) => x,
            Imp::Borrowed(x) => x,
        }
    }
}
fn keep<'a>(x: &'a String) -> &'a str {
    let ref r = *x;
    r.as_str()
}
fn opt<'a>(o: &'a Option<String>) -> Option<&'a str> {
    match *o {
        Some(ref s) => Some(s.as_str()),
        None => None,
    }
}
fn main() {
    with(|&ref ints| ints);
    let _ = (Imp::Borrowed(&[1]).as_slice().len(), keep(&String::new()), opt(&None));
}
"#, &[], "r18");
    // (the review's: a definition's bound written twice gives its equation (`T: Iterator` and `where T: Iterator<Item =
    // u8>`), and a bound that makes a `?Sized` parameter Sized (Clone, Copy, a trait of the program's `: Sized`))
    errors_at(r#"struct S<T: Iterator>(T) where T: Iterator<Item = u8>;
impl<T: Iterator<Item = u8>> Drop for S<T> { fn drop(&mut self) {} }
struct C<T: ?Sized + Clone>(Box<T>);
impl<T: Clone> Drop for C<T> { fn drop(&mut self) {} }
struct P<T: ?Sized + Copy>(Box<T>);
impl<T: Copy> Drop for P<T> { fn drop(&mut self) {} }
trait Tr: Sized {}
struct Q<T: ?Sized + Tr>(Box<T>);
impl<T: Tr> Drop for Q<T> { fn drop(&mut self) {} }
fn main() {}
"#, &[], "r19");
    // (a trait of the program's makes the parameter Sized only through its header (`: Sized`, `where Self: Sized`,
    // a supertrait that is Clone): an empty one does not, E0367)
    errors_at("trait Tr {}\nstruct S<T: ?Sized + Tr>(Box<T>);\nimpl<T: Tr> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(3, "requires `T: Sized`")], "r20");
    errors_at(r#"trait B: Clone {}
trait Tr: B {}
struct S<T: ?Sized + Tr>(Box<T>);
impl<T: Tr> Drop for S<T> { fn drop(&mut self) {} }
trait W where Self: Sized {}
struct V<T: ?Sized + W>(Box<T>);
impl<T: W> Drop for V<T> { fn drop(&mut self) {} }
fn main() {}
"#, &[], "r21");
}

#[test]
fn part42_drop_impl_trait_headers_equation_values_and_dyn_argument_lifetimes() {
    // (each checked with rustc 1.97. A trait makes a `?Sized` parameter Sized only through its supertraits or a
    // `Self: Sized` (Clone, Copy, Default, Sized), read from its parsed header: not a bound of its own parameters, nor
    // one on another type, nor a word in a comment)
    errors_at("trait Tr<U: ?Sized> {}\nstruct S<T: ?Sized + Tr<u8>>(Box<T>);\nimpl<T: Tr<u8>> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(3, "requires `T: Sized`")], "s1");
    errors_at("trait Tr<U: Clone> {}\nstruct S<T: ?Sized + Tr<u8>>(Box<T>);\nimpl<T: Tr<u8>> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(3, "requires `T: Sized`")], "s2");
    errors_at("trait Tr where u8: Clone {}\nstruct S<T: ?Sized + Tr>(Box<T>);\nimpl<T: Tr> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(3, "requires `T: Sized`")], "s3");
    errors_at("trait Tr: /* Default */ std::fmt::Debug {}\nstruct S<T: ?Sized + Tr>(Box<T>);\nimpl<T: Tr> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(3, "requires `T: Sized`")], "s7");
    errors_at(r#"trait A: Sized {}
struct SA<T: ?Sized + A>(Box<T>);
impl<T: A> Drop for SA<T> { fn drop(&mut self) {} }
trait B0: Clone {}
trait B: B0 {}
struct SB<T: ?Sized + B>(Box<T>);
impl<T: B> Drop for SB<T> { fn drop(&mut self) {} }
trait C where Self: Sized {}
struct SC<T: ?Sized + C>(Box<T>);
impl<T: C> Drop for SC<T> { fn drop(&mut self) {} }
trait D: std::default::Default {}
struct SD<T: ?Sized + D>(Box<T>);
impl<T: D> Drop for SD<T> { fn drop(&mut self) {} }
fn main() {}
"#, &[], "s4-s8");
    // (an equation of the impl's bounds has the definition's value, its parameters renamed)
    errors_at("struct S<T: Iterator<Item = u8>>(T);\nimpl<T: Iterator<Item = u16>> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(2, "requires `<T as Iterator>::Item == u16`")], "e1");
    errors_at("struct S<T: Iterator<Item = U>, U>(T, U);\nimpl<A: Iterator<Item = u8>, B> Drop for S<A, B> { fn drop(&mut self) {} }\nfn main() {}\n", &[(2, "requires `<A as Iterator>::Item == u8`")], "e4");
    errors_at(r#"struct S2<T: Iterator<Item = u8>>(T);
impl<T: Iterator<Item = u8>> Drop for S2<T> { fn drop(&mut self) {} }
struct S3<T: Iterator<Item = U>, U>(T, U);
impl<A: Iterator<Item = B>, B> Drop for S3<A, B> { fn drop(&mut self) {} }
struct S5<'a, T: Iterator<Item = &'a u8>>(T, &'a u8);
impl<'x, A: Iterator<Item = &'x u8>> Drop for S5<'x, A> { fn drop(&mut self) {} }
struct S6<T: Iterator>(T) where T: Iterator<Item = u8>;
impl<T: Iterator<Item = u8>> Drop for S6<T> { fn drop(&mut self) {} }
trait Trait { type Assoc; }
struct Foo<T: Trait>(T);
impl<T: Trait<Assoc = U>, U: ?Sized> Drop for Foo<T> { fn drop(&mut self) {} }
struct S8<T: Iterator<Item = (u8, Vec<String>)>>(T);
impl<T: Iterator<Item = (u8, Vec<String>)>> Drop for S8<T> { fn drop(&mut self) {} }
fn main() {}
"#, &[], "e2-e8");
    // (the review's: values the same type spelled apart (a path's prefix, an alias of the program's) are not
    // reported; ones surely apart are, std's and the program's types among them)
    errors_at(r#"type B = u8;
struct S1<T: Iterator<Item = String>>(T);
impl<T: Iterator<Item = std::string::String>> Drop for S1<T> { fn drop(&mut self) {} }
struct S2<T: Iterator<Item = u8>>(T);
impl<T: Iterator<Item = B>> Drop for S2<T> { fn drop(&mut self) {} }
struct S4<T: Iterator<Item = Option<u8>>>(T);
impl<T: Iterator<Item = core::option::Option<u8>>> Drop for S4<T> { fn drop(&mut self) {} }
fn main() {}
"#, &[], "q1-q4");
    errors_at("struct S<T: Iterator<Item = Vec<u8>>>(T);\nimpl<T: Iterator<Item = std::vec::Vec<u16>>> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(2, "requires `<T as Iterator>::Item == std::vec::Vec<u16>`")], "v1");
    errors_at("struct Foo;\nstruct S<T: Iterator<Item = Foo>>(T);\nimpl<T: Iterator<Item = u8>> Drop for S<T> { fn drop(&mut self) {} }\nfn main() {}\n", &[(3, "requires `<T as Iterator>::Item == u8`")], "v3");
    // (a trait object's arguments are invariant: a `'static` of the value's there makes `let _`'s annotation's
    // lifetime 'static (variance/variance-associated-types2.rs))
    errors_at("trait Foo { type Bar; }\nfn make() -> Box<dyn Foo<Bar = &'static u32>> { panic!() }\nfn take<'a>(_: &'a u32) {\n    let _: Box<dyn Foo<Bar = &'a u32>> = make();\n}\nfn main() {}\n", &[(4, "lifetime may not live long enough")], "d1");
    errors_at("trait Foo<T> {}\nfn take<'a>(x: &'a dyn Foo<&'static u32>) {\n    let _: &dyn Foo<&'a u32> = x;\n}\nfn main() {}\n", &[(3, "lifetime may not live long enough")], "d2");
    errors_at("fn take<'a>(x: Box<dyn Fn(&'static u32)>, _: &'a u32) {\n    let _: Box<dyn Fn(&'a u32)> = x;\n}\nfn main() {}\n", &[(2, "lifetime may not live long enough")], "d3");
    // (and are fine where the lifetimes agree, are elided (inferred), or are the object's own)
    errors_at(r#"trait Foo { type Bar; }
trait Gen<T> {}
fn make() -> Box<dyn Foo<Bar = &'static u32>> { panic!() }
fn take<'a>(_: &'a u32, x: Box<dyn Gen<&'static u32> + 'static>, y: Box<dyn Gen<&'static u32>>) {
    let _: Box<dyn Foo<Bar = &'static u32>> = make();
    let _: Box<dyn Foo<Bar = &u32>> = make();
    let _: Box<dyn Gen<&'static u32> + 'a> = x;
    let _: Box<dyn Gen<&u32>> = y;
}
fn take2<'a: 'static>(x: Box<dyn Gen<&'static u32>>, _: &'a u32) {
    let _: Box<dyn Gen<&'a u32>> = x;
}
fn main() {}
"#, &[], "d4");
}

#[test]
fn part43_trait_obligations_of_calls_relate_the_impls_they_select() {
    // (each checked with rustc 1.97. A bound of the callee that selects one of the program's impls: the impl's head is
    // the goal's type. A lifetime the bound binds (`for<'a>`) is a placeholder of the call, which no region of the
    // impl's may be but its own)
    errors_at("trait Foo<X> {}\nfn want_hrtb<T>() where T: for<'a> Foo<&'a isize> {}\nstruct StaticInt;\nimpl Foo<&'static isize> for StaticInt {}\nfn give_static() {\n    want_hrtb::<StaticInt>()\n}\nfn main() {}\n", &[(6, "implementation of `Foo` is not general enough")], "static");
    errors_at("trait Foo<X> {}\nfn want_hrtb<T>() where T: for<'a> Foo<&'a isize> {}\nimpl<'a> Foo<&'a isize> for &'a u32 {}\nfn give_some<'a>() {\n    want_hrtb::<&'a u32>()\n}\nfn main() {}\n", &[(5, "implementation of `Foo` is not general enough")], "some");
    errors_at("trait Foo<X> {}\nfn want_foo2<T: for<'a, 'b> Foo<(&'a isize, &'b isize)>>() {}\nstruct SomeStruct;\nimpl<'a> Foo<(&'a isize, &'a isize)> for SomeStruct {}\nfn b() { want_foo2::<SomeStruct>(); }\nfn main() {}\n", &[(5, "implementation of `Foo` is not general enough")], "conflate");
    errors_at("trait Bar<'x> {}\nimpl Bar<'static> for u16 {}\nfn want_bar_hr<T: for<'q> Bar<'q>>() {}\nfn f() { want_bar_hr::<u16>(); }\nfn main() {}\n", &[(4, "implementation of `Bar` is not general enough")], "lifetime arg");
    // (the impl's where clauses of such traits are obligations in turn, with its header's `'de: 'a`)
    errors_at(r#"trait Deserialize<'de> {}
trait DeserializeOwned: for<'de> Deserialize<'de> {}
impl<T> DeserializeOwned for T where T: for<'de> Deserialize<'de> {}
impl<'de: 'a, 'a> Deserialize<'de> for &'a str {}
fn main() {
    fn assert_deserialize_owned<T: DeserializeOwned>() {}
    assert_deserialize_owned::<&'static str>();
}
"#, &[(7, "implementation of `Deserialize` is not general enough")], "where clause");
    // (not only under a binder: an impl for `'static` makes the callee's lifetime 'static)
    errors_at("trait Foo<X> {}\nstruct St;\nimpl Foo<&'static isize> for St {}\nfn want_a<'a, T: Foo<&'a isize>>(_: &'a isize) {}\nfn f() { let x = 5; want_a::<St>(&x); }\nfn main() {}\n", &[(5, "does not live long enough")], "static arg");
    errors_at("trait Bar<'x> {}\nimpl Bar<'static> for u16 {}\nfn want_bar<'q, T: Bar<'q>>(_: &'q u8) {}\nfn f() { let x = 5u8; want_bar::<u16>(&x); }\nfn main() {}\n", &[(4, "does not live long enough")], "static lifetime arg");
    // (and impls that hold for every lifetime, or for the ones given, are fine)
    errors_at(r#"trait Foo<X> { fn foo(&self, _x: X) { } }
fn want_hrtb<T>() where T : for<'a> Foo<&'a isize> {}
fn want_foo1<T>() where T : for<'z> Foo<(&'z isize, &'z isize)> {}
fn want_a<'a, T: Foo<&'a isize>>(_: &'a isize) {}
struct AnyInt;
impl<'a> Foo<&'a isize> for AnyInt { }
struct SomeStruct;
impl<'a> Foo<(&'a isize, &'a isize)> for SomeStruct {}
struct Pair;
impl<'a, 'b> Foo<(&'a isize, &'b isize)> for Pair {}
struct St;
impl Foo<&'static isize> for St {}
impl<'a> Foo<&'a isize> for &'a u32 { }
trait Bar<'x> {}
impl<'x> Bar<'x> for u8 {}
impl Bar<'static> for u16 {}
fn want_bar<'q, T: Bar<'q>>(_: &'q u8) {}
fn want_bar_hr<T: for<'q> Bar<'q>>() {}
fn caller<'c>(r: &'c isize, q: &'c u8) {
    want_hrtb::<AnyInt>();
    want_foo1::<SomeStruct>();
    want_foo1::<Pair>();
    want_a::<AnyInt>(r);
    want_a::<&u32>(r);
    want_bar::<u8>(q);
    want_bar_hr::<u8>();
    static S: isize = 1;
    want_a::<St>(&S);
    want_bar::<u16>(&7);
}
fn main() { caller(&1, &2); }
"#, &[], "ok");
}

#[test]
fn part44_placeholders_universes_and_outlives_and_fn_pointer_binders() {
    // (each checked with rustc 1.97. A trait obligation's placeholder is none of the regions of the outer universe:
    // the reference's region of `&u32`, which inference makes, is not the impl's `'a` for every `'a`)
    errors_at("trait Foo<X> {}\nfn want_hrtb<T>() where T: for<'a> Foo<&'a isize> {}\nimpl<'a> Foo<&'a isize> for &'a u32 {}\nfn give() { want_hrtb::<&u32>(); }\nfn main() {}\n", &[(4, "implementation of `Foo` is not general enough")], "universe");
    // (and outlives only itself)
    errors_at(r#"fn main() {
    test::<FooS>(&mut 42);
}
trait Foo<'a> {}
struct FooS<'a> {
    data: &'a mut u32,
}
impl<'a, 'b: 'a> Foo<'b> for FooS<'a> {}
fn test<'a, F>(_data: &'a mut u32) where F: for<'b> Foo<'b> {}
"#, &[(2, "implementation of `Foo` is not general enough")], "outlives");
    // (a region a goal's fn pointer type binds is none of the impl's that its head does not bind there, both ways)
    errors_at("trait X {}\nimpl<'a> X for fn(&'a ()) {}\nfn indirect<T: X>() {}\nfn f() { indirect::<fn(&())>(); }\nfn main() {}\n", &[(4, "implementation of `X` is not general enough")], "fn binds");
    errors_at("trait Foo {}\nimpl<A> Foo for fn(A) {}\nfn assert_foo<T: Foo>() {}\nfn main() { assert_foo::<fn(&i32)>(); }\n", &[(4, "implementation of `Foo` is not general enough")], "fn(A)");
    errors_at("trait X {}\nimpl X for fn(&()) {}\nfn indirect<T: X>() {}\nfn f<'a>() { indirect::<fn(&'a ())>(); }\nfn main() {}\n", &[(4, "implementation of `X` is not general enough")], "head binds");
    errors_at("trait Trait<T> {}\nfn foo<T>() where T: Trait<for<'b> fn(fn(&'b u32))> {}\nimpl<'a> Trait<fn(fn(&'a u32))> for () {}\nfn main() { foo::<()>(); }\n", &[(4, "implementation of `Trait` is not general enough")], "bound's fn");
    errors_at("trait Foo {}\nimpl<A> Foo for Vec<fn(A)> {}\nimpl<'a> Foo for &'a u8 {}\nfn f<T: Foo>() {}\nfn main() { f::<Vec<fn(&u8)>>(); }\n", &[(5, "implementation of `Foo` is not general enough")], "Vec<fn(A)>");
    // (a type parameter of the impl that stands for the whole fn pointer type has its binder in it: fine)
    errors_at("trait Foo {}\nimpl<T> Foo for Box<T> {}\nimpl<T> Foo for Vec<T> {}\nimpl<T> Foo for (T, u8) {}\nimpl<'a, T> Foo for &'a [T] {}\nimpl<'a> Foo for &'a u8 {}\nfn f<T: Foo>() {}\nfn main() {\n    f::<Box<fn(&u8)>>();\n    f::<Vec<fn(&u8)>>();\n    f::<(fn(&u8), u8)>();\n    f::<&[fn(&u8)]>();\n}\n", &[], "T around fn");
    // (and impls general enough for them are fine; a region inference makes is either)
    errors_at(r#"trait X {}
impl X for fn(&()) {}
fn indirect<T: X>() {}
trait Y {}
impl<'a> Y for fn(&'a ()) {}
fn ind_y<T: Y>() {}
trait Foo {}
impl<A> Foo for fn(A) {}
fn assert_foo<T: Foo>() {}
trait Bar<'b> {}
struct FooS<'a>(&'a u8);
impl<'a, 'b> Bar<'b> for FooS<'a> {}
fn want_bar<F: for<'b> Bar<'b>>() {}
trait Baz<'x> {}
struct G<'y>(&'y u8);
impl<'a, 'b: 'a> Baz<'a> for G<'b> {}
fn want_baz<F: for<'x> Baz<'x>>() {}
trait Q<T> {}
impl<'a> Q<&'a u8> for u8 {}
fn want_q<T: for<'z> Q<&'z u8>>(_: T) {}
fn f<'a>(h: fn(&())) {
    indirect::<fn(&())>();
    indirect::<for<'b> fn(&'b ())>();
    ind_y::<fn(&'a ())>();
    ind_y::<fn(&'static ())>();
    assert_foo::<fn(i32)>();
    assert_foo::<fn(&'static i32)>();
    assert_foo::<fn(&'a i32)>();
    want_bar::<FooS>();
    want_baz::<G>();
    want_q(5u8);
    let _ = h;
}
fn main() { f(|_| {}); }
"#, &[], "ok");
}
