//! Region checking against signatures (crates/typeck/src/acg/regionck.rs): "lifetime may not live long enough" where
//! the constraints that are known require it, and nothing on code rustc accepts
use zustc::{Options2, Session2};

fn check(src: &str) -> Vec<(usize, String)> {
    let mut s = Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() });
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

/// A local that keeps a loan of a value already dead, through a call of the program's fn whose result borrows from
/// its argument
#[test]
fn keep_through_calls() {
    let d = check("fn gimme(x: &(u32,)) -> &u32 { &x.0 }\nfn main() {\n    let x = gimme({\n        let v = (22,);\n        &v\n    });\n    println!(\"{:?}\", x);\n}\n");
    assert!(d.len() == 1 && d[0].0 == 5 && d[0].1.contains("`v` does not live long enough"), "{d:?}");
    assert_eq!(check("fn first<'a>(_x: &u32, y: &'a u32) -> &'a u32 { y }\nfn main() {\n    let k = 1;\n    let x = first({ let v = 2; &v }, &k);\n    println!(\"{}\", x);\n}\n"), vec![]);
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
