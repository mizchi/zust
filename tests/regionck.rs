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
    println!("{} {} {}", p.rest(), p.peek(), p.take());
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
