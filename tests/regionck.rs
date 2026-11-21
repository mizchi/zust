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
