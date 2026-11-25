//! check (LSP の診断) はエラーがあっても止まらず、全関数の診断を返す
use zustc::{Options2, Session2};

#[test]
fn reports_all_errors_and_recovers() {
    let src = "enum E { A(i64), B }\nfn f(e: E) -> i64 {\n    match e {\n        E::A(x, y) => x,\n        E::B => 0,\n    }\n}\nfn g() -> bool { 1 }\nfn h() -> u8 { 2 }\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.len(), 2, "{d:?}");
    assert_eq!((d[0].line, d[1].line), (4, 8));
    // 直した後は診断が消え、変わっていない関数は検査済みの印で飛ばされる
    let fixed = src
        .replace("E::A(x, y)", "E::A(x)")
        .replace("-> bool { 1 }", "-> bool { true }");
    assert!(s.check(&fixed).is_empty());
    assert!(s.check(&fixed).is_empty());
}

/// 呼び出し元が「検査済み」でキャッシュに当たっても、ジェネリック関数の本体の型エラーは報告される
/// (検査済みの印に、その関数が要求したインスタンスを保存して要求し直すため)
#[test]
fn generic_body_errors_with_cached_callers() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/traits.rs"))
        .unwrap();
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    assert!(s.check(&src).is_empty());
    let bad = src.replace(
        "    a.area() + b.area()\n",
        "    a.area() + b.area() + true\n",
    );
    assert_ne!(bad, src);
    let d = s.check(&bad);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].msg.contains("expected `i64`, found `bool`"), "{d:?}");
    assert!(s.check(&src).is_empty());
}

/// クロージャの本体のエラーは (呼ばれていなくても) 正しい行で報告され、直せば消える。
/// 検査済みの印が付いた後も、編集したクロージャの本体だけが検査し直される
#[test]
fn closure_and_dyn_errors() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/closures.rs"))
        .unwrap();
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    assert!(s.check(&src).is_empty());
    let bad = src.replace("let add_k = |x| x + k;", "let add_k = |x| x + k + nope;");
    assert_ne!(bad, src);
    let d = s.check(&bad);
    assert_eq!(d.len(), 1, "{d:?}");
    let line = src.lines().position(|l| l.contains("let add_k")).unwrap() + 1;
    assert_eq!(d[0].line, line, "{d:?}");
    assert!(s.check(&src).is_empty());

    let dy = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/dyn_trait.rs"))
        .unwrap();
    assert!(s.check(&dy).is_empty());
    let bad = dy.replace("let d: &dyn Shape = &q;", "let d: &dyn Shape = &5u32;");
    assert_ne!(bad, dy);
    let d = s.check(&bad);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].msg.contains("not implemented for `u32`"), "{d:?}");
}

/// 編集をまたいだ check の記録 (ChkMemo) を使っても、毎回新しい Session2 で検査したものと診断が同じ。
/// examples の関数の本体 (総称関数も) に、型エラーになる文・ならない文を足したり消したりを続ける
#[test]
fn check_memo_matches_fresh_check() {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut ps: Vec<_> = std::fs::read_dir(format!("{root}/examples")).unwrap().map(|e| e.unwrap().path()).collect();
    ps.sort();
    let stmts = ["let _q: u32 = 1;\n", "let _q: u32 = true;\n", "let _w = 1u8 + 2u16;\n", "let _e: i64 = 3;\n"];
    let mut rng: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |n: usize| -> usize {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % n as u64) as usize
    };
    let mut compared = 0;
    for p in ps.iter().filter(|p| p.extension().is_some_and(|e| e == "rs")).take(20) {
        let src = std::fs::read_to_string(p).unwrap();
        let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
        let mut cur = src.clone();
        for _ in 0..8 {
            let d1 = s.check(&cur);
            let d2 = Session2::new(Options2 { jobs: 1, ..Options2::default() }).check(&cur);
            let f = |d: &[zustc::Diagnostic]| d.iter().map(|x| (x.line, x.col, x.msg.clone())).collect::<Vec<_>>();
            assert_eq!(f(&d1), f(&d2), "{}", p.display());
            compared += 1;
            // (関数の本体の先頭 (`{\n` の後ろ) に文を足すか、足したものを消す)
            let at: Vec<usize> = cur.match_indices(") {\n").map(|(i, _)| i + 4).collect();
            if at.is_empty() {
                break;
            }
            if next(3) == 0 && cur != src {
                cur = src.clone();
            } else {
                let i = at[next(at.len())];
                cur = format!("{}    {}{}", &cur[..i], stmts[next(stmts.len())], &cur[i..]);
            }
        }
    }
    assert!(compared > 50, "{compared}");
}

/// Each constant and static is checked against its type where it is defined, used or not (as rustc does), with the
/// inference of a fn body (`{ let mut b = 1; b }` is a u32). Fixing one clears its diagnostic
#[test]
fn constants_checked_where_defined() {
    let src = "const LIMIT: u8 = 300u16;\nconst SIZES: [u8; 3] = [1, 2];\nstatic NAME: &[u8] = \"abc\";\nconst FINE: u32 = { let mut b = 1; b += 1; b };\nconst LABEL: &str = \"ok\";\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![1, 2, 3], "{d:?}");
    assert!(d.iter().all(|d| d.msg.starts_with("mismatched types")), "{d:?}");
    let fixed = src.replace("300u16", "200").replace("[1, 2]", "[1, 2, 3]").replace("\"abc\"", "b\"abc\"");
    assert!(s.check(&fixed).is_empty(), "{:?}", s.check(&fixed));
}

/// An enum's discriminants are of its `#[repr(..)]` integer type (isize without one), and a constant or an enum
/// written in a fn body is reported at the line it was written on (not where it was hoisted to)
#[test]
fn discriminants_and_nested_constants() {
    let src = "enum Level {\n    Low = 1i64,\n    High = 2,\n}\nfn f() {\n    #[repr(u8)]\n    enum Small {\n        A = 1,\n        B = 2i8,\n    }\n    const LIMIT: u8 = 300u16;\n    let _ = Small::A;\n}\nfn main() {\n    let _ = Level::Low;\n    f();\n}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![2, 9, 11], "{d:?}");
}

/// A generic fn nothing calls is checked with placeholders for its type parameters: a mismatch of types written out,
/// and an operator its bounds do not give, are reported; not a mismatch a bound may explain (`Tr<X = T>`, `&T: Add`)
#[test]
fn uncalled_generic_fns() {
    let src = "use std::fmt::Debug;\nfn shown(x: impl Debug) -> String {\n    x\n}\nfn total<T>(a: T, b: T) -> T {\n    a + b\n}\ntrait Layout { type Unit; }\nfn unit<L: Layout<Unit = U>, U>(u: U) -> U { u }\nfn add_refs<T>(a: &T, b: &T) -> T where for<'x> &'x T: std::ops::Add<&'x T, Output = T> {\n    a + b\n}\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![3, 6], "{d:?}");
    assert!(d[1].msg.contains("cannot add `T` to `T`"), "{d:?}");
}

/// A constant whose value needs its own is E0391 at the constant, not an endless evaluation
#[test]
fn constant_cycles() {
    let src = "const A: i32 = B;\nconst B: i32 = A;\nconst SELF_SIZE: usize = SELF_SIZE;\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert!(d.iter().all(|d| d.msg.contains("(E0391)")), "{d:?}");
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![1, 2, 3], "{d:?}");
}

/// E0038 for `dyn Tr` in a signature when Tr is not dyn compatible (a method without `self` nor `where Self: Sized`),
/// called or not; not when that method is `where Self: Sized`
#[test]
fn dyn_incompatible_in_signature() {
    let bad = "trait Shape {\n    fn new() -> Self;\n    fn area(&self) -> f64;\n}\n#[inline]\nfn total(shapes: &[Box<dyn Shape>]) -> f64 {\n    shapes.iter().map(|s| s.area()).sum()\n}\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(bad);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![6], "{d:?}");
    assert!(d[0].msg.contains("(E0038)"), "{d:?}");
    let good = bad.replace("fn new() -> Self;", "fn new() -> Self\n    where\n        Self: Sized;");
    assert!(s.check(&good).is_empty(), "{:?}", s.check(&good));
}

#[test]
fn errors_of_items_in_fn_bodies_at_their_lines() {
    // an item in a fn body is checked after the text, and its error is reported where it is written
    let bad = "fn outer<T>(x: T) {\n    fn inner(y: T) {}\n}\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(bad);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![2], "{d:?}");
    // (rustc's E0401: the inner fn cannot name the outer one's parameter)
    assert!(d[0].msg.contains("can't use generic parameters from outer item (E0401)"), "{d:?}");
    let dir = std::env::temp_dir().join(format!("zust-unhoist-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let e = Session2::new(Options2 { jobs: 1, ..Options2::default() }).build(bad, &dir.join("a")).unwrap_err();
    assert!(e.starts_with("2:"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A raw string with quotes and brackets in an enum's body is no part of its discriminants; a program that writes
/// the probes' names gets probes of other names
#[test]
fn discriminants_past_raw_strings_and_probe_names_of_the_program() {
    let src = "#[repr(u8)]\nenum E {\n    #[doc = r#\"\"{\"#]\n    A = 1,\n    B = 2i8,\n}\nfn main() {\n    let _ = E::A;\n}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![5], "{d:?}");
    let named = "fn __zust_const_0() {}\nconst BAD: u8 = 300u16;\nfn main() {\n    __zust_const_0();\n}\n";
    let d = s.check(named);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![2], "{d:?}");
    assert!(d[0].msg.contains("mismatched types"), "{d:?}");
}

/// What the scans for discriminants and constants read past (review of #52): a char literal's quote, a doc's
/// `repr(`, a doc's bracket; a mismatch with a raw pointer to an expected type that is none; a repeat of `Self::C`
#[test]
fn discriminants_and_constants_past_literals() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let lines = |s: &mut Session2, src: &str| s.check(src).iter().map(|d| d.line).collect::<Vec<_>>();
    let quote = "#[repr(u8)]\nenum E {\n    A = b'\"',\n    /* } */\n    B = 2i8,\n}\nfn main() {\n    let _ = E::A;\n}\n";
    assert_eq!(lines(&mut s, quote), vec![5]);
    let doc = "#[doc = \"repr(u8)\"]\nenum E {\n    A = 1isize,\n}\nfn main() {\n    let _ = E::A;\n}\n";
    assert_eq!(lines(&mut s, doc), Vec::<usize>::new());
    let bracket = "#[repr(u8)]\nenum E {\n    #[doc = \"(\"]\n    A = 1i8,\n}\nfn main() {\n    let _ = E::A;\n}\n";
    assert_eq!(lines(&mut s, bracket), vec![4]);
    let ptr = "const BAD: bool = 0 as *const u8;\nfn main() {}\n";
    assert_eq!(lines(&mut s, ptr), vec![1]);
    let repeat = "struct S;\nimpl S {\n    const C: S = S;\n    fn make() {\n        let _ = [Self::C; 2];\n    }\n}\nfn main() {\n    S::make();\n}\n";
    assert_eq!(lines(&mut s, repeat), Vec::<usize>::new());
}

/// A scrutinee's type from the arms: a unit struct pattern (`Some(S)`), a reference pattern of a type not known yet
/// (`&S`), as rustc infers `fn get<T>() -> T` from them (rust-lang/rust#55810)
#[test]
fn scrutinee_types_from_unit_struct_and_reference_patterns() {
    let src = "struct S;\nimpl S {\n    fn method(&self) -> bool {\n        true\n    }\n}\nfn get<T>() -> T {\n    unimplemented!()\n}\nfn opt<T>() -> Option<T> {\n    None\n}\nfn main() {\n    match opt() {\n        Some(S) => {}\n        None => {}\n    }\n    let w = opt();\n    if let Some(S) = w {}\n    match get() {\n        x if x.method() => {}\n        &S => {}\n    }\n}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert!(d.is_empty(), "{d:?}");
}

/// Review of #55: an operator a bound's supertrait gives, a mismatch a bound's equation may make (`Deref<Target = i32>`,
/// in a where clause too) are no errors of an uncalled generic fn; `dyn` in a fn pointer of a signature is read; an
/// item's `#[allow(deprecated)]` allows its intrinsics paths
#[test]
fn uncalled_generic_fns_with_supertraits_and_equations() {
    let ok = "trait Sum: std::ops::Add<Output = Self> + Sized {}\nfn f<T: Sum>(a: T, b: T) -> T {\n    a + b\n}\nfn g<T: std::ops::Deref<Target = i32>>(x: &T) -> &i32 {\n    x\n}\nfn h<T>(x: &T) -> &i32\nwhere\n    T: std::ops::Deref<Target = i32>,\n{\n    x\n}\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    assert!(s.check(ok).is_empty(), "{:?}", s.check(ok));
    let fnptr = "trait Bad {\n    fn make() -> Self;\n}\nfn unused<T>(_: fn(&dyn Bad)) {}\nfn main() {}\n";
    let d = s.check(fnptr);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![4], "{d:?}");
    assert!(d[0].msg.contains("(E0038)"), "{d:?}");
    let allowed = "#![deny(deprecated)]\n#[allow(deprecated)]\nfn conv(x: f32) -> u32 {\n    unsafe { std::intrinsics::transmute(x) }\n}\nfn main() {\n    let _ = conv(1.0);\n}\n";
    assert!(s.check(allowed).is_empty(), "{:?}", s.check(allowed));
    // (a string that says `allow(deprecated` allows nothing)
    let in_str = "#![deny(deprecated)]\nfn main() {\n    let _ = \"allow(deprecated\";\n    let _: u32 = unsafe { std::intrinsics::transmute(0u32) };\n}\n";
    assert_eq!(s.check(in_str).iter().map(|d| d.line).collect::<Vec<_>>(), vec![4]);
    // (an equation fixes `T::Target`, not T: a T returned as i32 is mismatched types, inline or in a where clause)
    let by_value = "trait Tr {\n    type Out;\n}\nfn bad<T: Tr<Out = i32>>(x: T) -> i32 {\n    x\n}\nfn bad2<T>(x: T) -> i32\nwhere\n    T: Tr<Out = i32>,\n{\n    x\n}\nfn main() {}\n";
    let d = s.check(by_value);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![5, 11], "{d:?}");
}

/// Review of #66: a constant's overflowing literal is reported at the constant, used or not; taking an
/// `#![allow(overflowing_literals)]` out leaves the declarations as they were, and the check memo is not reused then
#[test]
fn overflowing_literals_in_constants_and_after_allow_removed() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let unused = "fn main() {}\n\nconst BAD: i8 = -129;\n";
    let d = s.check(unused);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![3], "{d:?}");
    assert!(d[0].msg.contains("literal out of range for `i8`"), "{d:?}");
    let allowed = "#![allow(overflowing_literals)]\nfn main() {\n    let x: i8 = -129;\n    println!(\"{x}\");\n}\n";
    assert!(s.check(allowed).is_empty(), "{:?}", s.check(allowed));
    let denied = allowed.replace("#![allow(overflowing_literals)]", "");
    let d = s.check(&denied);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![3], "{d:?}");
}

/// Two type parameters nothing relates (no equation, no fn bound, no where clause) are mismatched in a generic fn
/// nothing calls, a trait's default method too; `x.clone()` of `x: &T` without `T: Clone` is the reference's, and an
/// operator on a reference to a parameter its bounds do not give is E0369. Parameters an equation, a bound or a where
/// clause relates are not
#[test]
fn free_parameters_mismatch_and_reference_operators() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let src = "fn f<X, Y>(x: X, y: Y) -> X {\n    y\n}\ntrait Tr: Sized {\n    fn test<X>(u: X) -> Self {\n        u\n    }\n}\nfn ok<T>(x: &T) -> &T {\n    x.clone()\n}\nfn add_ref<T: Copy>(a: T) {\n    let _ = &a + a;\n}\nfn main() {}\n";
    let d = s.check(src);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![2, 6, 13], "{d:?}");
    assert!(d[2].msg.contains("cannot add `T` to `&T`"), "{d:?}");
    let related = "trait Tr { type Out; }\nfn g<T: Tr<Out = U>, U>(x: T::Out) -> U {\n    x\n}\nfn h<T, U>(x: T) -> U\nwhere\n    T: Into<U>,\n{\n    x.into()\n}\nfn r<T>(a: &T, b: T) -> T\nwhere\n    for<'x> &'x T: std::ops::Add<T, Output = T>,\n{\n    a + b\n}\nfn swap<A, B>(p: (A, B)) -> (B, A) {\n    (p.1, p.0)\n}\nfn pick<A: Clone, B>(a: &A, _b: B) -> A {\n    a.clone()\n}\nfn main() {}\n";
    assert!(s.check(related).is_empty(), "{:?}", s.check(related));
}

/// `[x; 3]` of a free parameter without Copy is E0277 in a generic fn nothing calls; a bound with Copy or Clone among
/// its supertraits (`trait Num: Copy`, `trait Dup: Clone`) makes the parameter Copy / Clone there
#[test]
fn repeat_needs_copy_and_marker_supertraits() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let src = "trait Num: Copy {}\nfn ok<T: Num>(x: T) -> [T; 3] { [x; 3] }\nfn ok2<T: Copy, const N: usize>(x: T) -> [T; N] { [x; N] }\nfn g<T, const N: usize>(x: T) -> [T; N] {\n    [x; N]\n}\nfn h<T>(x: T) -> [T; 3] {\n    [x; 3]\n}\nfn main() {}\ntrait Dup: Clone {}\nfn dup<T: Dup>(x: &T) -> T { x.clone() }\ntrait Cp: Copy {}\nfn dup2<T: Cp>(x: &T) -> T { x.clone() }\n";
    let d = s.check(src);
    assert_eq!(d.iter().map(|d| d.line).collect::<Vec<_>>(), vec![8], "{d:?}");
    assert!(d[0].msg.contains("the trait bound `T: Copy` is not satisfied"), "{d:?}");
}

/// Review of #68: a supertrait chain gives no Copy it does not have, and its equation however deep it is (or in a
/// `where Self:` predicate); `where Self: Copy` is a supertrait; a parameter bounded by a trait with a supertrait
/// equation is not free; a variant pattern may leave defaulted arguments out
#[test]
fn review68_supertraits_equations_defaults() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let long = "trait A9 {}\ntrait A8: A9 {}\ntrait A7: A8 {}\ntrait A6: A7 {}\ntrait A5: A6 {}\ntrait A4: A5 {}\ntrait A3: A4 {}\ntrait A2: A3 {}\ntrait A1: A2 {}\ntrait A0: A1 {}\nfn rep<T: A0>(x: T) -> [T; 3] {\n    [x; 3]\n}\nfn main() {}\n";
    assert_eq!(s.check(long).iter().map(|d| d.line).collect::<Vec<_>>(), vec![12], "{:?}", s.check(long));
    for ok in ["trait A8<U>: std::ops::Deref<Target = U> {}\ntrait A7<U>: A8<U> {}\ntrait A6<U>: A7<U> {}\ntrait A5<U>: A6<U> {}\ntrait A4<U>: A5<U> {}\ntrait A3<U>: A4<U> {}\ntrait A2<U>: A3<U> {}\ntrait A1<U>: A2<U> {}\ntrait A0<U>: A1<U> {}\nfn project<T: A0<U>, U>(x: &T) -> &U {\n    x\n}\nfn main() {}\n", "trait View<U> where Self: std::ops::Deref<Target = U> {}\nfn project<T: View<U>, U>(x: &T) -> &U {\n    x\n}\nfn main() {}\n", "trait Num where Self: Copy {}\nfn repeat<T: Num>(x: T) -> [T; 3] {\n    [x; 3]\n}\nfn main() {}\n", "trait View<U>: std::ops::Deref<Target = U> {}\nfn project<T: View<U>, U>(x: &T) -> &U {\n    x\n}\nfn main() {}\n", "enum E<T, U = ()> {\n    V(T, U),\n}\nfn main() {\n    let e = E::<u8, ()>::V(1, ());\n    match e {\n        E::<u8>::V(..) => {}\n    }\n}\n"] {
        assert!(s.check(ok).is_empty(), "{:?}", s.check(ok));
    }
}

/// A struct field or an impl's self type naming a struct of the program's with a bounded parameter: the bound is held
/// by the enclosing item's own bound, a where clause, a supertrait, a bound with arguments, an impl for a concrete
/// argument, or a parameter's implicit `Sized`, and then nothing is reported
#[test]
fn wf_bounds_held_by_bounds_where_clauses_and_supertraits() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let src = "trait Tr {}\ntrait Sub: Tr {}\nstruct S<T: Tr>(T);\nstruct A<K: Tr> {\n    s: S<K>,\n}\nstruct B<K>\nwhere\n    K: Tr,\n{\n    s: S<K>,\n}\nstruct C<K: Sub> {\n    s: S<K>,\n}\nstruct D<K: Tr + Clone> {\n    s: Vec<S<K>>,\n}\nimpl<K: Tr> S<K> {\n    fn get(&self) -> &K {\n        &self.0\n    }\n}\nimpl Tr for u8 {}\nstruct E {\n    s: S<u8>,\n}\ntrait G<X> {}\nstruct H<T: G<u8>>(T);\nstruct I<K: G<u8>> {\n    h: H<K>,\n}\nstruct Sz<T: Sized>(T);\nstruct J<K> {\n    s: Sz<K>,\n}\nimpl<K> Clone for A<K>\nwhere\n    K: Tr + Clone,\n{\n    fn clone(&self) -> Self {\n        A { s: S(self.s.get().clone()) }\n    }\n}\nfn main() {}\n";
    assert!(s.check(src).is_empty(), "{:?}", s.check(src));
}

/// A parameter that says `?Sized` (in its bounds or a where clause) does not hold a `Sized` bound, even with the word
/// `Sized` in its `?Sized`, unless another of its bounds has `Sized` as a supertrait (`Clone`); another parameter of the
/// same item still does, and a `?Sized` in a string declares nothing
#[test]
fn wf_maybe_unsized_parameter_does_not_hold_sized() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let bad = "struct Sz<T: Sized>(T);\nstruct J<K: ?Sized> {\n    s: Box<Sz<K>>,\n}\nfn main() {}\n";
    let ds = s.check(bad);
    assert_eq!(ds.iter().map(|d| d.line).collect::<Vec<_>>(), vec![3], "{ds:?}");
    assert!(ds[0].msg.contains("`K: Sized`"), "{ds:?}");
    let wher = "struct Sz<T: Sized>(T);\nstruct J<K>\nwhere\n    K: std::fmt::Debug + ?Sized,\n{\n    s: Box<Sz<K>>,\n}\nfn main() {}\n";
    let ds = s.check(wher);
    assert_eq!(ds.iter().map(|d| (d.line, d.msg.contains("`K: Sized`"))).collect::<Vec<_>>(), vec![(6, true)], "{ds:?}");
    // (std's `Clone` has `Sized` as a supertrait, before or after the `?Sized`: `T: ?Sized + Clone` is Sized)
    let clone_first = "struct Sz<T: Sized>(T);\nstruct W<T: ?Sized + Clone> {\n    value: Sz<T>,\n}\nfn main() {}\n";
    assert!(s.check(clone_first).is_empty(), "{:?}", s.check(clone_first));
    let ok = "struct Sz<T: Sized>(T);\nstruct J<K, V: ?Sized> {\n    s: Sz<K>,\n    v: Box<V>,\n}\nfn main() {}\n";
    assert!(s.check(ok).is_empty(), "{:?}", s.check(ok));
    // (`Clone` has `Sized` as a supertrait: `K: Clone + ?Sized` is Sized; a `?Sized` in a string declares nothing)
    let clone = "struct Sz<T: Sized>(T);\nstruct J<K>\nwhere\n    K: Clone + ?Sized,\n{\n    s: Box<Sz<K>>,\n}\nfn main() {}\n";
    assert!(s.check(clone).is_empty(), "{:?}", s.check(clone));
    let doc = "struct Sz<T: Sized>(T);\n#[doc = \"K: ?Sized\"]\nstruct J<K> {\n    s: Sz<K>,\n}\nfn main() {}\n";
    assert!(s.check(doc).is_empty(), "{:?}", s.check(doc));
}

/// A name nothing declares, in a generic fn nothing calls or a trait's default method, is E0425 there too, whatever it
/// is spelled; one an item or a `use` declares is not (zustc may not lower a generic fn as a value), a word in a
/// comment in the `use` declares nothing, and `use` then a newline is a `use` too
/// (a fn whose parameter is bounded by a trait whose methods the body may call, `T: PartialEq` with `==`, gets no
/// placeholder instance in zustc, so the name is not looked up in it)
#[test]
fn undeclared_names_in_uncalled_generic_fns() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let src = "fn f() -> i32 {\n    foo + 1\n}\nfn g<T: Copy>() {\n    foo == 2;\n}\ntrait B<A> {\n    fn a() -> A {\n        this.a\n    }\n}\nfn main() { f(); }\n";
    let ds = s.check(src);
    assert_eq!(ds.iter().map(|d| d.line).collect::<Vec<_>>(), vec![2, 5, 9], "{ds:?}");
    assert!(ds.iter().all(|d| d.msg.contains("cannot find value `foo`") || d.msg.contains("cannot find value `this`")), "{ds:?}");
    let named = "fn f<T>() {\n    unsupported_value;\n}\nfn main() {}\n";
    assert_eq!(s.check(named).iter().map(|d| d.line).collect::<Vec<_>>(), vec![2], "{:?}", s.check(named));
    let comment = "use std::convert::identity /* ghost */;\nfn f<T>() {\n    ghost;\n}\nfn main() {}\n";
    assert_eq!(s.check(comment).iter().map(|d| d.line).collect::<Vec<_>>(), vec![3], "{:?}", s.check(comment));
    let ok = "use std::convert::identity;\nfn g<T: Copy>(x: T) -> T {\n    let f = identity;\n    f(x)\n}\nfn h<T>() -> fn(T) -> T {\n    fn id<U>(x: U) -> U {\n        x\n    }\n    id\n}\nfn main() {}\n";
    assert!(s.check(ok).is_empty(), "{:?}", s.check(ok));
    let newline = "use\nstd::convert::identity;\nfn g<T: Copy>(x: T) -> T {\n    let f = identity;\n    f(x)\n}\nfn main() {}\n";
    assert!(s.check(newline).is_empty(), "{:?}", s.check(newline));
    let semi = "use std::convert::{ /* ; */ identity };\nfn g<T: Copy>(x: T) -> T {\n    let f = identity;\n    f(x)\n}\nfn main() {}\n";
    assert!(s.check(semi).is_empty(), "{:?}", s.check(semi));
}

/// `#![deny(dead_code)]` below the item level: a field nothing reads (a derived Debug / Clone reads none), a variant
/// nothing constructs (a pattern does not, a `match` scrutinee does), an inherent impl's const nothing names, a type
/// alias nothing names. `#![allow(unused_variables)]` leaves dead_code denied; a type named only in `*const T` or an
/// impl header, a union field something writes and a tuple struct's `()` field are used
#[test]
fn dead_code_fields_variants_members_aliases() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let field = "#![allow(unused_variables)]\n#![deny(dead_code)]\n#[derive(Debug, Clone)]\nstruct A {\n    x: u8,\n    f: u8,\n}\nfn main() {\n    let a = A { x: 1, f: 2 };\n    let _ = a.x;\n}\n";
    assert_eq!(first(field), Some((6, "field `f` is never read".to_string())));
    let tuple = "#![deny(dead_code)]\nstruct T(u8, u8);\nstruct U(u8, u8);\nfn main() {\n    let t = T(1, 2);\n    let u = U(1, 2);\n    let _ = (t.0, u.0, u.1);\n}\n";
    assert_eq!(first(tuple), Some((2, "field `1` is never read".to_string())));
    let variant = "#![deny(dead_code)]\nenum E {\n    A,\n    B,\n}\nfn main() {\n    match E::A {\n        E::A => {}\n        E::B => {}\n    }\n}\n";
    assert_eq!(first(variant), Some((4, "variant `B` is never constructed".to_string())));
    let member = "#![deny(dead_code)]\nstruct S;\nimpl S {\n    const C: u8 = 1;\n}\nfn main() {\n    let _ = S;\n}\n";
    assert_eq!(first(member), Some((4, "associated constant `C` is never used".to_string())));
    let alias = "#![deny(dead_code)]\ntype Used = u8;\ntype Unused = u8;\nfn id(x: Used) -> Used {\n    x\n}\nfn main() {\n    id(0);\n}\n";
    assert_eq!(first(alias), Some((3, "type alias `Unused` is never used".to_string())));
    let ok = "#![deny(dead_code)]\nstruct W;\ntype WI = W;\ntrait T {\n    fn t(&self) {}\n}\nimpl T for WI {}\nenum V {}\nextern \"C\" {\n    fn free(p: *const V);\n}\nunion U {\n    a: u8,\n    b: u8,\n}\nstruct P(u8, ());\nfn main() {\n    W.t();\n    unsafe { free(std::ptr::null()) };\n    let u = U { a: 1 };\n    let _ = unsafe { u.b };\n    let p = P(1, ());\n    let _ = p.0;\n}\n";
    assert_eq!(first(ok), None);
}

/// `#![deny(unreachable_code)]` inside expressions: an argument after one that diverges, a call whose last argument
/// diverges, an operation whose operand diverges, and the statement after a `match` on an empty enum or an
/// `if`/`else` whose branches both diverge; not the right of `||`, which may not run
#[test]
fn unreachable_code_in_expressions() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let arg = "#![deny(unreachable_code)]\nfn foo(x: u8, y: usize) {}\nfn a() {\n    foo(return, 22);\n}\nfn main() {}\n";
    assert_eq!(first(arg), Some((4, "unreachable expression".to_string())));
    let call = "#![deny(unreachable_code)]\nfn bar(x: u8) {}\nfn b() {\n    bar(return);\n}\nfn main() {}\n";
    assert_eq!(first(call), Some((4, "unreachable call".to_string())));
    let op = "#![deny(unreachable_code)]\nfn c() -> u8 {\n    let x = 1 + return 2;\n    x\n}\nfn main() {}\n";
    assert_eq!(first(op).map(|f| f.0), Some(3));
    let empty = "#![deny(unreachable_code)]\nenum Void {}\nfn d(v: Void) {\n    match v {}\n    let x = 2;\n}\nfn main() {}\n";
    assert_eq!(first(empty), Some((5, "unreachable statement".to_string())));
    let both = "#![deny(unreachable_code)]\nfn e(b: bool) {\n    if b {\n        return;\n    } else {\n        return;\n    }\n    println!(\"dead\");\n}\nfn main() {}\n";
    assert_eq!(first(both), Some((8, "unreachable statement".to_string())));
    let ok = "#![deny(unreachable_code)]\nfn f(b: bool) -> bool {\n    let x = b || return false;\n    if b {\n        return true;\n    }\n    x\n}\nfn main() {}\n";
    assert_eq!(first(ok), None);
}

/// `#![deny(unused_variables)]` beyond `let`: a `match` arm's binding its guard and body never read (an or-pattern's
/// names once), a closure's parameter its body never reads, a `for` loop's binding the body never reads
#[test]
fn unused_variables_in_arms_closures_and_loops() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let arm = "#![deny(unused_variables)]\nfn main() {\n    let v = match 0u8 {\n        a => 0,\n    };\n    let _ = v;\n}\n";
    assert_eq!(first(arm), Some((4, "unused variable: `a`".to_string())));
    let or = "#![deny(unused_variables)]\nenum E {\n    A(i32),\n    B(i32),\n}\nfn main() {\n    match E::A(1) {\n        E::A(x) | E::B(x) => {}\n    }\n}\n";
    assert_eq!(first(or), Some((8, "unused variable: `x`".to_string())));
    let closure = "#![deny(unused_variables)]\nfn main() {\n    let f = |x: u8| 1;\n    let _ = f(2);\n}\n";
    assert_eq!(first(closure), Some((3, "unused variable: `x`".to_string())));
    let ok = "#![deny(unused_variables)]\nfn main() {\n    let v = [1u8, 2];\n    for x in v.iter() {\n        let _ = x;\n    }\n    let n = match 3u8 {\n        k if k > 2 => 1,\n        _ => 0,\n    };\n    let g = |y: u8| y + n;\n    let _ = g(1);\n}\n";
    assert_eq!(first(ok), None);
}

/// Review of #72: a variant in a `match` arm's body (`true => E::A, false => E::B`) is constructed; a value a fn
/// returns (`let s = make()`) is no struct's own; a comma in a tuple field's type (`Result<u8, u8>`) splits nothing;
/// `warn(dead_code)` (of the crate, a field) denies nothing, but `deny(warnings)` makes it an error; `export_name`
/// uses an inherent fn; an empty braced struct is a 1-ZST field; `F::B` of another enum constructs no `E::B`; block
/// comments nest in a `use`; `Iterator<Item = u8> + ?Sized` is still `?Sized`
#[test]
fn review72_dead_code_and_sized() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let flag = true;\n    let _e = match flag { true => E::A, false => E::B };\n}\n";
    assert!(s.check(src).is_empty(), "arms_ok: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let flag = true;\n    let _e = match flag {\n        true => { E::A }\n        false => E::B,\n    };\n}\n";
    assert!(s.check(src).is_empty(), "block_arm_ok: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\nstruct S { value: u8 }\nfn make() -> S { S { value: 1 } }\nfn main() {\n    let s = make();\n    let _ = s.value;\n}\n";
    assert!(s.check(src).is_empty(), "factory_ok: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\nstruct Pair(Result<u8, u8>, u8);\nfn main() {\n    let p = Pair(Ok(1), 2);\n    let _ = (p.0, p.1);\n}\n";
    assert!(s.check(src).is_empty(), "tuple_result_ok: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\n#![warn(dead_code)]\nfn unused() {}\nfn main() {}\n";
    assert!(s.check(src).is_empty(), "warn_ok: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\nstruct S {\n    x: u8,\n    #[warn(dead_code)]\n    y: u8,\n}\nfn main() {\n    let s = S { x: 1, y: 2 };\n    let _ = s.x;\n}\n";
    assert!(s.check(src).is_empty(), "field_warn_ok: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\nstruct S;\nimpl S {\n    #[unsafe(export_name = \"entry\")]\n    extern \"C\" fn dispatch() {}\n}\nfn main() {\n    let _ = S;\n}\n";
    assert!(s.check(src).is_empty(), "export_ok: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\nstruct Marker {}\nstruct Pair(u8, Marker);\nfn main() {\n    let p = Pair(1, Marker {});\n    let _ = p.0;\n}\n";
    assert!(s.check(src).is_empty(), "marker_ok: {:?}", s.check(src));
    let src = "use std::convert::{ /* a /* b */ ; */ identity };\nfn g<T: Copy>(x: T) -> T {\n    let f = identity;\n    f(x)\n}\nfn main() {}\n";
    assert!(s.check(src).is_empty(), "nested_comment_ok: {:?}", s.check(src));
    let src = "#![expect(dead_code)]\n#![deny(dead_code)]\nfn unused() {}\nfn main() {}\n";
    assert_eq!(s.check(src).first().map(|d| d.line), Some(3), "expect_then_deny_err: {:?}", s.check(src));
    let src = "#![deny(warnings)]\n#![allow(dead_code)]\n#![warn(dead_code)]\nfn unused() {}\nfn main() {}\n";
    assert_eq!(s.check(src).first().map(|d| d.line), Some(4), "warnings_warn_err: {:?}", s.check(src));
    let src = "#![deny(dead_code)]\nenum E {\n    A,\n    B,\n}\nenum F {\n    B,\n}\nfn main() {\n    let _ = E::A;\n    let _ = F::B;\n}\n";
    assert_eq!(s.check(src).first().map(|d| d.line), Some(4), "owner_err: {:?}", s.check(src));
    let src = "struct Sz<T: Sized>(T);\nstruct J<K: Iterator<Item = u8> + ?Sized> {\n    s: Box<Sz<K>>,\n}\nfn main() {}\n";
    assert_eq!(s.check(src).first().map(|d| d.line), Some(3), "iter_unsized_err: {:?}", s.check(src));
}

/// Second review of #72: `forbid(warnings)` reaches only the lints left at warn (an `allow(dead_code)` after it
/// stands); `Fn() + ?Sized` / `FnOnce(u8) -> u8 + ?Sized` are still `?Sized`; `<<` in a discriminant is a shift, not
/// a type's arguments; `#[derive (PartialEq)]` is a derive; `self.f` in an `impl` of an alias reads the aliased
/// struct's field
#[test]
fn review72_second_round() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let src = "#![forbid(warnings)]\n#![allow(dead_code)]\nfn unused() {}\nfn main() {}\n";
    assert_eq!(first(src), None, "forbid_warnings_allow_ok");
    let src = "#![forbid(warnings)]\nfn unused() {}\nfn main() {}\n";
    assert_eq!(first(src).map(|d| d.0), Some(2), "forbid_warnings_err");
    let src = "struct Sz<T: Sized>(T);\nstruct J<K: Fn() + ?Sized> {\n    s: Box<Sz<K>>,\n}\nfn main() {}\n";
    assert_eq!(first(src).map(|d| (d.0, d.1.contains("`K: Sized`"))), Some((3, true)), "fn_unsized_err");
    let src = "struct Sz<T: Sized>(T);\nstruct J<K: FnOnce(u8) -> u8 + ?Sized> {\n    s: Box<Sz<K>>,\n}\nfn main() {}\n";
    assert_eq!(first(src).map(|d| d.0), Some(3), "fnonce_unsized_err");
    let src = "struct Sz<T: Sized>(T);\nstruct J<K: Fn() + Clone + ?Sized> {\n    s: Box<Sz<K>>,\n}\nfn main() {}\n";
    assert_eq!(first(src), None, "fn_clone_sized_ok");
    let src = "#![deny(dead_code)]\nenum E {\n    A = 1 << 0,\n    B,\n}\nfn main() {\n    let _ = E::A;\n}\n";
    assert_eq!(first(src), Some((4, "variant `B` is never constructed".to_string())), "shift_discr_err");
    let src = "#![deny(dead_code)]\nenum E {\n    A = 1 << 0,\n    B = 1 << 1,\n    C,\n}\nfn main() {\n    let _ = (E::A, E::B, E::C);\n}\n";
    assert_eq!(first(src), None, "shift_discr_ok");
    let src = "#![deny(dead_code)]\n#[derive (PartialEq)]\nstruct S {\n    value: u8,\n}\nfn main() {\n    let a = S { value: 1 };\n    let b = S { value: 2 };\n    let _ = a == b;\n}\n";
    assert_eq!(first(src), None, "spaced_derive_ok");
    let src = "#![deny(dead_code)]\nstruct S {\n    value: u8,\n}\ntype Alias = S;\nimpl Alias {\n    fn read(&self) -> u8 {\n        self.value\n    }\n}\nfn main() {\n    let s = S { value: 1 };\n    let _ = s.read();\n}\n";
    assert_eq!(first(src), None, "alias_impl_ok");
    // (third round: an arm's body in parentheses is an expression; `fn S()` beside `struct S { .. }` constructs no
    // S; an item's or a field's own warn is an error under the crate's `deny(warnings)`)
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let flag = true;\n    let _e = match flag {\n        true => (E::A),\n        false => (E::B),\n    };\n}\n";
    assert_eq!(first(src), None, "paren_arms_ok");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let _e = match 1 {\n        0 => Some((E::A)),\n        _ => Some(E::B),\n    };\n}\n";
    assert_eq!(first(src), None, "nested_paren_arm_ok");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn f(e: Option<E>) -> u8 {\n    match e {\n        Some(E::A) => 1,\n        _ => 0,\n    }\n}\nfn main() {\n    let _ = f(Some(E::B));\n}\n";
    assert_eq!(first(src), Some((2, "variant `A` is never constructed".to_string())), "pattern_in_some_err");
    let src = "#![deny(dead_code)]\n#![allow(non_snake_case)]\nstruct S {\n    other: u8,\n}\nstruct T {\n    value: u8,\n}\nfn S() -> T {\n    T { value: 1 }\n}\nfn main() {\n    let _ = S { other: 1 }.other;\n    let t = S();\n    let _ = t.value;\n}\n";
    assert_eq!(first(src), None, "fn_named_like_struct_ok");
    let src = "#![deny(warnings)]\n#[allow(dead_code)]\n#[warn(dead_code)]\nfn unused() {}\nfn main() {}\n";
    assert_eq!(first(src).map(|d| d.0), Some(4), "item_allow_then_warn_err");
    let src = "#![deny(dead_code)]\n#[warn(dead_code)]\nfn unused() {}\nfn main() {}\n";
    assert_eq!(first(src), None, "item_warn_ok");
    let src = "#![deny(warnings)]\nstruct S {\n    x: u8,\n    #[warn(dead_code)]\n    y: u8,\n}\nfn main() {\n    let s = S { x: 1, y: 2 };\n    let _ = s.x;\n}\n";
    assert_eq!(first(src), Some((5, "field `y` is never read".to_string())), "field_warn_warnings_err");
    let src = "#![deny(warnings)]\nmod m {\n    #[allow(unused_variables)]\n    fn _foo() {}\n}\nfn main() {}\n";
    assert_eq!(first(src), None, "underscore_in_module_ok");
    // (fourth round: an element of a group is what the group is (a struct literal's field, a call's argument, a
    // tuple pattern's element); a closure's body is an expression; `S { .. }.inner` binds no S)
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let make = || E::A;\n    let _ = make();\n    let id = |x: u8| E::B;\n    let _ = id(1);\n}\n";
    assert_eq!(first(src), None, "closure_body_ok");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nstruct Holder {\n    a: E,\n    b: u8,\n}\nfn main() {\n    let h = Holder { a: E::A, b: 0 };\n    let _ = (h.a, h.b, E::B);\n}\n";
    assert_eq!(first(src), None, "struct_field_ok");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn f(_: E, _: u8) {}\nfn main() {\n    f(E::A, 1);\n    let _ = [E::B, E::B];\n}\n";
    assert_eq!(first(src), None, "call_and_array_ok");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn g(t: (E, u8)) -> u8 {\n    match t {\n        (E::A, x) => x,\n        _ => 0,\n    }\n}\nfn main() {\n    let _ = g((E::B, 1));\n}\n";
    assert_eq!(first(src), Some((2, "variant `A` is never constructed".to_string())), "tuple_pattern_err");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nstruct Holder {\n    a: E,\n}\nfn h(x: Holder) -> u8 {\n    match x {\n        Holder { a: E::A } => 1,\n        _ => 0,\n    }\n}\nfn main() {\n    let _ = h(Holder { a: E::B });\n}\n";
    assert_eq!(first(src), Some((2, "variant `A` is never constructed".to_string())), "struct_pattern_err");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn k(e: E) -> u8 {\n    match e {\n        E::B | E::A => 1,\n    }\n}\nfn main() {\n    let _ = k(E::B);\n}\n";
    assert_eq!(first(src), Some((2, "variant `A` is never constructed".to_string())), "or_pattern_err");
    let src = "#![deny(dead_code)]\nstruct Inner {\n    value: u8,\n}\nstruct Outer {\n    inner: Inner,\n}\nfn main() {\n    let t = Outer { inner: Inner { value: 1 } }.inner;\n    let _ = t.value;\n}\n";
    assert_eq!(first(src), None, "projected_initializer_ok");
    // (fifth round, with patterns from the parsed program: `matches!`'s first argument is an expression, its second a
    // pattern; `!(..)` after `=` is no macro; a leading `|` of an or-pattern is a pattern's; `|` between two values
    // is a bitwise or; a closure's parameter shadowing a `let` gives no owner)
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let _ = matches!(E::A, E::B);\n    let _ = E::B;\n}\n";
    assert_eq!(first(src), None, "matches_first_ok");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let _ = matches!(E::A, E::B);\n}\n";
    assert_eq!(first(src), Some((2, "variant `B` is never constructed".to_string())), "matches_second_err");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let _ = !(E::A as u8 == 0);\n    let _ = E::B;\n}\n";
    assert_eq!(first(src), None, "not_operator_ok");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    match (E::A, 0) {\n        (| E::A | E::B, _) => (),\n    }\n}\n";
    assert_eq!(first(src), Some((2, "variant `B` is never constructed".to_string())), "leading_pipe_err");
    let src = "#![deny(dead_code)]\nenum E { A, B }\nfn main() {\n    let _ = E::A as u8 | E::B as u8;\n}\n";
    assert_eq!(first(src), None, "bitwise_or_ok");
    let src = "#![deny(dead_code)]\nstruct S {\n    other: u8,\n}\nstruct T {\n    value: u8,\n}\nfn main() {\n    let x = S { other: 1 };\n    let _ = x.other;\n    let f = |x: T| x.value;\n    let _ = f(T { value: 1 });\n}\n";
    assert_eq!(first(src), None, "closure_shadow_ok");
    // (sixth round: the parsed region runs through an impl's closing brace after the last member; a `matches!` guard
    // is an expression; a closure's parameter inside a macro's arguments is a binding too)
    let src = "#![deny(dead_code)]\nenum E {\n    A,\n}\nfn main() {\n    let _ = E::A;\n    E::ping();\n}\nimpl E {\n    fn ping() {}\n}\n";
    assert_eq!(first(src), None, "impl_last_ok");
    let src = "#![deny(dead_code)]\nenum E {\n    A,\n    B,\n}\nfn main() {\n    let _ = matches!(E::A, _ if E::B as u8 == 1);\n}\n";
    assert_eq!(first(src), None, "matches_guard_ok");
    let src = "#![deny(dead_code)]\nstruct S {\n    other: u8,\n}\nstruct T {\n    value: u8,\n}\nfn main() {\n    let x = S { other: 1 };\n    let t = T { value: 1 };\n    assert!((|x: T| x.value)(t) == x.other);\n}\n";
    assert_eq!(first(src), None, "closure_in_macro_ok");
}

/// E0308 where rustc points: the branch whose type differs from the ones before it (its tail, the last statement of
/// a block without one, the arm of an empty block), a loop body with a value (its tail), a struct literal's field (its
/// value, the name of a shorthand field)
#[test]
fn mismatch_at_the_branch_rustc_points_at() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut line = |src: &str| s.check(src).first().map(|d| d.line);
    let if_else = "fn main() {\n    let _ = if true {\n        1i32\n    } else {\n        2u32\n    };\n}\n";
    assert_eq!(line(if_else), Some(5));
    let semi = "fn main() {\n    let _ = match 3u8 {\n        0 => {\n            1i32\n        }\n        _ => {\n            0;\n        }\n    };\n}\n";
    assert_eq!(line(semi), Some(7));
    let empty = "fn f() {\n    match Some(0u32) {\n        Some(x) => {\n            x\n        }\n        None => {}\n    };\n}\nfn main() {}\n";
    assert_eq!(line(empty), Some(6));
    let lp = "fn main() {\n    let _a: i8 = loop {\n        1\n    };\n}\n";
    assert_eq!(line(lp), Some(3));
    let short = "struct Foo {\n    t: Thing,\n}\nstruct Thing;\nfn main() {\n    let t = &Thing;\n    let _f = Foo {\n        t\n    };\n}\n";
    assert_eq!(line(short), Some(8));
    // (no else: at the `if`, as rustc reports E0317)
    let no_else = "fn main() {\n    let _x: i32 = if true {\n        1\n    };\n}\n";
    assert_eq!(line(no_else), Some(2));
}

/// A condition (or iterator) that took the block meant to follow it: rustc's error at the `if` / `for`
#[test]
fn swallowed_block_at_the_keyword() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let bare = "fn a() {\n    if {}\n}\nfn main() {}\n";
    assert_eq!(first(bare), Some((2, "missing condition for `if` expression".to_string())));
    let unfinished = "fn main() {\n    let n = 1;\n    if n == {\n        println!(\"five\");\n    }\n}\n";
    assert_eq!(first(unfinished), Some((3, "this `if` expression is missing a block after the condition".to_string())));
    let for_in = "fn main() {\n    for i in {\n    }\n}\n";
    assert_eq!(first(for_in), Some((2, "missing expression to iterate on in `for` loop".to_string())));
    // (a block as the condition, followed by the block: fine)
    let ok = "fn main() {\n    if { true } {\n        println!(\"ok\");\n    }\n}\n";
    assert_eq!(first(ok), None);
}

/// E0658 for the attributes and ABIs behind a feature gate the source does not enable, with rustc's messages; none
/// when the source enables it
#[test]
fn feature_gated_attributes_and_abis() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let cases = [
        ("#[lang = \"foo\"]\nfn f() {}\nfn main() {}\n", 1, "lang items are subject to change", "lang_items"),
        ("#![needs_allocator]\nfn main() {}\n", 1, "the `needs_allocator` attribute is an experimental feature", "allocator_internals"),
        ("fn f() {}\n#[rustc_nonnull_optimization_guaranteed]\nstruct S {}\nfn main() {}\n", 2, "use of an internal attribute", "rustc_attrs"),
        ("#[stable(feature = \"a\", since = \"1.0.0\")]\nfn f() {}\nfn main() {}\n", 1, "stability attributes may not be used outside of the standard library", "staged_api"),
        ("#[repr(simd)]\nstruct F([f32; 4]);\nfn main() {}\n", 1, "SIMD types are experimental and possibly buggy", "repr_simd"),
        ("#[repr(transparent)]\nunion U {\n    f: u8,\n}\nfn main() {}\n", 2, "transparent unions are unstable", "transparent_unions"),
        ("#[doc(cfg(unix))]\nfn main() {}\n", 1, "the `doc(cfg)` and `doc(auto_cfg)` attributes are experimental", "doc_cfg"),
        ("extern \"rust-cold\" fn f() {}\nfn main() {}\n", 1, "the extern \"rust-cold\" ABI is experimental and subject to change", "rust_cold_cc"),
    ];
    for (src, line, msg, feat) in cases {
        assert_eq!(first(src), Some((line, format!("{msg} (E0658)"))), "{src}");
        let enabled = format!("#![feature({feat})]\n{src}");
        assert!(first(&enabled).is_none_or(|d| !d.1.contains("E0658")), "enabled: {enabled}");
    }
    // (a stable `doc` / `repr`: no gate)
    assert_eq!(first("#[doc = \"x\"]\n#[repr(C)]\nstruct S;\nfn main() {}\n"), None);
}

/// unused_imports reads a `use` past its outer attributes: `#[deprecated] #[cfg(not(FALSE))] use std::fs;` is unused;
/// a `cfg` that is false strips it, an allow of the lint leaves it alone
#[test]
fn unused_import_after_attributes() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let src = "#![deny(unused_imports)]\n#[deprecated]\n#[cfg(not(FALSE))]\nuse std::fs;\nfn main() {}\n";
    assert_eq!(first(src), Some((4, "unused import: `std::fs`".to_string())));
    assert_eq!(first("#![deny(unused_imports)]\n#[cfg(FALSE)]\nuse std::fs;\nfn main() {}\n"), None);
    assert_eq!(first("#![deny(unused_imports)]\n#[allow(unused_imports)]\nuse std::fs;\nfn main() {}\n"), None);
}

#[test]
fn trait_arity_at_impls() {
    // (E0107 at an impl of the program's trait with the wrong number of generic arguments; messages, lines and
    // columns as rustc 1.100 gives them. Lifetimes and defaulted parameters are counted as rustc does)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let src = "trait Seq {}\nimpl Seq<bool> for u32 {}\nfn main() {}\n";
    assert_eq!(first(src), Some((2, 6, "trait takes 0 generic arguments but 1 generic argument was supplied (E0107)".to_string())), "too_many");
    let src = "trait Tr<T> {}\nimpl Tr for u8 {}\nfn main() {}\n";
    assert_eq!(first(src), Some((2, 6, "missing generics for trait `Tr` (E0107)".to_string())), "missing");
    let src = "trait Tr<'a, T, U = u8> {}\nimpl<'a> Tr<'a, i32, u16, u32> for u8 {}\nfn main() {}\n";
    assert_eq!(first(src), Some((2, 10, "trait takes at most 2 generic arguments but 3 generic arguments were supplied (E0107)".to_string())), "at_most");
    let src = "fn main() {\n    trait Tr<T> {}\n    impl Tr<u8, u8> for u8 {}\n}\n";
    assert_eq!(first(src), Some((3, 10, "trait takes 1 generic argument but 2 generic arguments were supplied (E0107)".to_string())), "in_fn_body");
    let src = "trait Tr<'a, T, U = u8> {}\nimpl<'a> Tr<'a, i32> for u8 {}\nimpl<'a> Tr<'a, i32, u16> for u16 {}\nfn main() {}\n";
    assert_eq!(first(src), None, "lifetime_and_default_ok");
    let src = "trait It { type Item; }\nimpl It for u8 { type Item = u8; }\nfn main() {}\n";
    assert_eq!(first(src), None, "assoc_type_ok");
    // (review of #91: an `=` inside a bound is no default; a braced const argument's `<`)
    let src = "trait Tr<T: Iterator<Item = u8>> {}\nimpl Tr for u8 {}\nfn main() {}\n";
    assert_eq!(first(src), Some((2, 6, "missing generics for trait `Tr` (E0107)".to_string())), "eq_in_bound");
    let src = "trait Tr<const B: bool, T> {}\nimpl Tr<{ 0 < 1 }> for u8 {}\nfn main() {}\n";
    assert_eq!(first(src), Some((2, 6, "trait takes 2 generic arguments but 1 generic argument was supplied (E0107)".to_string())), "braced_const");
    // (second round: a braced comparison inside a bound does not hide the parameter's default)
    let src = "trait Flag<const B: bool> {}\ntrait Tr<T: Flag<{ 1 > 0 }> = u8> {}\nimpl Tr for () {}\nfn main() {}\n";
    assert_eq!(first(src).filter(|d| d.2.contains("E0107")), None, "braced_bound_default");
}

#[test]
fn review91_attributes_imports_unreachable() {
    // (review of #91, each checked with rustc 1.100)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    // (unused_imports: a `]` in an attribute's string; `expect(unused_must_use)` is another lint; a name another
    // `use` starts with is used)
    let src = "#![deny(unused_imports)]\n#[doc = \"]\"]\nuse std::fs as unused_fs;\nfn main() {}\n";
    assert_eq!(first(src), Some((3, "unused import: `std::fs as unused_fs`".to_string())), "bracket_in_string");
    let src = "#![deny(unused_imports)]\n#[expect(unused_must_use)]\nuse std::fs as unused_fs;\nfn main() {}\n";
    assert_eq!(first(src), Some((3, "unused import: `std::fs as unused_fs`".to_string())), "other_lint");
    let src = "#![deny(unused_imports)]\n#[deprecated]\nuse std::fs as files;\nuse files::File;\nfn main() { let _: Option<File> = None; }\n";
    // (zustc does not yet resolve `File` through the alias; only the lint is checked here)
    assert_eq!(first(src).filter(|d| d.1.starts_with("unused import")), None, "used_by_use");
    // (fourth round: the last lint attribute that names the lint or its group sets the level, and a warn is not an
    // error; a `*` in an attribute is not a glob)
    let unused_fs = Some((2, "unused import: `std::fs`".to_string()));
    for (attrs, want) in [
        ("#[warn(unused_imports)]", None),
        ("#[warn(unused)]", None),
        ("#[allow(unused)] #[warn(unused_imports)]", None),
        ("#[warn(unused_imports)] #[allow(unused)]", None),
        ("#[deny(unused_imports)] #[allow(unused_imports)]", None),
        ("#[allow(unused_imports)] #[deny(unused_imports)]", unused_fs.clone()),
        ("#[allow(unused_imports, warnings)] #[forbid(unused_imports)]", unused_fs.clone()),
        ("#[allow(warnings)]", unused_fs.clone()),
    ] {
        let src = format!("#![deny(unused_imports)]\n{attrs} use std::fs;\nfn main() {{}}\n");
        assert_eq!(first(&src), want, "lint_level {attrs}");
    }
    // (fifth round: a warn is still an error where `warnings` is denied, by the crate or the item)
    for (krate, attrs, want) in [
        ("#![deny(warnings)]", "#[warn(unused_imports)]", unused_fs.clone()),
        ("#![deny(warnings)]", "#[warn(unused)]", unused_fs.clone()),
        ("#![deny(warnings)]", "", unused_fs.clone()),
        ("#![deny(warnings)]", "#[allow(warnings)]", None),
        ("#![deny(warnings)]", "#[allow(warnings)] #[warn(unused_imports)]", None),
        ("#![deny(warnings)]", "#[warn(unused_imports)] #[allow(warnings)]", None),
        ("#![deny(warnings)]", "#[warn(warnings)] #[warn(unused_imports)]", None),
        ("#![deny(unused_imports)]", "#[warn(unused_imports)] #[deny(warnings)]", unused_fs.clone()),
    ] {
        let src = format!("{krate}\n{attrs} use std::fs;\nfn main() {{}}\n");
        assert_eq!(first(&src), want, "warnings_level {krate} {attrs}");
    }
    let src = "#![deny(unused_imports)]\n#[doc = \"*\"] use std::fs as unused_fs;\nfn main() {}\n";
    assert_eq!(first(src), Some((2, "unused import: `std::fs as unused_fs`".to_string())), "star_in_attribute");
    // (the unstable ABIs: after any number of attributes; a raw string)
    let src = "#[doc = \"a\"]\n#[doc = \"b\"]\n#[doc = \"c\"]\n#[allow(dead_code, unused)]\npub extern \"rust-cold\" fn f() {}\nfn main() {}\n";
    assert_eq!(first(src), Some((5, "the extern \"rust-cold\" ABI is experimental and subject to change (E0658)".to_string())), "abi_after_attrs");
    let src = "extern r\"rust-cold\" fn f() {}\nfn main() {}\n";
    assert_eq!(first(src), Some((1, "the extern \"rust-cold\" ABI is experimental and subject to change (E0658)".to_string())), "raw_abi");
    // (repr(transparent): the item it is on, past a raw-string attribute and `pub(crate)`; no slice inside a char)
    let src = "#[repr(transparent)]\n#[doc = r\"note\"]\nstruct Wrap(u8);\nunion Slot { x: u8 }\nfn main() { let _ = Wrap(1); let _ = Slot { x: 1 }; }\n";
    assert_eq!(first(src), None, "transparent_struct_then_union");
    let src = "#[repr(transparent)]\npub(crate) union U { x: u8 }\nfn main() {}\n";
    assert_eq!(first(src), Some((2, "transparent unions are unstable (E0658)".to_string())), "transparent_pub_crate_union");
    let src = format!("#[repr(transparent)]\n#[doc = \"{}\"]\npub union U {{ x: u8 }}\nfn main() {{}}\n", "a".repeat(600));
    assert_eq!(first(&src), Some((3, "transparent unions are unstable (E0658)".to_string())), "transparent_after_long_attribute");
    // (a window that ends inside `union`: grown until the name is whole)
    for n in 470..490 {
        let src = format!("#[repr(transparent)]\n#[doc = \"{}\"]\nunion U {{ x: u8 }}\nfn main() {{}}\n", "a".repeat(n));
        assert_eq!(first(&src), Some((3, "transparent unions are unstable (E0658)".to_string())), "transparent_window_edge_{n}");
    }
    for n in 480..510 {
        for vis in ["", "pub ", "pub(crate) "] {
            let src = format!("#[repr(transparent)]\n#[doc = \"{}\"]\n{vis}union U {{ x: u8 }}\nfn main() {{}}\n", "a".repeat(n));
            assert_eq!(first(&src), Some((3, "transparent unions are unstable (E0658)".to_string())), "transparent_window_edge_{vis}{n}");
        }
    }
    let pad = "é".repeat(300);
    let src = format!("#[repr(transparent)]\nstruct W(u8);\nfn main() {{ let _s = \"{pad}\"; let _ = W(1); }}\n");
    for k in 0..4 {
        let src = format!("{}{src}", " ".repeat(k));
        assert_eq!(first(&src), None, "multibyte_after_{k}");
    }
    // (unreachable_code: the program's own `process::exit` returns)
    let src = "#![deny(unreachable_code)]\nmod process { pub fn exit() -> u8 { 1 } }\nfn take(_: u8, _: u8) {}\nfn main() { take(process::exit(), 2); }\n";
    // (zustc resolves `process::exit` to std's here, a separate gap; only the lint is checked)
    assert_eq!(first(src).filter(|d| d.1.starts_with("unreachable")), None, "user_process_exit");
    // (third round: `mod` and `process` apart; std's `process::exit` still diverges beside the program's module)
    let src = "#![deny(unreachable_code)]\nmod\n    process { pub fn exit() -> u8 { 1 } }\nfn take(_: u8, _: u8) {}\nfn main() { take(process::exit(), 2); }\n";
    assert_eq!(first(src).filter(|d| d.1.starts_with("unreachable")), None, "user_process_split");
    let src = "#![deny(unreachable_code)]\nmod process { pub fn exit() -> u8 { 1 } }\nfn take(_: u8, _: u8) {}\nfn main() { take(std::process::exit(0), 2); }\n";
    assert_eq!(first(src), Some((4, "unreachable expression".to_string())), "std_process_exit");
}

#[test]
fn fn_item_cannot_capture() {
    // (E0434 at the use, at rustc 1.100's line and column; a nested fn's own parameters, the program's items and
    // closures are left alone)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let e = |l: usize, c: usize| Some((l, c, "can't capture dynamic environment in a fn item (E0434)".to_string()));
    assert_eq!(first("fn foo() {\n    let y = 5;\n    fn bar() -> u32 {\n        y\n    }\n}\nfn main() {}\n"), e(4, 9), "let");
    assert_eq!(first("fn main() {\n    let bar: isize = 5;\n    fn foo() -> isize { return bar; }\n}\n"), e(3, 32), "one_line");
    assert_eq!(first("fn foo(x: isize) {\n    fn mth() {\n        fn bar() -> isize { x }\n    }\n}\nfn main() { foo(2); }\n"), e(3, 29), "two_levels");
    assert_eq!(first("struct A;\nimpl A {\n    fn m(&self) {\n        fn x() {\n            self.m()\n        }\n    }\n}\nfn main() {}\n"), e(5, 13), "self");
    let ok = "const N: u32 = 7;\nfn main() {\n    let x = 1u32;\n    let n2 = 2;\n    fn inner(x: u32) -> u32 { x + N }\n    let c = || x + n2;\n    println!(\"{} {}\", inner(x), c());\n}\n";
    assert_eq!(first(ok), None, "own_params_items_closures_ok");
}

#[test]
fn visibility_where_it_means_nothing() {
    // (E0449 at the `pub`, at rustc 1.100's line and column; a visibility where it means something is left alone)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let e = |l: usize, c: usize| Some((l, c, "visibility qualifiers are not permitted here (E0449)".to_string()));
    assert_eq!(first("struct B;\npub impl B {}\nfn main() {}\n"), e(2, 1), "impl");
    assert_eq!(first("trait A { fn f(&self); }\nstruct B;\nimpl A for B {\n    #[inline]\n    pub fn f(&self) {}\n}\nfn main() {}\n"), e(5, 5), "trait_impl_item");
    assert_eq!(first("trait A {\n    pub const N: u8;\n}\nfn main() {}\n"), e(2, 5), "trait_item");
    assert_eq!(first("pub extern \"C\" {\n    pub fn bar();\n}\nfn main() {}\n"), e(1, 1), "extern_block");
    assert_eq!(first("enum Foo {\n    V1 { pub f: i32 },\n}\nfn main() { let _ = Foo::V1 { f: 1 }; }\n"), e(2, 10), "variant_field");
    assert_eq!(first("enum Foo {\n    V2(pub i32),\n}\nfn main() { let _ = Foo::V2(1); }\n"), e(2, 8), "tuple_variant_field");
    let ok = "#[derive(Clone, Debug, PartialEq)]\npub enum E { A(u8), B { x: u8 } }\npub struct S { pub x: u8 }\nimpl S {\n    pub fn new() -> S { S { x: 1 } }\n}\npub trait T { fn f(&self) -> u8; }\nimpl T for S { fn f(&self) -> u8 { self.x } }\npub extern \"C\" fn g() {}\nfn main() { let s = S::new(); let _ = (s.f(), E::A(1).clone(), E::B { x: 2 }); g(); }\n";
    assert_eq!(first(ok), None, "valid_visibility");
}

#[test]
fn dyn_without_associated_types() {
    // (E0191 at a trait object written in a type that leaves an associated type of its trait, or of a supertrait,
    // without a value, as rustc 1.100 reports it)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let e = |line: usize, col: usize, what: &str| Some((line, col, format!("the value of the associated {what} must be specified (E0191)")));
    let src = "trait Trait {\n    type Bar;\n}\ntype Foo = dyn Trait;\nfn main() {}\n";
    assert_eq!(first(src), e(4, 16, "type `Bar` in `Trait`"), "type_alias");
    let src = "trait Foo { type A; fn dummy(&self) {} }\nfn bar(_x: &dyn Foo) {}\nfn main() {}\n";
    assert_eq!(first(src), e(2, 17, "type `A` in `Foo`"), "parameter");
    let src = "trait Tr { type A; type B; }\nfn f(_: &dyn Tr<A = u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), e(2, 14, "type `B` in `Tr`"), "one_bound");
    let src = "trait T3 { type A; type B; type C; }\nfn f(_: &dyn T3) {}\nfn main() {}\n";
    assert_eq!(first(src), e(2, 14, "types `A`, `B` and `C` in `T3`"), "three_in_one_trait");
    // (several traits: each `X` in `Tr`, sorted)
    let src = "trait Sub: Super + Zed { type Y; }\ntrait Zed { type Z; }\ntrait Super { type X; }\nfn f(_: &dyn Sub) {}\nfn main() {}\n";
    assert_eq!(first(src), e(4, 14, "types `X` in `Super`, `Y` in `Sub`, `Z` in `Zed`"), "supertraits");
    // (a binding on the object goes to its own trait first; a supertrait's binding in the trait's header counts,
    // unless it names `Self`)
    let src = "pub trait Super { type X; }\npub trait Sub: Super { type X; }\nfn f<T>(_: Box<dyn Sub<X = T>>) {}\nfn main() {}\n";
    assert_eq!(first(src), e(3, 20, "type `X` in `Super`"), "shadowed");
    let src = "trait Super { type X; }\ntrait Sub: Super<X = u8> { type Y; }\nfn f(_: &dyn Sub<Y = u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "bound_in_header");
    let src = "trait Base { type Output; }\ntrait Helper: Base<Output = <Self as Helper>::Target> { type Target; }\nfn f(_: Box<dyn Helper<Target = i32>>) {}\nfn main() {}\n";
    assert_eq!(first(src), e(3, 17, "type `Output` in `Base`"), "self_in_header_binding");
    // (the trait's own arguments as written)
    let src = "trait G<T> { type A; }\nfn f<'a>(_: Box<dyn G<&'a str> + Send + 'a>) {}\nfn main() {}\n";
    assert_eq!(first(src), e(2, 21, "type `A` in `G<&'a str>`"), "arguments_shown");
    // (std's traits; a path to std's beside the program's trait of the name)
    let src = "fn f(_: &dyn Iterator) {}\nfn main() {}\n";
    assert_eq!(first(src), e(1, 14, "type `Item` in `Iterator`"), "std_iterator");
    let src = "fn f(_: &dyn DoubleEndedIterator) {}\nfn main() {}\n";
    assert_eq!(first(src), e(1, 14, "type `Item` in `Iterator`"), "std_supertrait");
    let src = "fn g(_: Box<dyn std::ops::Deref>) {}\nfn main() {}\n";
    assert_eq!(first(src), e(1, 17, "type `Target` in `Deref`"), "std_path");
    let src = "trait Iterator { fn go(&self); }\nfn f(_: &dyn Iterator) {}\nfn g(_: &dyn std::iter::Iterator) {}\nfn main() {}\n";
    assert_eq!(first(src), e(3, 14, "type `Item` in `std::iter::Iterator`"), "own_trait_of_std_name");
    // (before E0038, as rustc: `fn new() -> Self` alone does not hide it)
    let src = "trait Tr { type A; fn new() -> Self; }\nfn f(_: &dyn Tr) {}\nfn main() {}\n";
    assert_eq!(first(src), e(2, 14, "type `A` in `Tr`"), "before_e0038");
    // (left alone: every type bound, `where Self: Sized`, `Fn(..)` sugar, a generic associated type (E0038), a
    // `dyn` in a string or a comment, a name that only contains `dyn`)
    let src = "trait S { type A where Self: Sized; type B; fn g(&self) {} }\nfn f(_: &dyn S<B = u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "where_self_sized");
    let src = "fn f(_: &dyn Fn(u8) -> u8, _: Box<dyn Iterator<Item = u8> + Send>, _: &dyn IntoIterator<Item = u8, IntoIter = std::vec::IntoIter<u8>>) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "all_bound");
    let src = "trait Ga { type A<'a>; }\nfn f(_: &dyn Ga) {}\nfn main() {}\n";
    assert!(first(src).is_none_or(|d| !d.2.contains("E0191")), "gat");
    let src = "trait Tr { type A; }\n// dyn Tr\nfn dyn_tr() -> &'static str { \"dyn Tr\" }\nfn main() { let _ = dyn_tr(); }\n";
    assert_eq!(first(src), None, "string_and_comment");
    // (a bound that implies Sized; a supertrait bound by two paths, one type each (rust-lang/rust#102933))
    let src = "trait Other: Sized {}\ntrait Boo { type Assoc where Self: Other; }\nfn boo(_: &dyn Boo) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "implies_sized");
    let src = "pub trait Service { type Response; type Fut; }\npub trait A1: Service<Response = i32> {}\npub trait A2: Service<Fut = u8> + A1 { fn foo(&self) {} }\nfn f(_: &dyn A2) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "bound_by_two_paths");
    // (a binding two supertraits could take binds neither (E0222 too); a wrong count of arguments is E0107 alone)
    let src = "pub trait Vehicle { type Color; }\npub trait Bx { type Color; }\npub trait BoxCar: Bx + Vehicle {}\nfn d<C>(_: &dyn BoxCar<Color = C>) {}\nfn main() {}\n";
    assert_eq!(first(src), e(4, 17, "types `Color` in `Bx`, `Color` in `Vehicle`"), "ambiguous_binding");
    let src = "trait T<X, Y = u8> { type A; }\nfn f(_: &dyn T<u8, A = u8>) {}\nfn g(_: &dyn T<u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), e(3, 14, "type `A` in `T<u8>`"), "defaulted_parameter");
    let src = "pub trait T<X, Y> { type A; type B; type C; }\npub struct Foo { i: Box<dyn T<usize, usize, usize, usize, B = usize>> }\nfn main() {}\n";
    assert_eq!(first(src), Some((2, 29, "trait takes 2 generic arguments but 4 generic arguments were supplied (E0107)".to_string())), "too_many_arguments");
}

#[test]
fn review100_capture_visibility_dyn_gat() {
    // (each expected line, column and message is rustc 1.100's on the same source; None where rustc accepts it)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    // (E0449: past a nested comment; `pub(crate)` and `unsafe` before `extern`; not an item in a discriminant)
    let vis = "visibility qualifiers are not permitted here (E0449)";
    let src = "trait T { fn f(&self); }\nstruct S;\nimpl T for S { #[inline] /* a /* b */ c */ pub fn f(&self) {} }\nfn main() {}\n";
    assert_eq!(first(src), at(3, 44, vis), "nested_comment");
    let src = "pub(crate) unsafe extern \"C\" { fn f(); }\npub unsafe extern \"C\" { fn g(); }\nfn main() {}\n";
    assert_eq!(first(src), at(1, 1, vis), "pub_crate_unsafe_extern");
    let src = "unsafe extern \"C\" { fn f(); }\npub unsafe extern \"C\" { fn g(); }\nfn main() {}\n";
    assert_eq!(first(src), at(2, 1, vis), "pub_unsafe_extern");
    let src = "enum E { A = { pub const N: isize = 1; N } }\nfn main() { let _ = E::A; }\n";
    assert_eq!(first(src), None, "discriminant_block");
    let src = "enum E { A = { pub const N: isize = 1; N }, B(pub u8) }\nfn main() {}\n";
    assert_eq!(first(src), at(1, 47, vis), "field_after_discriminant");
    // (E0434: at the free use, not one a nested block or a closure binds; not a sibling fn's parameter on the same
    // line; columns in characters)
    let cap = "can't capture dynamic environment in a fn item (E0434)";
    let src = "fn outer(x: u32) -> u32 {\n    fn inner() -> u32 { { let x = 1u32; let _ = x; } x }\n    x\n}\nfn main() { outer(1); }\n";
    assert_eq!(first(src), at(2, 54, cap), "free_use_after_block");
    let src = "fn outer() {\n    let s = 1u8;\n    fn inner() -> u8 { let f = |s: u8| s; f(1) + s }\n}\nfn main() { outer(); }\n";
    assert_eq!(first(src), at(3, 50, cap), "free_use_after_closure");
    let src = "    fn unrelated(drop: u8) { let _ = drop; } fn outer() { fn inner() { drop(0u8); } inner(); }\nfn main() { unrelated(1); outer(); }\n";
    assert_eq!(first(src), None, "sibling_on_one_line");
    let src = "fn main() {\n    let x = 1u8;\n    fn inner() -> u8 { let _s = \"ああああああああああああああ\"; x }\n}\n";
    assert_eq!(first(src), at(3, 51, cap), "non_ascii_first_line");
    // (E0191: a turbofish; a `<..>` far after the name; the principal trait after auto traits and a lifetime; each
    // instantiation of a supertrait; `where Self: Base` as a supertrait)
    let src = "fn f(_: &dyn Iterator::<Item = u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "turbofish");
    let src = format!("fn f(_: &dyn Iterator{}<Item = u8>) {{}}\nfn main() {{}}\n", " ".repeat(49));
    assert_eq!(first(&src), None, "far_arguments");
    let src = "fn f(_: Box<dyn Fn(u8) -> u8 + Send>, _: &dyn Send, _: &(dyn Sync + Send)) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "auto_traits_alone");
    let item = |what: &str| format!("the value of the associated {what} must be specified (E0191)");
    let src = "type Obj = dyn Send + Iterator;\nfn main() {}\n";
    assert_eq!(first(src), at(1, 23, &item("type `Item` in `Iterator`")), "after_auto_trait");
    let src = "fn f<'a>(_: &(dyn 'a + Iterator)) {}\nfn main() {}\n";
    assert_eq!(first(src), at(1, 24, &item("type `Item` in `Iterator`")), "after_lifetime");
    let src = "trait Sup<T> { type Assoc: Default; }\nimpl<T: Default> Sup<T> for () { type Assoc = T; }\nimpl<T: Default, U: Default> Dyn<T, U> for () {}\ntrait Dyn<A, B>: Sup<A, Assoc = A> + Sup<B> {}\nfn main() { let _q: <dyn Dyn<i32, u32> as Sup<u32>>::Assoc = Default::default(); }\n";
    assert_eq!(first(src), at(5, 26, &item("type `Assoc` in `Sup<u32>`")), "per_instantiation");
    let src = "trait Sup<T> { type Assoc; }\ntrait Dyn<A>: Sup<A, Assoc = A> + Sup<u8, Assoc = u8> {}\nfn f(_: &dyn Dyn<i32>) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "both_instantiations_bound");
    let src = "trait Base { type A; }\ntrait Sub where Self: Base {}\nfn f(_: &dyn Sub) {}\nfn main() {}\n";
    assert_eq!(first(src), at(3, 14, &item("type `A` in `Base`")), "where_self");
    let src = "trait Base { type A; }\ntrait Other {}\ntrait Sub where Self: Other, Self: Fn(u8, u8) + Base {}\nfn f(_: &dyn Sub) {}\nfn main() {}\n";
    assert_eq!(first(src), at(4, 14, &item("type `A` in `Base`")), "where_self_clauses");
    let src = "trait Base { type A; }\ntrait Sub where Self: Base<A = u8> {}\nfn f(_: &dyn Sub) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "where_self_bound");
    // (a supertrait walk cut short at 64 traits says nothing about the ones it did not reach: here the binding of
    // `Base::A` is past the cut, and rustc accepts the object)
    let mut src = String::from("trait Base { type A; }\ntrait T0: Base + T1 {}\n");
    for i in 1..70 {
        src += &format!("trait T{i}: T{} {{}}\n", i + 1);
    }
    src += "trait T70: Base<A = u8> {}\nfn f(_: &dyn T0) {}\nfn main() {}\n";
    assert_eq!(first(&src), None, "long_chain");
    // (E0107: a GAT's turbofish; a qualified path whose trait has arguments; a dyn's count of trait arguments)
    let src = "trait Lend { type Item<'a> where Self: 'a; fn next<'a>(&'a mut self) -> Self::Item::<'a>; }\nfn main() {}\n";
    assert_eq!(first(src), None, "gat_turbofish");
    let src = "trait Tr<T> { type Item<'a>; }\nfn f<U: Tr<u8>>() { let _: Option<<U as Tr<u8>>::Item> = None; }\nfn main() {}\n";
    assert_eq!(first(src), at(2, 50, "missing generics for associated type `Tr::Item` (E0107)"), "qualified_trait_arguments");
    let src = "trait T<X> { type A; }\nfn f(_: &dyn T<A = u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), at(2, 14, "trait takes 1 generic argument but 0 generic arguments were supplied (E0107)"), "dyn_binding_only");
    let src = "trait T<X> { type A; }\nfn f(_: &dyn T) {}\nfn main() {}\n";
    assert_eq!(first(src), at(2, 14, "missing generics for trait `T` (E0107)"), "dyn_no_arguments");
    let src = "trait T<X, Y = u8> { type A; }\nfn f(_: &dyn T<u8, u8, u8, A = u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), at(2, 14, "trait takes at most 2 generic arguments but 3 generic arguments were supplied (E0107)"), "dyn_too_many");
}

#[test]
fn malformed_attribute_input() {
    // (a built-in attribute in a shape rustc cannot read, as rustc 1.100 reports it; a `cfg` predicate is read from
    // the source as written, as the configuring takes the attributes away)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let lit = |found: &str| format!("expected a literal (`1u8`, `1.0f32`, `\"string\"`, etc.) here, found {found}");
    let cases: &[(&str, Option<String>)] = &[
        // (deprecated: known keys with string values, each once)
        ("#[deprecated(since = \"a\", note = \"a\", reason)]\nfn f() {}\nfn main() { }\n", Some("malformed `deprecated` attribute input".into())),
        ("#[deprecated(reason = \"x\")]\nfn f() {}\nfn main() { }\n", Some("malformed `deprecated` attribute input".into())),
        ("#[deprecated(\"since\")]\nfn f() {}\nfn main() { }\n", Some("malformed `deprecated` attribute input".into())),
        ("#[deprecated(since = b\"1.29\", note = \"hi\")]\nfn f() {}\nfn main() { }\n", Some("malformed `deprecated` attribute input".into())),
        ("#[deprecated(since = \"a\", since = \"b\", note = \"c\")]\nfn f() {}\nfn main() { }\n", Some("malformed `deprecated` attribute input".into())),
        ("#[deprecated(note = test)]\nfn f() {}\nfn main() { }\n", Some(lit("expression"))),
        ("#[deprecated(note = concat!(\"a\", \"b\"))]\nfn f() {}\nfn main() { }\n", Some(lit("macro call"))),
        ("#[deprecated(since = r\"1.0\", note = \"a, b\")]\nfn f() {}\n#[deprecated()]\nfn g() {}\nfn main() { }\n", None),
        // (repr, windows_subsystem, link)
        ("#[repr(\"C\")]\nstruct A {}\nfn main() {}\n", Some("malformed `repr` attribute input".into())),
        ("#[repr(C, align(8))]\nstruct A { x: u8 }\nfn main() { let _ = A { x: 1 }.x; }\n", None),
        ("#![windows_subsystem = \"wrong\"]\nfn main() {}\n", Some("malformed `windows_subsystem` attribute input".into())),
        ("#![windows_subsystem = \"console\"]\nfn main() {}\n", None),
        ("#[link(kind = \"wonderful_unicorn\")] extern \"C\" {}\nfn main() {}\n", Some("malformed `link` attribute input".into())),
        // (cfg predicates: a name, `name = \"value\"`, all / any of any number, not of one)
        ("#[cfg(foo(bar))]\nfn check() {}\nfn main() {}\n", Some("malformed `cfg` attribute input".into())),
        ("fn main() {}\n#[cfg(not())]\nfn f() {}\n", Some("malformed `cfg` attribute input".into())),
        ("fn main() {}\n#[cfg(a, b)]\nfn f() {}\n", Some("malformed `cfg` attribute input".into())),
        ("fn main() {}\n#[cfg(foo = 1)]\nfn f() {}\n", Some("malformed `cfg` attribute input".into())),
        ("fn main() {}\n#[cfg(feature = -1)]\nfn f() {}\n", Some(lit("expression"))),
        ("fn main() {}\n#[cfg(target-os = \"windows\")]\nfn f() {}\n", Some("expected one of `(`, `,`, `::`, or `=`, found `-`".into())),
        ("#![cfg_attr(a())]\nfn main() {}\n", Some("malformed `cfg_attr` attribute input".into())),
        ("#[cfg_attr(foo(bar), inline)]\nfn f() {}\nfn main() {}\n", Some("malformed `cfg_attr` attribute input".into())),
        ("#[cfg(all())]\nfn f() {}\n#[cfg(not(any()))]\nfn g() {}\n#[cfg(all(unix,))]\nfn h() {}\n#[cfg(feature = \"a\")]\nfn i() {}\nfn main() {}\n", None),
        // (cfg! of one predicate)
        ("fn main() {\n    let _ = cfg!();\n}\n", Some("macro requires a cfg-pattern as an argument".into())),
        ("fn main() {\n    if cfg!(not()) { }\n}\n", Some("malformed `cfg` macro input".into())),
        ("fn main() {\n    let _ = cfg!(a, b);\n}\n", Some("expected 1 cfg-pattern".into())),
        // (not one in a comment, a string or a macro's pattern)
        ("// #[cfg(foo(bar))]\nfn main() { let _ = \"#[cfg(foo(bar))]\"; let _ = cfg!(unix); }\nmacro_rules! m { ($p:meta) => { #[cfg($p)] fn x() {} } }\n", None),
    ];
    for (src, want) in cases {
        let got = first(src);
        match want {
            Some(m) => assert_eq!(got.as_ref().map(|d| d.1.as_str()), Some(m.as_str()), "{src}"),
            None => assert_eq!(got, None, "{src}"),
        }
    }
}

#[test]
fn gat_arity_at_uses() {
    // (E0107 at a use of a generic associated type without its arguments or with the wrong number of lifetimes,
    // as rustc 1.100 reports them; valid uses, bindings with arguments and other traits' names are left alone)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let src = "trait StreamingIterator {\n    type Item<'a>;\n    fn next(&mut self) -> Option<Self::Item>;\n}\nfn main() {}\n";
    assert_eq!(first(src), Some((3, 40, "missing generics for associated type `StreamingIterator::Item` (E0107)".to_string())), "self_path");
    let src = "trait Provider {\n    type A<'a>;\n}\nstruct Holder<B> {\n    inner: Box<dyn Provider<A = B>>,\n}\nfn main() {}\n";
    assert_eq!(first(src), Some((5, 29, "missing generics for associated type `Provider::A` (E0107)".to_string())), "binding");
    let src = "trait Foo {\n    type Assoc<'a, const N: usize>;\n}\nfn foo<T: Foo>() {\n    let _: <T as Foo>::Assoc<3>;\n}\nfn main() {}\n";
    assert_eq!(first(src), Some((5, 24, "associated type takes 1 lifetime argument but 0 lifetime arguments were supplied (E0107)".to_string())), "qualified_lifetime");
    let src = "trait Lend {\n    type Item<'a> where Self: 'a;\n    fn next<'a>(&'a mut self) -> Option<Self::Item<'a>>;\n}\nstruct W(Vec<u8>);\nimpl Lend for W {\n    type Item<'a> = &'a u8;\n    fn next<'a>(&'a mut self) -> Option<Self::Item<'a>> { self.0.first() }\n}\nfn take<T: for<'a> Lend<Item<'a> = &'a u8>>(t: &mut T) -> u8 { *t.next().unwrap() }\nfn main() {\n    let v: Vec<u8> = vec![1];\n    let _: Box<dyn Iterator<Item = &u8>> = Box::new(v.iter());\n    let _ = take(&mut W(vec![3]));\n}\n";
    assert_eq!(first(src), None, "valid_uses_ok");
}

#[test]
fn unsafe_attributes() {
    // (`#[unsafe(..)]` around an attribute that is not an unsafe one, the keyword where a path goes, and an unsafe
    // attribute without `unsafe` where that is an error, as rustc 1.100 reports them)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    let f = "pub extern \"C\" fn f() {}\nfn main() {}\n";
    for a in ["repr(C)", "inline", "link_name = \"x\"", "doc = \"x\"", "diagnostic::on_unimplemented(message = \"m\")"] {
        let name = a.split(['(', ' ']).next().unwrap();
        assert_eq!(check(2021, &format!("#[unsafe({a})]\n{f}")), at(1, &format!("`{name}` is not an unsafe attribute")), "{a}");
    }
    for a in ["no_mangle", "export_name = \"x\"", "link_section = \".x\""] {
        assert_eq!(check(2021, &format!("#[unsafe({a})]\n{f}")), None, "unsafe_{a}");
        assert_eq!(check(2021, &format!("#[{a}]\n{f}")), None, "bare_2021_{a}");
        assert_eq!(check(2024, &format!("#[{a}]\n{f}")), at(1, "unsafe attribute used without unsafe"), "bare_2024_{a}");
    }
    assert_eq!(check(2015, &format!("#![deny(rust_2024_compatibility)]\n#[no_mangle]\n{f}")), at(2, "unsafe attribute used without unsafe"), "denied_lint");
    assert_eq!(check(2021, "#[unsafe(unsafe(no_mangle))]\nfn a() {}\nfn main() {}\n"), at(1, "expected identifier, found keyword `unsafe`"), "double");
    assert_eq!(check(2021, "#[derive(unsafe(Debug))]\nstruct Foo;\nfn main() {}\n"), at(1, "expected identifier, found keyword `unsafe`"), "in_derive");
    assert_eq!(check(2021, "#[derive(Debug, unsafety)]\nstruct Foo;\nfn main() {}\n").is_some_and(|d| d.1.contains("keyword")), false, "a_name_starting_with_unsafe");
    assert_eq!(check(2024, "#[unsafe(does_not_exist)]\nfn aa() {}\nfn main() {}\n"), at(1, "cannot find attribute `does_not_exist` in this scope"), "unknown_first");
}

#[test]
fn proc_macro_attributes_outside_a_proc_macro_crate() {
    // (a proc macro's attribute in a crate that is not `proc-macro`, as rustc 1.100 reports it: on a fn, the crate
    // type; on another item, the item's kind. A comment that names the crate type does not make the crate one)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    for a in ["proc_macro", "proc_macro_derive", "proc_macro_attribute"] {
        let input = if a == "proc_macro_derive" { "(Foo)" } else { "" };
        let src = format!("// `proc-macro`\n#[{a}{input}]\npub fn f(a: u8) -> u8 {{ a }}\nfn main() {{}}\n");
        assert_eq!(first(&src), at(2, &format!("the `{a}` attribute is only usable with crates of the `proc-macro` crate type")), "{a}_fn");
        let src = format!("#[{a}{input}]\npub struct S;\nfn main() {{}}\n");
        assert_eq!(first(&src), at(1, &format!("the `{a}` attribute cannot be used on structs")), "{a}_struct");
    }
}

#[test]
fn review100_second_round() {
    // (each expected line, column and message is rustc 1.100's on the same source; None where rustc accepts it)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    let item = |what: &str| format!("the value of the associated {what} must be specified (E0191)");
    let cap = "can't capture dynamic environment in a fn item (E0434)";
    // (E0449: a comment inside `pub(..)`)
    let src = "pub(in crate /* ) */) unsafe extern \"C\" { fn f(); }\nfn main() {}\n";
    assert_eq!(first(src), at(1, 1, "visibility qualifiers are not permitted here (E0449)"), "comment_in_restricted_visibility");
    // (E0434: a format string's capture, and `matches!`'s operands)
    let src = "fn outer(x: u8) {\n    fn inner() {\n        let _ = format!(\"{x}\");\n    }\n}\nfn main() { outer(1); }\n";
    assert_eq!(first(src), at(3, 27, cap), "format_capture");
    let src = "fn outer(x: u8) {\n    fn inner() -> bool {\n        matches!(\n            x, 1)\n    }\n}\nfn main() { outer(1); }\n";
    assert_eq!(first(src), at(4, 13, cap), "matches_operand");
    let src = "fn outer(x: u8) {\n    fn inner(y: u8) -> bool {\n        matches!(y,\n            1 if x > 0)\n    }\n}\nfn main() { outer(1); }\n";
    assert_eq!(first(src), at(4, 18, cap), "matches_guard");
    // (E0107: an associated fn of a GAT's name is a value; a trait that requires arguments is not settled by its name)
    let src = "trait Tr { type A<'a>; #[allow(non_snake_case)] fn A() {} fn f() { Self::A(); } }\nfn main() {}\n";
    assert_eq!(first(src), None, "assoc_fn_of_the_name");
    let src = "trait Tr { type A<'a>; fn f() -> Self::A; }\nfn main() {}\n";
    assert_eq!(first(src), at(1, 40, "missing generics for associated type `Tr::A` (E0107)"), "type_use_still_reported");
    let src = "trait T<X> {}\ntype Obj = dyn T;\nfn main() {}\n";
    assert_eq!(first(src), at(2, 16, "missing generics for trait `T` (E0107)"), "dyn_of_trait_without_types");
    // (E0191: an instantiation with its defaults is one, however written; a lifetime is not a type argument)
    let src = "trait Base<T = u8> { type A; type B; }\ntrait Mid: Base<A = u8> {}\ntrait Root: Base<u8, B = u8> + Mid {}\nfn f(_: &dyn Root) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "defaults_merge");
    let src = "trait Base<T = u8> { type A; type B; }\ntrait Mid: Base<A = u8> {}\ntrait Root: Base<u8> + Mid {}\nfn f(_: &dyn Root) {}\nfn main() {}\n";
    assert_eq!(first(src), at(4, 14, &item("type `B` in `Base`")), "defaults_left_out_of_the_message");
    let src = "trait Base<T = u8> { type A; }\ntrait Mid: Base<u16, A = u8> {}\ntrait Root: Base + Mid {}\nfn f(_: &dyn Root) {}\nfn main() {}\n";
    assert_eq!(first(src), at(4, 14, &item("type `A` in `Base`")), "other_instantiation_apart");
    let src = "trait Base<T> { type A; }\ntrait Obj<'a, T>: Base<T, A = u8> + Base<u8> {}\nfn f(_: &dyn Obj<'static, u8>) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "lifetime_argument_not_substituted");
}

#[test]
fn review100_third_round() {
    // (each expected line, column and message is rustc 1.100's on the same source; None where rustc accepts it)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    let cap = "can't capture dynamic environment in a fn item (E0434)";
    // (E0434: a raw string's `\` is no escape; a sibling block's `let` is not in scope at the nested fn, an enclosing
    // block's is; a pattern's names are its bindings, not its fields or types)
    let src = "fn outer(x: u8) {\n    fn inner() {\n        let _ = format!(r\"{x}\\\");\n    }\n}\nfn main() { outer(1); }\n";
    assert_eq!(first(src), at(3, 28, cap), "raw_string_capture");
    let src = "fn outer() {\n    fn inner() { drop(0u8); }\n    { let drop = 1u8; let _ = drop; }\n    inner();\n}\nfn main() { outer(); }\n";
    assert_eq!(first(src), None, "sibling_block_let");
    let src = "fn outer() {\n    let x = 1u8;\n    {\n        let y = 2u8;\n        fn inner() -> u8 { y }\n        let _ = (x, inner());\n    }\n}\nfn main() { outer(); }\n";
    assert_eq!(first(src), at(5, 28, cap), "enclosing_block_let");
    let src = "fn outer(p: u8) {\n    let Point { x: a, y } = Point { x: 1, y: 2 };\n    let (m, mut n): (u8, u8) = (1, 2);\n    fn inner() -> u8 { a + y + m + n + p }\n    let _ = (a, y, m, n);\n}\nstruct Point { x: u8, y: u8 }\nfn main() { outer(1); }\n";
    assert_eq!(first(src), at(4, 24, cap), "pattern_bindings");
    // (E0191: a default that names an earlier parameter takes its argument)
    let item = |what: &str| format!("the value of the associated {what} must be specified (E0191)");
    let src = "trait Base<T, U = T> { type A; type B; }\ntrait Mid: Base<u8, A = u8> {}\ntrait Root: Base<u8, u8, B = u8> + Mid {}\nfn f(_: &dyn Root) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "default_of_an_earlier_parameter");
    let src = "trait Base<T, U = T> { type A; type B; }\ntrait Mid: Base<u8, A = u8> {}\ntrait Root: Base<u8, u8> + Mid {}\nfn f(_: &dyn Root) {}\nfn main() {}\n";
    assert_eq!(first(src), at(4, 14, &item("type `B` in `Base<u8>`")), "default_of_an_earlier_parameter_shown");
}

#[test]
fn review100_fourth_round() {
    // (each expected line, column and message is rustc 1.100's on the same source; None where rustc accepts it)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()));
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    let cap = "can't capture dynamic environment in a fn item (E0434)";
    // (E0434: an `if let`'s names are in its body only; a `let`'s start after its `;`, not in its initializer)
    let src = "fn outer() {\n    if let Some(drop) = Some(1u8) {\n        let _ = drop;\n    }\n    fn inner() { drop(0u8); }\n    inner();\n}\nfn main() { outer(); }\n";
    assert_eq!(first(src), None, "if_let_ends_with_its_body");
    let src = "fn outer() {\n    if let Some(y) = Some(1u8) {\n        fn inner() -> u8 { y }\n        let _ = (y, inner());\n    }\n}\nfn main() { outer(); }\n";
    assert_eq!(first(src), at(3, 28, cap), "if_let_in_its_body");
    let src = "fn outer() {\n    let drop = {\n        fn inner() { drop(0u8); }\n        inner();\n        1u8\n    };\n    let _ = drop;\n}\nfn main() { outer(); }\n";
    assert_eq!(first(src), None, "let_not_in_its_initializer");
    let src = "fn outer() {\n    let Some(z) = Some(1u8) else { return };\n    fn inner() -> u8 { z }\n    let _ = (z, inner());\n}\nfn main() { outer(); }\n";
    assert_eq!(first(src), at(3, 24, cap), "let_else_after_its_semicolon");
    // (E0191: an associated type's segment in a default is not a parameter)
    let src = "trait Id { type T; }\nimpl Id for u8 { type T = u8; }\ntrait Base<T, U = <u8 as Id>::T> { type A; type B; }\ntrait Mid: Base<u8, A = u8> {}\ntrait Root: Base<u8, <u8 as Id>::T, B = u8> + Mid {}\nfn f(_: &dyn Root) {}\nfn main() {}\n";
    assert_eq!(first(src), None, "associated_segment_in_a_default");
}

#[test]
fn attributes_at_crate_level_and_misplaced() {
    // (as rustc 1.100 reports them: an attribute macro an item takes, written at the crate; an attribute on an item
    // of a kind it does not apply to, an error or, for those rustc only warns about, an error where the lint is denied)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let mut first = |src: &str| s.check(src).first().map(|d| (d.line, d.msg.clone()));
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    for a in ["derive(Debug)", "test", "test_case", "bench", "global_allocator"] {
        let name = a.split('(').next().unwrap();
        assert_eq!(first(&format!("#![{a}]\nfn main() {{}}\n")), at(1, &format!("`{name}` attribute cannot be used at crate level")), "{a}");
    }
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("inline", "const FOO: u8 = 0;", Some("the `inline` attribute cannot be used on constants")),
        ("used", "fn foo() {}", Some("the `used` attribute cannot be used on functions")),
        ("used", "static FOO: u32 = 0;", None),
        ("target_feature(enable = \"sse2\")", "struct S;", Some("the `target_feature` attribute cannot be used on structs")),
        ("debugger_visualizer(natvis_file = \"a.natvis\")", "fn foo() {}", Some("the `debugger_visualizer` attribute cannot be used on functions")),
        ("repr(packed)", "enum E { A }", Some("the `repr(packed)` attribute cannot be used on enums")),
        ("repr(C)", "fn foo() {}", Some("the `repr(C)` attribute cannot be used on functions")),
        ("repr(C)", "enum E { A }", None),
        ("no_mangle", "const C: u8 = 0;", Some("const items should never be `#[no_mangle]`")),
        // (warnings only: not an error)
        ("cold", "struct S;", None),
        ("macro_use", "struct S;", None),
        ("no_mangle", "struct S;", None),
    ];
    for (a, item, want) in cases {
        assert_eq!(first(&format!("#[{a}]\n{item}\nfn main() {{}}\n")), want.and_then(|m| at(1, m)), "{a} {item}");
    }
    // (a comment between attributes does not hide the ones after it)
    assert_eq!(first("#[inline]\n// a comment\n#[target_feature(enable = \"sse2\")]\nconst FOO: u8 = 0;\nfn main() {}\n"), at(1, "the `inline` attribute cannot be used on constants"), "comment_between");
    assert_eq!(first("#[inline = \"\"]\nfn f() {}\nfn main() {}\n"), at(1, "valid forms for the attribute are `inline`, `inline(always)`, and `inline(never)`"), "inline_eq");
    assert_eq!(first("#![deny(unused_attributes)]\n#[macro_export]\n#[macro_use]\n// a comment\n#[path = \"foo\"]\nmacro_rules! foo {\n    () => {};\n}\nfn main() {}\n"), at(3, "the `macro_use` attribute cannot be used on macro defs"), "macro_def");
    assert_eq!(first("#[macro_use]\nmacro_rules! foo {\n    () => {};\n}\nfn main() {}\n"), None, "macro_def_not_denied");
    // (a warned one is an error where `unused_attributes` is denied)
    assert_eq!(first("#![deny(unused_attributes)]\n#[cold]\nstruct S;\nfn main() {}\n"), at(2, "the `cold` attribute cannot be used on structs"), "denied");
}

#[test]
fn review106_attribute_scans() {
    // (each expected line and message is rustc 1.100's on the same source; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    // (cfg: a candidate in a block comment or a string is not an attribute; `cfg!` takes a trailing comma)
    assert_eq!(check(2021, "/* #[cfg(not())] */\nfn main() {}\n"), None, "block_comment");
    assert_eq!(check(2021, "fn main() { let _ = \"#[cfg(not())]\"; let _ = r#\"#[cfg(not())]\"#; }\n"), None, "strings");
    assert_eq!(check(2021, "fn main() { let _ = cfg!(unix,); }\n"), None, "trailing_comma");
    assert_eq!(check(2021, "/* a */ #[cfg(not())]\nfn g() {}\nfn main() {}\n"), at(1, "malformed `cfg` attribute input"), "after_a_comment");
    // (a hashed raw string is a string literal)
    assert_eq!(check(2021, "#[deprecated(note = r#\"reason\"#)]\npub fn f() {}\nfn main() {}\n"), None, "raw_note");
    assert_eq!(check(2021, "#[cfg(feature = r#\"x\"#)]\nfn g() {}\nfn main() {}\n"), None, "raw_cfg_value");
    // (a lint's level is the last inner attribute that names it, not any `deny`)
    let f = "#[no_mangle]\npub fn f() {}\nfn main() { f(); }\n";
    assert_eq!(check(2021, &format!("#![deny(dead_code)]\n#![warn(unsafe_attr_outside_unsafe)]\n{f}")), None, "other_lint_denied");
    assert_eq!(check(2021, &format!("#![deny(unsafe_attr_outside_unsafe)]\n{f}")), at(2, "unsafe attribute used without unsafe"), "named_lint_denied");
    // (a comment that names the crate type does not make the crate a proc-macro one)
    assert_eq!(check(2021, "// crate_type = \"proc-macro\"\n#[proc_macro]\npub fn m(a: u8) -> u8 { a }\nfn main() {}\n"), at(2, "the `proc_macro` attribute is only usable with crates of the `proc-macro` crate type"), "comment_crate_type");
}

#[test]
fn review106_second_round() {
    // (each expected line and message is rustc 1.100's on the same source; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    let crate_type = "the `proc_macro` attribute is only usable with crates of the `proc-macro` crate type";
    // (a value is compared as text: a raw string is the same string)
    let src = "#![windows_subsystem = r\"console\"]\n#[link(name = \"m\", kind = r\"static\")]\nextern \"C\" {}\nfn main() {}\n";
    assert_eq!(check(2021, src), None, "raw_values");
    assert_eq!(check(2021, "#![windows_subsystem = \"con\\x73ole\"]\nfn main() {}\n"), None, "escaped_value");
    assert_eq!(check(2021, "#![windows_subsystem = r\"gui\"]\nfn main() {}\n"), at(1, "malformed `windows_subsystem` attribute input"), "raw_unknown_value");
    // (an inner attribute may span lines; one in a block comment is no attribute)
    let src = "#![crate_type =\n    \"proc-macro\"]\nextern crate proc_macro;\n#[proc_macro]\npub fn m(a: proc_macro::TokenStream) -> proc_macro::TokenStream { a }\n";
    assert!(check(2021, src).is_none_or(|(_, m)| m != crate_type), "multiline_crate_type");
    let src = "#[proc_macro]\npub fn m(a: u8) -> u8 { a }\nfn main() {}\n\n/*\n#![crate_type = \"proc-macro\"]\n*/\n";
    assert_eq!(check(2021, src), at(1, crate_type), "block_comment_crate_type");
    // (an escaped backslash ends no string: the entries after it are split)
    assert_eq!(check(2021, "#[deprecated(note = \"\\\\\", since = \"1.0\",)]\npub fn f() {}\nfn main() {}\n"), None, "escaped_backslash");
    // (comments in a cfg predicate are whitespace)
    assert_eq!(check(2021, "#[cfg(all(/* note */))]\nfn f() {}\nfn main() {}\n"), None, "comment_in_predicate");
    assert_eq!(check(2021, "#[cfg(not(/* a */ unix, windows))]\nfn f() {}\nfn main() {}\n"), at(1, "malformed `cfg` attribute input"), "comment_and_two_patterns");
    // (a macro's pattern is tokens, checked only when expanded)
    assert_eq!(check(2021, "macro_rules! eat { (cfg!(not())) => {}; }\nfn main() {}\n"), None, "macro_pattern");
    assert_eq!(check(2021, "macro_rules! eat { () => {}; }\nfn main() { let _ = cfg!(not()); }\n"), at(2, "malformed `cfg` macro input"), "after_a_macro");
    // (an item's `allow` among other lints lowers the level again)
    let src = "#![deny(unsafe_attr_outside_unsafe)]\n\n#[allow(dead_code, unsafe_attr_outside_unsafe)]\n#[no_mangle]\npub fn f() {}\n\nfn main() { f(); }\n";
    assert_eq!(check(2021, src), None, "allow_in_a_list");
}

#[test]
fn private_paths() {
    // (E0603: each expected line, column and message is rustc 1.100's on the same source; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |l: usize, c: usize, m: &str| Some((l, c, format!("{m} (E0603)")));
    let m = "mod a {\n    fn f() {}\n    mod b { pub fn g() {} }\n    struct U;\n    struct T(u8);\n    struct S { x: u8 }\n    const C: u8 = 1;\n    type Al = u8;\n    enum E { V }\n    pub(super) fn sup() {}\n    pub mod c { pub(super) fn cs() {} pub(in crate::a) fn ci() {} }\n}\nfn main() {\n";
    let main = |line: &str| format!("{m}{line}\n}}\n");
    for (line, col, msg) in [
        ("    a::f();", 8, "function `f` is private"),
        ("    a::b::g();", 8, "module `b` is private"),
        ("    let _ = a::U;", 16, "unit struct `U` is private"),
        ("    let _ = &a::U;", 17, "unit struct `U` is private"),
        ("    let _: a::U;", 15, "struct `U` is private"),
        ("    let _ = a::T(1);", 16, "tuple struct constructor `T` is private"),
        ("    let _ = a::S { x: 1 };", 16, "struct `S` is private"),
        ("    let _ = a::C;", 16, "constant `C` is private"),
        ("    let _: a::Al = 1;", 15, "type alias `Al` is private"),
        ("    let _ = a::E::V;", 16, "enum `E` is private"),
        ("    a::c::cs();", 11, "function `cs` is private"),
        ("    a::c::ci();", 11, "function `ci` is private"),
        ("    struct Z { u: u8 } let _ = Z { u: { let _ = a::U; 1 } };", 52, "unit struct `U` is private"),
    ] {
        assert_eq!(check(2021, &main(line)), at(14, col, msg), "{line}");
    }
    assert_eq!(check(2021, &main("    a::sup();")), None, "pub_super_from_the_parent");
    // (an import: what rustc calls a struct of any kind there; 2015 and later read `use a::f` from the crate root here)
    for ed in [2015, 2021] {
        assert_eq!(check(ed, "mod a { struct U; }\nuse a::U;\nfn main() {}\n"), at(2, 8, "struct `U` is private"), "use_{ed}");
    }
    // (a child module sees its parent's private items; a re-export, a glob, a macro's item and a module of a fn body are
    // not followed)
    let src = "mod a {\n    fn f() {}\n    pub mod b { pub fn g() { super::f(); crate::a::f(); } }\n    mod c { pub fn j() {} }\n    pub use self::c::j;\n    macro_rules! mk { () => { pub fn made() {} } }\n    mk!();\n    pub struct S;\n    impl S { pub fn new() -> S { S } }\n}\nuse a::*;\nfn main() {\n    a::j();\n    a::made();\n    let _ = a::S::new();\n    mod a2 { pub fn z() {} }\n    a2::z();\n}\n";
    for ed in [2015, 2021] {
        assert_eq!(check(ed, src), None, "visible_{ed}");
    }
    // (a syntax error first, as rustc)
    let src = "mod foo { mod bar { struct B; } }\nstruct Foo {\n    a: foo:A,\n    b: foo::bar::B,\n}\nfn main() {}\n";
    assert!(check(2021, src).is_some_and(|(l, _, m)| l == 3 && !m.contains("E0603")), "syntax_error_first");
}

#[test]
fn review106_third_round() {
    // (each expected line and message is rustc 1.100's on the same source; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    // (a comment in an attribute's input is whitespace)
    assert_eq!(check(2021, "#![windows_subsystem = /* reason */ \"console\"]\nfn main() {}\n"), None, "comment_in_inner_input");
    assert_eq!(check(2021, "#[deprecated(note = /* why */ \"old\")]\npub fn f() {}\nfn main() {}\n"), None, "comment_in_item_input");
    // (a Unicode escape's digits may have `_` between them)
    assert_eq!(check(2021, "#![windows_subsystem = \"\\u{6_3}onsole\"]\nfn main() {}\n"), None, "unicode_escape_underscore");
    // (`macro_rules !` with a space; one in a comment defines nothing)
    // (zustc reads no `macro_rules !` with a space yet: only no cfg error is asked of it)
    assert!(check(2021, "macro_rules ! eat { (cfg!(not())) => {}; }\nfn main() {}\n").is_none_or(|(_, m)| !m.contains("cfg")), "spaced_macro_rules");
    assert_eq!(check(2021, "// macro_rules! x {\nfn main() { let _ = cfg!(not()); }\n"), at(2, "malformed `cfg` macro input"), "macro_rules_in_a_comment");
    // (what a false `cfg` takes away is not read: an item, a statement or a block of one, an attribute after one)
    for src in [
        "#[cfg(any())]\nfn f() { let _ = cfg!(not()); }\nfn main() {}\n",
        "#[cfg(any())]\n#[cfg(not())]\nfn f() {}\nfn main() {}\n",
        "#[cfg(any())]\nmod m {\n    #[cfg(not())]\n    fn f() {}\n}\nfn main() {}\n",
        "fn main() {\n    #[cfg(any())]\n    let _ = cfg!(not());\n    #[cfg(any())]\n    {\n        let _ = cfg!(not());\n    }\n}\n",
    ] {
        assert_eq!(check(2021, src), None, "{src}");
    }
    assert_eq!(check(2021, "#[cfg(not())]\n#[cfg(any())]\nfn f() {}\nfn main() {}\n"), at(1, "malformed `cfg` attribute input"), "malformed_first");
    assert_eq!(check(2021, "#[cfg(all())]\n#[cfg(not())]\nfn f() {}\nfn main() {}\n"), at(2, "malformed `cfg` attribute input"), "after_a_true_cfg");
    assert_eq!(check(2021, "fn main() { let _ = cfg!(not()); }\n"), at(1, "malformed `cfg` macro input"), "kept_macro");
    // (a char escape the text ends in: no crash)
    let _ = check(2021, "fn main() { let _ = cfg!(x '\\");
    let _ = check(2021, "#[cfg(a = '\\");
}

#[test]
fn attributes_on_variants_params_and_prototypes() {
    // (each expected line and message is rustc 1.100's on the same source; None where rustc accepts it)
    let check = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    let params = "allow, cfg, cfg_attr, deny, expect, forbid, and warn are the only allowed built-in attributes in function parameters";
    for (src, line, msg) in [
        ("enum Foo {\n    #[inline]\n    Variant,\n}\nfn main() {}\n", 2, "the `inline` attribute cannot be used on enum variants"),
        ("enum Foo {\n    #[repr(u8)]\n    Variant,\n}\nfn main() {}\n", 2, "the `repr(u8)` attribute cannot be used on enum variants"),
        ("fn f(#[inline] p: u32) {}\nfn main() {}\n", 1, params),
        ("fn f(#[allow(unused)] a: u8, #[doc = \"x\"] b: u8) {}\nfn main() {}\n", 1, params),
        ("struct S;\nimpl S {\n    fn g(&self, #[inline] a: u8) {}\n}\nfn main() {}\n", 3, params),
        ("#[link_ordinal(123)]\nstruct Foo {}\nfn main() {}\n", 1, "the `link_ordinal` attribute cannot be used on structs"),
        ("#[repr(i128)]\nstruct Foo;\nfn main() {}\n", 1, "the `repr(i128)` attribute cannot be used on structs"),
        ("#[repr(C, u8)]\nstruct S;\nfn main() {}\n", 1, "the `repr(u8)` attribute cannot be used on structs"),
        ("#![deny(unused_attributes)]\ntrait Trait {\n    #[inline]\n    fn foo();\n}\nfn main() {}\n", 3, "the `inline` attribute cannot be used on required trait methods"),
    ] {
        assert_eq!(check(src), at(line, msg), "{src}");
    }
    // (accepted, or only warned of: a lint attribute or `cfg` on a parameter, `non_exhaustive` / `cold` on a variant,
    // `inline` on a required trait method where `unused_attributes` only warns, an enum's integer repr)
    for src in [
        "fn f(#[allow(unused)] a: u8, #[cfg(all())] b: u8) {}\nfn main() {}\n",
        "enum E {\n    #[cold]\n    A,\n    #[non_exhaustive]\n    B,\n}\nfn main() {}\n",
        "trait T {\n    #[inline]\n    fn f();\n}\nfn main() {}\n",
        "#[repr(u8)]\nenum E { A }\nfn main() {}\n",
        "fn f<F: Fn(u8) -> u8>(#[allow(unused)] g: F) {}\nfn main() {}\n",
    ] {
        assert_eq!(check(src), None, "{src}");
    }
}

#[test]
fn unknown_attributes_anywhere() {
    // (each expected line is rustc 1.100's "cannot find attribute" on the same source; None where rustc accepts it)
    let check = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, n: &str| Some((l, format!("cannot find attribute `{n}` in this scope")));
    for (src, line) in [
        ("#[foobar] macro_rules! m { () => {} }\nfn main() {}\n", 1),
        ("#[foobar] mod m { pub fn f() {} }\nfn main() {}\n", 1),
        ("enum E {\n    #[foobar] A,\n}\nfn main() {}\n", 2),
        ("struct S {\n    #[foobar] x: u8,\n}\nfn main() {}\n", 2),
        ("fn main() {\n    #[foobar] let x = 1;\n}\n", 2),
        ("fn f(#[foobar] x: u8) {}\nfn main() {}\n", 1),
        ("extern \"C\" {\n    #[foobar] fn g();\n}\nfn main() {}\n", 2),
    ] {
        assert_eq!(check(src), at(line, "foobar"), "{src}");
    }
    // (`diagnostic` is a namespace, not an attribute)
    assert_eq!(check("#[diagnostic]\npub struct Bar;\nfn main() {}\n"), at(1, "diagnostic"), "bare_diagnostic");
    // (accepted: a macro's input or body, a tool's path, a derive's helper, a comment, a namespaced one)
    for src in [
        "macro_rules! m { ($i:item) => { $i } }\nm!(#[foobar] fn f() {});\nfn main() {}\n",
        "macro_rules! m { () => { #[foobar] fn f() {} } }\nfn main() {}\n",
        "#[rustfmt::skip]\nfn main() {}\n",
        "#[derive(Default)]\nenum E {\n    #[default]\n    A,\n}\nfn main() {}\n",
        "// #[foobar]\nfn main() { let _ = \"#[foobar]\"; }\n",
        "#[diagnostic::on_unimplemented(message = \"m\")]\ntrait T {}\nfn main() {}\n",
    ] {
        assert_eq!(check(src), None, "{src}");
    }
}

#[test]
fn unresolved_imports() {
    // (each expected line, column and message is rustc 1.100's on the same source, in both editions unless one is
    // given; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    for edition in [2015, 2021] {
        // (a name the module does not define; on through a struct, a fn, a type alias's glob)
        assert_eq!(check(edition, "mod m {}\nuse m::nothing as _;\nfn main() {}\n"), at(2, "unresolved import `m::nothing` (E0432)"), "missing");
        assert_eq!(check(edition, "mod m { pub struct S; impl S { pub const C: u8 = 1; } }\nuse m::S::C;\nfn main() {}\n"), at(2, "unresolved import `m::S` (E0432)"), "through_struct");
        assert_eq!(check(edition, "mod a { pub fn f() {} }\nfn main() { use a::f::g; }\n"), at(2, "unresolved import `a::f` (E0432)"), "through_fn");
        assert_eq!(check(edition, "mod m { pub type A = u8; }\nuse m::A::*;\nfn main() {}\n"), at(2, "unresolved import `m::A` (E0432)"), "glob_of_alias");
        assert_eq!(check(edition, "mod foo { pub fn f() {} }\nuse foo::f::{self};\nfn main() {}\n"), at(2, "unresolved import `foo::f` (E0432)"), "self_of_fn");
        // (what else could define the name there: an `extern` block, a glob, a macro, an import; an enum's variant, a
        // module of the same name as a fn)
        for src in [
            "mod m { extern \"C\" { pub fn abs(x: i32) -> i32; } }\nuse m::abs;\nfn main() {}\n",
            "mod m { pub use std::fmt::*; }\nuse m::Debug;\nfn main() {}\n",
            "mod m { macro_rules! mk { () => { pub fn g() {} } } mk!(); }\nuse m::g;\nfn main() {}\n",
            "mod m { pub enum E { A } }\nuse m::E::A;\nfn main() { let _ = A; }\n",
            "mod m { pub use std::fmt as f; }\nuse m::f::Debug;\nfn main() {}\n",
            "mod m { pub mod n { pub fn f() {} } pub fn n() {} }\nuse m::n::f;\nfn main() { f() }\n",
            "mod a { pub mod b {} }\nuse a::b::{self as c};\nfn main() {}\n",
        ] {
            assert_eq!(check(edition, src), None, "{edition} {src}");
        }
    }
    // (an `extern crate` binds its name in the module: `use crate::m::alloc::..` is no unresolved import there)
    assert_eq!(check(2021, "mod m { pub extern crate alloc; }\nuse crate::m::alloc::vec::Vec;\nfn main() { let _: Vec<u8> = Vec::new(); }\n"), None, "extern_crate_in_module");
    // (E0433: `super` and `self` past the start of a path)
    let src = "pub mod x {\n    pub use crate::x::super::{self as c1};\n    pub fn foo() {}\n}\nfn main() {}\n";
    assert_eq!(check(2015, src), at(2, "`super` in paths can only be used in start position (E0433)"), "super_in_middle");
    let src = "pub mod x {\n    pub fn foo() {}\n}\nfn main() {\n    crate::x::self::foo();\n}\n";
    assert_eq!(check(2021, src), at(5, "`self` in paths can only be used in start position or last position (E0433)"), "self_in_middle");
    // (2015: a path from the crate root, whose `main` is a fn)
    assert_eq!(check(2015, "use main::bar;\nfn main() {}\n"), at(1, "unresolved import `main` (E0432)"), "from_root_2015");
}

#[test]
fn field_privacy() {
    // (E0451 / E0616: a field a module may not see, in a struct literal or pattern, a `..base`, an access; each
    // expected line, column and message is rustc 1.100's on the same source, in both editions; None where rustc
    // accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    for edition in [2015, 2021] {
        let src = "mod a { pub struct Foo { x: isize } }\nfn main() {\n    let _ = a::Foo { x: 1 };\n}\n";
        assert_eq!(check(edition, src), at(3, 22, "field `x` of struct `Foo` is private (E0451)"), "literal");
        let src = "mod a { pub struct Foo { x: isize } pub fn make() -> Foo { Foo { x: 3 } } }\nuse a::Foo;\nfn main() {\n    match a::make() {\n        Foo { x: _ } => {}\n    }\n}\n";
        assert_eq!(check(edition, src), at(5, 15, "field `x` of struct `Foo` is private (E0451)"), "pattern");
        let src = "mod a { pub struct S { pub a: u8, b: u8 } pub fn mk() -> S { S { a: 1, b: 2 } } }\nfn main() {\n    let s = a::mk();\n    let _ = a::S { a: 3, ..s };\n}\n";
        assert_eq!(check(edition, src), at(4, 28, "field `b` of struct `S` is private (E0451)"), "base");
        let src = "mod m { pub struct S { pub v: bool, a: u8 } }\nfn main() {\n    let _ = m::S { v: true };\n}\n";
        assert_eq!(check(edition, src), at(3, 13, "cannot construct `S` with struct literal syntax due to private fields"), "left_out");
        let src = "mod a {\n    pub struct Foo { x: u32 }\n    impl Foo { pub fn new() -> Foo { Foo { x: 0 } } }\n}\nfn main() {\n    let f = a::Foo::new();\n    let _ = f.x;\n}\n";
        assert_eq!(check(edition, src), at(7, 15, "field `x` of struct `Foo` is private (E0616)"), "access");
        let src = "mod m { pub union U { pub a: u8, c: u8 } }\nfn main() {\n    let _ = m::U { c: 0 };\n}\n";
        assert_eq!(check(edition, src), at(3, 20, "field `c` of union `U` is private (E0451)"), "union");
        // (the type check's error first: rustc's privacy pass does not run after it)
        let src = "mod foo { pub struct Foo { x: u32 } impl Foo { fn method(&self) {} } }\nfn main() {\n    let f = foo::Foo { x: 0 };\n    f.method;\n}\n";
        assert_eq!(check(edition, src).map(|x| x.0), Some(4), "type_error_first");
        // (in the module, `pub(super)` from the parent, a child module, `pub` and `pub(crate)`)
        for src in [
            "mod a { pub struct Foo { x: u8 } pub fn f() -> u8 { let s = Foo { x: 1 }; s.x } }\nfn main() { let _ = a::f(); }\n",
            "mod a { pub mod b { pub struct S { pub(super) x: u8 } } pub fn f() -> u8 { let s = b::S { x: 1 }; s.x } }\nfn main() { let _ = a::f(); }\n",
            "mod a { pub struct S { x: u8 } pub mod c { pub fn g(s: &super::S) -> u8 { s.x } } pub fn mk() -> S { S { x: 1 } } }\nfn main() { let _ = a::c::g(&a::mk()); }\n",
            "mod a { pub struct S { pub x: u8, pub(crate) y: u8 } }\nfn main() { let s = a::S { x: 1, y: 2 }; let _ = s.x + s.y; }\n",
        ] {
            assert_eq!(check(edition, src), None, "{edition} {src}");
        }
    }
}

#[test]
fn method_privacy() {
    // (E0624: a fn or const of an inherent impl, not `pub`, used outside the module of the impl; each expected line,
    // column and message is rustc 1.100's on the same source, in both editions; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    for edition in [2015, 2021] {
        let src = "mod a { pub struct S; impl S { fn new() -> S { S } } }\nfn main() {\n    let _ = a::S::new();\n}\n";
        assert_eq!(check(edition, src), at(3, 19, "associated function `new` is private (E0624)"), "path");
        let src = "mod a { pub struct Foo; impl Foo { fn f(&self) {} } }\nfn main() {\n    let x = a::Foo;\n    x.f();\n}\n";
        assert_eq!(check(edition, src), at(4, 7, "method `f` is private (E0624)"), "method");
        let src = "mod a { pub struct Foo; impl Foo { const ID: i32 = 1; } }\nfn main() {\n    let _ = a::Foo::ID;\n}\n";
        assert_eq!(check(edition, src), at(3, 21, "associated constant `ID` is private (E0624)"), "const");
        // (the impl's module, not the type's)
        let src = "mod a { pub struct S; }\nmod b { impl crate::a::S { fn g(&self) {} } }\nfn main() {\n    a::S.g();\n}\n";
        assert_eq!(check(edition, src), at(4, 10, "method `g` is private (E0624)"), "impl_elsewhere");
        // (in the impl's module, `pub(crate)`, a child module, a trait's fn of the name, an impl in a fn of the root)
        for src in [
            "mod a { pub struct S; impl S { fn new() -> S { S } pub fn make() -> S { S::new() } } }\nfn main() { let _ = a::S::make(); }\n",
            "mod a { pub struct S; impl S { pub(crate) fn new() -> S { S } } }\nfn main() { let _ = a::S::new(); }\n",
            "mod a { pub struct S; impl S { fn f(&self) {} } pub mod c { pub fn g() { super::S.f(); } } }\nfn main() { a::c::g(); }\n",
            "mod a { pub struct S; impl S { fn f(&self) {} } pub trait T { fn f(&self); } impl T for S { fn f(&self) {} } }\nuse a::T;\nfn main() { a::S.f(); }\n",
            "mod a { pub struct Foo; }\nfn f() { impl a::Foo { fn bar(&self) {} } }\nfn main() { a::Foo.bar(); f(); }\n",
        ] {
            assert_eq!(check(edition, src), None, "{edition} {src}");
        }
    }
}

#[test]
fn import_privacy_and_reexports() {
    // (E0603 through a private import; E0364 / E0365: a re-export wider than what it names. Each expected line,
    // column and message is rustc 1.100's on the same source, in both editions; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    for edition in [2015, 2021] {
        let src = "mod public { pub struct Hi; }\nmod testing { use super::public::Hi; }\nuse testing::Hi;\nfn main() {}\n";
        assert_eq!(check(edition, src), at(3, 14, "struct import `Hi` is private (E0603)"), "use");
        let src = "mod a { pub struct S; }\nmod b { use crate::a::S; }\nfn main() {\n    let _ = b::S;\n}\n";
        assert_eq!(check(edition, src), at(4, 16, "unit struct import `S` is private (E0603)"), "value");
        let src = "fn f() {}\nmod m { pub use crate::f; }\nfn main() {}\n";
        assert_eq!(check(edition, src), at(2, 17, "`f` is only public within the crate, and cannot be re-exported outside (E0364)"), "crate_item");
        let src = "mod a { fn foo() {} pub mod b { pub use super::foo; } }\nfn main() {}\n";
        assert_eq!(check(edition, src), at(1, 41, "`foo` is private, and cannot be re-exported (E0364)"), "private_item");
        let src = "mod a { mod inner {} pub mod b { pub use super::inner; } }\nfn main() {}\n";
        assert_eq!(check(edition, src), at(1, 42, "`inner` is private, and cannot be re-exported (E0365)"), "module");
        // (a `pub` item, a `pub(crate)` or `pub(super)` re-export as narrow as the item, a `pub use` re-exported, a
        // private import used in its module)
        for src in [
            "pub fn f() {}\nmod m { pub use crate::f; }\nfn main() {}\n",
            "fn f() {}\nmod m { pub(crate) use crate::f; }\nfn main() {}\n",
            "mod a { fn foo() {} pub mod b { pub(super) use super::foo; } }\nfn main() {}\n",
            "mod a { pub struct S; }\nmod b { pub use crate::a::S; }\nuse b::S;\nfn main() { let _ = S; }\n",
            "mod a { pub struct S; }\nmod b { use crate::a::S; pub fn f() -> S { S } }\nfn main() { let _ = b::f(); }\n",
        ] {
            assert_eq!(check(edition, src), None, "{edition} {src}");
        }
    }
}

#[test]
fn import_cycles() {
    // (E0432: an import that names only itself through other imports, a glob of a module into itself; each expected
    // line, column and message is rustc 1.100's on the same source, in both editions; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    for edition in [2015, 2021] {
        assert_eq!(check(edition, "mod y {\n    pub use crate::y::x;\n}\nfn main() {}\n"), at(2, 13, "unresolved import `crate::y::x` (E0432)"), "self");
        let src = "mod a { pub use crate::b::foo; }\nmod b { pub use crate::a::foo; }\nfn main() {}\n";
        assert_eq!(check(edition, src), at(1, 17, "unresolved import `crate::b::foo` (E0432)"), "two");
        assert_eq!(check(edition, "use self::*;\nfn main() {}\n"), at(1, 5, "unresolved import `self::*` (E0432)"), "glob_self");
        // (a cycle of globs, an import of an item, one through another crate's)
        for src in [
            "mod a { pub use crate::b::*; pub fn f() {} }\nmod b { pub use crate::a::*; }\nfn main() { b::f(); }\n",
            "mod a { pub use crate::b::x; }\nmod b { pub fn x() {} }\nfn main() { a::x(); }\n",
            "mod a { pub use std::fmt::Debug; }\nmod b { pub use crate::a::Debug; }\nfn main() {}\n",
        ] {
            assert_eq!(check(edition, src), None, "{edition} {src}");
        }
    }
}

#[test]
fn tuple_constructors_and_leading_super() {
    // (a tuple struct with a field a module may not see: its constructor is as private as that field; more leading
    // `super`s than the module has parents. Each expected line, column and message is rustc 1.100's on the same source,
    // in both editions; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    for edition in [2015, 2021] {
        let src = "mod a { pub struct S(u8); }\nfn main() {\n    let _ = a::S(1);\n}\n";
        assert_eq!(check(edition, src), at(3, 16, "tuple struct constructor `S` is private (E0603)"), "path");
        let src = "mod a { pub struct S(pub u8, u8); }\nuse a::S;\nfn main() {\n    let _ = S(1, 2);\n}\n";
        assert_eq!(check(edition, src), at(4, 13, "cannot initialize a tuple struct which contains private fields (E0423)"), "call");
        let src = "mod a { pub struct S(u8); }\nuse a::S;\nfn f(s: S) { let S(_) = s; }\nfn main() {}\n";
        assert_eq!(check(edition, src), at(3, 18, "cannot match against a tuple struct which contains private fields (E0532)"), "pattern");
        let src = "mod outer {\n    mod inner {\n        struct Example(super::super::super::Impl);\n    }\n}\nstruct Impl;\nfn main() {}\n";
        assert_eq!(check(edition, src), at(3, 38, "too many leading `super` keywords within `crate::outer::inner` (E0433)"), "super");
        // (a `pub` field, the constructor in its module, the struct as a type, `super` within the crate)
        for src in [
            "mod a { pub struct S(pub u8); }\nuse a::S;\nfn main() {\n    let _ = S(1);\n}\n",
            "mod a { pub struct S(u8); pub fn f() -> S { S(1) } }\nfn main() { let _ = a::f(); }\n",
            "mod a { pub struct S(u8); pub fn f() -> S { S(1) } }\nfn g(_: a::S) {}\nfn main() { g(a::f()); }\n",
            "mod a { pub fn f() {} mod b { fn g() { super::f(); } } }\nfn main() {}\n",
        ] {
            assert_eq!(check(edition, src), None, "{edition} {src}");
        }
    }
}

#[test]
fn generic_parameters_from_outer_item() {
    // (E0401: an item in a fn body that names a generic parameter or `Self` of an item around it; each expected line,
    // column and message is rustc 1.100's on the same source; None where rustc accepts it)
    let check = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    let src = "fn foo<U>(v: Vec<U>) -> U {\n    fn bar(w: Vec<U>) -> u8 { 0 }\n    v.into_iter().next().unwrap()\n}\nfn main() {}\n";
    assert_eq!(check(src), at(2, 19, "can't use generic parameters from outer item (E0401)"), "type");
    let src = "fn foo<const X: u32>() {\n    fn bar() -> u32 {\n        X\n    }\n}\nfn main() {}\n";
    assert_eq!(check(src), at(3, 9, "can't use generic parameters from outer item (E0401)"), "const");
    let src = "struct S;\nimpl S {\n    fn f() {\n        fn g(_: Self) {}\n    }\n}\nfn main() {}\n";
    assert_eq!(check(src), at(4, 17, "can't use `Self` from outer item (E0401)"), "self_type");
    let src = "struct S0<T>(T);\nimpl<T> S0<T> {\n    fn foo() {\n        fn bar() -> S0<i32> {\n            Self(0)\n        }\n    }\n}\nfn main() {}\n";
    assert_eq!(check(src), at(5, 13, "can't reference `Self` constructor from outer item (E0401)"), "self_ctor");
    // (the item's own parameter of the name, a non-generic impl's constructor (a warning), lifetimes, an impl of a
    // local struct)
    for src in [
        "fn foo<T>(x: T) {\n    fn bar<T>(y: T) {}\n}\nfn main() {}\n",
        "struct S0(usize);\nimpl S0 {\n    fn foo() {\n        fn bar() -> S0 { Self(0) }\n    }\n}\nfn main() {}\n",
        "fn f<'a>(x: &'a str) -> usize {\n    fn g<'b>(y: &'b str) -> usize { y.len() }\n    g(x)\n}\nfn main() {}\n",
        "fn f<T>() {\n    struct L;\n    impl L { fn new() -> Self { L } }\n}\nfn main() {}\n",
    ] {
        assert_eq!(check(src), None, "{src}");
    }
}

#[test]
fn glob_ambiguity() {
    // (E0659: a name two glob imports bring with different definitions, where a path names it; each expected line
    // and column is rustc 1.100's on the same source, in both editions; None where rustc accepts it)
    let check = |edition: u16, src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let two = "mod a { pub struct S; }\nmod b { pub struct S; }\nuse a::*;\nuse b::*;\n";
    let fns = "mod a { pub fn f() {} }\nmod b { pub fn f() {} }\nuse a::*;\nuse b::*;\n";
    let at = |l: usize, c: usize, n: &str| Some((l, c, format!("`{n}` is ambiguous (E0659)")));
    for edition in [2015, 2021] {
        // (a type, a call, an enum's variants, a path through a module's globs, an import)
        assert_eq!(check(edition, &format!("{two}fn f(_: S) {{}}\nfn main() {{}}\n")), at(5, 9, "S"), "type");
        assert_eq!(check(edition, &format!("{fns}fn main() {{\n    f();\n}}\n")), at(6, 5, "f"), "call");
        assert_eq!(check(edition, "use self::A::*;\nuse self::B::*;\nenum A { V }\nenum B { V }\nfn main() { let _ = V; }\n"), at(5, 21, "V"), "variants");
        let src = "mod a {\n    mod c { pub struct E; }\n    mod d { pub struct E; }\n    pub use self::c::*;\n    pub use self::d::*;\n}\nfn main() { let _ = a::E; }\n";
        assert_eq!(check(edition, src), at(7, 24, "E"), "through_module");
        let src = "mod a { pub struct S; }\nmod b { pub struct S; }\nmod m { pub use crate::a::*; pub use crate::b::*; }\nuse m::S;\nfn main() {}\n";
        assert_eq!(check(edition, src), at(4, 8, "S"), "import");
        // (one definition both globs bring, an item or an import of the module that takes the name, a private item a
        // glob does not bring, no use of the name, a generic parameter, a `mut` binding, a field and a method, ..)
        for src in [
            "mod a { pub struct S; }\nmod b { pub use crate::a::S; }\nuse a::*;\nuse b::*;\nfn main() { let _ = S; }\n".to_string(),
            format!("{two}struct S;\nfn main() {{ let _ = S; }}\n"),
            format!("{two}use a::S;\nfn main() {{ let _ = S; }}\n"),
            "mod a { pub struct S; }\nmod b { struct S; }\nuse a::*;\nuse b::*;\nfn main() { let _ = S; }\n".to_string(),
            format!("{two}fn main() {{}}\n"),
            format!("{two}fn f<S>(_: S) {{}}\nfn main() {{}}\n"),
            format!("{fns}fn main() {{ let mut f = 1; f += 1; let _ = f; }}\n"),
            format!("{fns}struct P {{ f: u8 }}\nimpl P {{ fn f(&self) -> u8 {{ self.f }} }}\nfn main() {{ let p = P {{ f: 1 }}; let _ = p.f(); }}\n"),
            // (an enum of a block: its variant's name as defined)
            format!("{two}fn main() {{ enum E {{ S }} let _ = E::S; }}\n"),
        ] {
            assert_eq!(check(edition, &src), None, "{edition} {src}");
        }
    }
}

#[test]
fn fn_qualifier_order() {
    // (each expected line, column and message is rustc 1.100's on the same source; None where rustc accepts it)
    let check = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    for (q, col, msg) in [
        ("unsafe async", 8, "expected one of `extern` or `fn`, found keyword `async`"),
        ("async const", 7, "expected one of `extern`, `fn`, `safe`, or `unsafe`, found keyword `const`"),
        ("extern \"C\" unsafe", 12, "expected `fn`, found keyword `unsafe`"),
        ("async unsafe const", 14, "expected one of `extern` or `fn`, found keyword `const`"),
        ("unsafe extern \"C\" async", 19, "expected `fn`, found keyword `async`"),
        ("extern extern", 8, "expected `fn`, found keyword `extern`"),
    ] {
        assert_eq!(check(&format!("{q} fn f() {{}}\nfn main() {{}}\n")), Some((1, col, msg.to_string())), "{q}");
    }
    // (an item a false `cfg` takes away is parsed too; a fn pointer type's qualifiers are read the same way)
    assert_eq!(check("#[cfg(false)]\nunsafe async fn f() {}\nfn main() {}\n").map(|x| (x.0, x.1)), Some((2, 8)), "configured_away");
    assert_eq!(check("type F = extern \"C\" unsafe fn(u8);\nfn main() {}\n").map(|x| (x.0, x.1)), Some((1, 21)), "fn_pointer");
    // (in order, or not before a `fn`, or in a comment or a macro: nothing)
    let src = "const unsafe extern \"C\" fn f() {}\ntype G = *const unsafe fn(u8);\nfn main() { unsafe { let _ = async { 1 }; } }\nextern \"C\" { fn x(); }\nconst C: u8 = 1;\n// unsafe async fn bad() {}\nmacro_rules! m { () => { unsafe async fn z() {} } }\n";
    assert_eq!(check(src), None, "in_order");
}

#[test]
fn review106_fourth_round() {
    // (each expected line and message is rustc 1.100's on the same source, or the error it reports there; None where
    // rustc accepts it)
    let check = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    // (accepted: comments between attributes and in a `macro_rules!` head, a `cfg!` that `stringify!` quotes, a raw
    // C string's text, a `cfg_attr` that does not hold, a crate a false `#![cfg]` takes away, `$` in a comment or a
    // literal, `#[unsafe(..)]` and `#![..]` with comments or spaces before the name)
    for src in [
        "#[cfg(any())] /* note */ #[cfg(not())] fn f() {}\nfn main() {}\n",
        "#[cfg(any())] /// doc\n#[cfg(not())] fn f() {}\nfn main() {}\n",
        "macro_rules! m /* {} */ { (cfg!(not())) => {}; }\nfn main() {}\n",
        "const S: &str = stringify!(a (cfg!(not())));\nfn main() {}\n",
        "#[cfg_attr(any(), cfg(not()))]\nfn f() {}\nfn main() {}\n",
        "#[cfg_attr(all(), cfg_attr(any(), cfg(not())))]\nfn f() {}\nfn main() {}\n",
        "fn main() { let _ = cfg!(feature = \"$\"); let _ = cfg!(unix /* $x */); }\n",
        "#[unsafe(/* safety */ no_mangle)]\npub extern \"C\" fn f() {}\nfn main() {}\n",
        "#![windows_subsystem = \"con\\u{7_3}ole\"]\nfn main() {}\n",
    ] {
        assert_eq!(check(src), None, "{src}");
    }
    // (a false `#![cfg]` takes the crate away, `main` too: rustc reports only that, E0601)
    assert!(check("#![cfg(any())]\nfn g() {}\n#[cfg(not())] fn f() {}\nfn main() {}\n").is_none_or(|(_, m)| !m.contains("cfg")), "crate_cfg_false");
    // (zustc's lexer reads no raw C string yet: only no cfg error is asked of it)
    assert!(check("fn main() { let _ = cr#\"text\" #[cfg(not())] \"#; }\n").is_none_or(|(_, m)| !m.contains("cfg")), "raw_c_string");
    // (`#![ crate_type = "proc-macro"]` is a proc-macro crate: zustc has no `proc_macro` crate, so only the crate-type
    // error is asked of it)
    let pm = "#![ crate_type = \"proc-macro\"]\nextern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn m(t: TokenStream) -> TokenStream { t }\n";
    assert!(check(pm).is_none_or(|(_, m)| !m.contains("crate type")), "spaced_crate_type");
    // (rejected: `cfg!` with spaces or another delimiter, a `$` only in a comment, a value with more after its
    // literal, a Unicode escape without its brace, a nested `cfg` of a `cfg_attr` that holds, an attribute after a
    // true `#![cfg]`)
    for (src, line, msg) in [
        ("fn main() { let _ = cfg ! (not()); }\n", 1, "malformed `cfg` macro input"),
        ("fn main() { let _ = cfg![not()]; }\n", 1, "malformed `cfg` macro input"),
        ("fn main() { let _ = cfg!{not()}; }\n", 1, "malformed `cfg` macro input"),
        ("fn main() { let _ = cfg!(not() /* $ */); }\n", 1, "malformed `cfg` macro input"),
        ("fn main() { let _ = cfg!(feature = \"a\" + \"b\"); }\n", 1, "malformed `cfg` macro input"),
        ("fn main() { let _ = cfg!(feature = \"\\u{61\"); }\n", 1, "malformed `cfg` macro input"),
        ("fn main() { let _ = cfg!(feature = \"\\u{_61}\"); }\n", 1, "malformed `cfg` macro input"),
        ("#![windows_subsystem = \"con\\u{_73}ole\"]\nfn main() {}\n", 1, "malformed `windows_subsystem` attribute input"),
        ("#[cfg(feature = \"a\" + \"b\")]\nfn f() {}\nfn main() {}\n", 1, "malformed `cfg` attribute input"),
        ("#[cfg_attr(all(), cfg(not()))]\nfn f() {}\nfn main() {}\n", 1, "malformed `cfg` attribute input"),
        ("#[cfg_attr(all(), inline,\n    cfg(not()))]\nfn f() {}\nfn main() {}\n", 2, "malformed `cfg` attribute input"),
        ("#[cfg_attr(all(), cfg_attr(all(), cfg(not())))]\nfn f() {}\nfn main() {}\n", 1, "malformed `cfg` attribute input"),
        ("#![cfg(all())]\n#[cfg(not())] fn f() {}\nfn main() {}\n", 2, "malformed `cfg` attribute input"),
    ] {
        assert_eq!(check(src), at(line, msg), "{src}");
    }
}

#[test]
fn item_errors_at_their_part() {
    // (an error of an item's definition at the part rustc points at: the union field that needs dropping, the derive
    // a union cannot have or that is unknown, the variant whose implicit discriminant overflows, also in a fn body;
    // each expected line is rustc 1.100's on the same source)
    let line = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let src = "union U {\n    a: u8,\n    b: String,\n}\nfn main() {}\n";
    assert_eq!(line(src).map(|d| d.0), Some(3), "union field");
    let src = "#[derive(\n    Clone,\n    Copy,\n    PartialEq,\n)]\nunion U {\n    a: u8,\n}\nfn main() {}\n";
    assert_eq!(line(src).map(|d| d.0), Some(4), "union derive");
    let src = "#[derive(Clone,\n         Sync,\n         Copy)]\nenum Foo {}\nfn main() {}\n";
    assert_eq!(line(src), Some((2, "cannot find derive macro `Sync` in this scope".to_string())), "unknown derive");
    let src = "#[repr(u8)]\nenum A {\n    Ok = 254,\n    Ok2,\n    OhNo,\n}\nfn main() {}\n";
    assert_eq!(line(src).map(|d| d.0), Some(5), "E0370");
    let src = "fn f() {\n    #[repr(i8)]\n    enum A {\n        Ok = i8::MAX - 1,\n        Ok2,\n        OhNo,\n    }\n    let _ = A::Ok;\n}\nfn main() {}\n";
    assert_eq!(line(src).map(|d| d.0), Some(6), "E0370 in a fn body");
}

#[test]
fn errors_at_their_statement() {
    // (an error whose expression has no position of its own, at the statement's line, and the checks that come with
    // it; each expected line and message is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let at = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(first("fn main() {\n    let x = 1;\n    [];\n}\n"), at(3, "type annotations needed for an empty array"), "empty array");
    assert_eq!(first("fn main() {\n    let x = 1;\n    match () {}\n}\n"), at(3, "non-exhaustive patterns: type `()` is non-empty (E0004)"), "match without arms");
    assert_eq!(first("fn f() -> u8 {\n    let x = 1;\n    return;\n}\nfn main() {}\n"), at(3, "`return;` in a function whose return type is not `()` (E0069)"), "E0069");
    assert_eq!(first("fn main() {\n    for _ in ..10 {}\n}\n"), at(2, "`RangeTo<{integer}>` is not an iterator (E0277)"), "RangeTo");
    assert_eq!(first("fn f(x: &mut i32) {\n    for _ in *x {}\n}\nfn main() {}\n"), at(2, "`i32` is not an iterator (E0277)"), "i32 iterated");
    assert_eq!(first("fn main() {\n    const { 2 } - const { 1 };\n}\n"), at(2, "mismatched types: expected `()`, found `i32`"), "block-like statement");
    assert_eq!(first("fn f() {\n    if let Some(v) = Some(1) {\n        const { v }\n    }\n}\nfn main() {}\n"), at(3, "attempt to use a non-constant value in a constant (E0435)"), "E0435");
    assert_eq!(first("fn f() -> str {\n    loop {}\n}\nfn main() {}\n"), at(1, "the size for values of type `str` cannot be known at compilation time (E0277)"), "unsized return");
    // (union patterns: one field, reading it is unsafe, `_` reads nothing)
    let u = "union U { a: u8, b: u16 }\n";
    assert_eq!(first(&format!("{u}fn f(U {{ a }}: U) -> u8 {{\n    a\n}}\nfn main() {{}}\n")), at(2, "access to union field is unsafe and requires unsafe function or block (E0133)"), "union parameter");
    assert_eq!(first(&format!("{u}fn f(u: U) {{\n    let U {{ a, b }} = u;\n}}\nfn main() {{}}\n")), at(3, "union patterns should have exactly one field"), "two fields");
    assert_eq!(first(&format!("{u}fn f(u: U) {{\n    let U {{ a: _ }} = u;\n    unsafe {{ let U {{ b }} = u; }}\n}}\nfn main() {{}}\n")), None, "wildcard and unsafe");
}

/// An impl is for a type only when the type meets the impl's bounds, its where clauses' too (issue #2): nested goals of
/// traits of the program's. Also a trait's supertraits must be implemented by an impl's self type, and an impl for a
/// reference is not for the type it refers to. Each was checked against rustc 1.97
#[test]
fn impl_bounds_are_nested_goals() {
    let check = |src: &str| Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check(src);
    let pre = "trait Bar {}\ntrait Foo { fn foo(&self) {} }\nstruct W<T>(T);\nimpl Bar for i32 {}\nfn need<F: Foo>(_: F) {}\n";
    let bad = |body: &str| {
        let src = format!("{pre}{body}\n");
        let d = check(&src);
        assert!(d.len() == 1 && d[0].msg.contains("is not satisfied"), "{src}\n{d:?}");
    };
    let ok = |body: &str| {
        let src = format!("{pre}{body}\n");
        let d = check(&src);
        assert!(d.is_empty(), "{src}\n{d:?}");
    };
    for (imp, call) in [
        ("impl<T: Bar> Foo for W<T> {}", "need(W(_))"),
        ("impl<T> Foo for W<T> where T: Bar {}", "need(W(_))"),
        ("impl<T: Bar> Foo for T {}", "need(_)"),
        ("impl<T: Bar> Foo for W<T> {}", "W(_).foo()"),
        ("impl Foo for W<i32> {}", "need(W(_))"),
        ("impl<T: Bar> Foo for W<T> {}\nimpl<T: Foo> Bar for Vec<T> {}", "need(W(vec![W(_)]))"),
    ] {
        bad(&format!("{imp}\nfn main() {{ {}; }}", call.replace('_', "1u8")));
        ok(&format!("{imp}\nfn main() {{ {}; }}", call.replace('_', "1i32")));
    }
    // (a cycle of blanket impls through each other's bounds is not followed forever. An obligation through it is
    // E0275: goals_have_a_certainty)
    ok("trait P {}\ntrait Q {}\nimpl<T: Q> P for T {}\nimpl<T: P> Q for T {}\nfn main() {}");
    // (a goal asked again and again on the way (`T{i}: T{i-1} + T{i-2}`) is proved once: without that, 2^n goals)
    let fib = |n: usize, imp0: &str| {
        let mut s = format!("trait T0 {{}}\ntrait T1 {{}}\n{imp0}\nimpl T1 for i32 {{}}\n");
        for i in 2..=n {
            s += &format!("trait T{i} {{}}\nimpl<X: T{} + T{}> T{i} for X {{}}\n", i - 1, i - 2);
        }
        s + &format!("fn need2<X: T{n}>(_: X) {{}}\nfn main() {{ need2(1i32); }}")
    };
    ok(&fib(60, "impl T0 for i32 {}"));
    bad(&fib(60, "impl T0 for u8 {}"));
    // (a trait object implements its trait's supertraits, whatever a blanket impl of one asks)
    ok("trait Sup {}\ntrait Sub2: Sup {}\ntrait Imp {}\nimpl<T: ?Sized + Sub2 + Imp> Sup for T {}\nfn n<T: ?Sized + Sup>() {}\nfn main() { n::<dyn Sub2>(); }");
    // (supertraits)
    bad("trait A {}\ntrait B: A {}\nstruct C;\nimpl B for C {}\nfn main() {}");
    ok("trait A {}\ntrait B: A {}\nstruct C;\nimpl A for C {}\nimpl B for C {}\nfn main() {}");
    // (an impl of the supertrait for exactly the self type, of a type implements does not see the program's impls of)
    for t in ["[u8; 1]", "[u8]", "Box<u8>"] {
        ok(&format!("trait A {{}}\ntrait B: A {{}}\nimpl A for {t} {{}}\nimpl B for {t} {{}}\nfn main() {{}}"));
    }
    // (two blanket impls of one trait each have their own bounds: one that holds is enough)
    ok("trait A {}\ntrait B {}\ntrait Tr<X> {}\nimpl<T: A> Tr<u8> for T {}\nimpl<T: B> Tr<u16> for T {}\nimpl A for i32 {}\nfn need2<T: Tr<u8>>(_: T) {}\nfn main() { need2(1i32); }");
    // (tuples: the impl's self type and bounds, as for a struct)
    bad("impl Foo for (i32,) {}\nfn main() { need((1u8,)); }");
    ok("impl Foo for (i32,) {}\nfn main() { need((1i32,)); }");
    bad("impl<T: Bar> Foo for (T,) {}\nfn main() { need((1u8,)); }");
    ok("impl<T: Bar> Foo for (T,) {}\nfn main() { need((1i32,)); }");
    ok("impl Foo for (i32, u8) {}\nimpl Foo for (u8, i32) {}\nfn main() { need((1u8, 2i32)); }");
    // (a raw pointer's mutability is part of the impl's self type)
    bad("impl Foo for W<*const i32> {}\nfn main() { let mut x = 1; need(W(&mut x as *mut i32)); }");
    ok("impl Foo for W<*const i32> {}\nfn main() { let x = 1; need(W(&x as *const i32)); }");
    bad("impl Foo for W<*const i32> {}\nfn main() { let x = 1; need(W(&x)); }");
    // (and inside a fn pointer's parameters)
    bad("impl Foo for (fn(*const i32),) {}\nfn f(_: *mut i32) {}\nfn main() { need((f as fn(*mut i32),)); }");
    ok("impl Foo for (fn(*const i32),) {}\nfn f(_: *const i32) {}\nfn main() { need((f as fn(*const i32),)); }");
    // (an impl for `&mut i32` is not for i32)
    bad("trait Tr {}\nimpl Tr for &mut i32 {}\nfn f<T: Tr>(_: T) {}\nfn main() { f(1i32); }");
    ok("trait Tr {}\nimpl Tr for &mut i32 {}\nfn f<T: Tr>(_: T) {}\nfn main() { let mut x = 1i32; f(&mut x); }");
}

/// A goal's answer is Yes, Maybe or No (Program::evaluate, issue #2). A bound whose goal needs itself (an inductive
/// cycle) is E0275, as rustc gives; a cycle no obligation goes through, or one another impl gets around, is not an
/// error. Each was checked against rustc 1.97
#[test]
fn goals_have_a_certainty() {
    let check = |src: &str| Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check(src);
    let e0275 = |src: &str, at: &str| {
        let d = check(src);
        assert!(d.len() == 1 && d[0].msg.contains("overflow evaluating the requirement") && d[0].msg.contains(at), "{src}\n{d:?}");
    };
    let ok = |src: &str| {
        let d = check(src);
        assert!(d.is_empty(), "{src}\n{d:?}");
    };
    let pq = "trait P {}\ntrait Q {}\nimpl<T: Q> P for T {}\nimpl<T: P> Q for T {}\nfn need<X: P>(_: X) {}\n";
    e0275(&format!("{pq}fn main() {{ need(1u8); }}"), "`u8: P`");
    ok(&format!("{pq}fn main() {{}}"));
    // (a generic fn's own bound is a where clause: not followed)
    ok(&format!("{pq}fn g<Y: P>(y: Y) {{ need(y); }}\nfn main() {{}}"));
    // (a trait with arguments)
    e0275("trait Loop<U> {}\nimpl<T: Loop<u8>> Loop<u8> for T {}\nfn need<T: Loop<u8>>() {}\nfn main() { need::<i32>(); }", "`i32: Loop");
    ok("trait A {}\ntrait Tr<U> {}\nimpl<T: A> Tr<u8> for T {}\nimpl A for i32 {}\nfn need<T: Tr<u8>>() {}\nfn main() { need::<i32>(); }");
    // (an impl whose bound is its own trait)
    e0275("trait P {}\nimpl<T: P> P for T {}\nfn need<X: P>(_: X) {}\nfn main() { need(1u8); }", "`u8: P`");
    // (through a supertrait's blanket impl: traits/inductive-overflow/simultaneous.rs)
    e0275("trait A {}\ntrait B {}\nimpl<T: A> B for T {}\nimpl<T: B> A for T {}\ntrait C {}\nimpl<T: B + A> C for T {}\nfn need<X: C>(_: X) {}\nfn main() { need(4); }", "`i32: C`");
    // (struct impls through each other end at a concrete impl: no cycle)
    ok("trait P {}\ntrait Q {}\nstruct W<T>(T);\nimpl<T: Q> P for W<T> {}\nimpl<T: P> Q for W<T> {}\nimpl Q for u8 {}\nfn need<X: P>(_: X) {}\nfn main() { need(W(1u8)); }");
    // (a recursive type list)
    ok("trait P {}\nimpl P for () {}\nimpl<H: P, T: P> P for (H, T) {}\nfn need<X: P>(_: X) {}\nfn main() { need(((), ((), ((), ())))); }");
    // (two blanket impls of one trait: the one that holds is taken, whatever the other's goals do)
    ok("trait A {}\ntrait B {}\ntrait Tr<X> {}\nimpl<T: B> Tr<u8> for T {}\nimpl<T: A> Tr<u16> for T {}\nimpl<T: Tr<u8>> B for T {}\nimpl A for i32 {}\nfn need<T: Tr<u16>>(_: T) {}\nfn main() { need(1i32); }");
}

#[test]
fn format_string_errors_at_their_char() {
    // (an invalid format string at the char rustc points at, escapes, line continuations and raw strings counted;
    // each expected line, column and message is rustc 1.100's on the same source)
    let first = |body: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
        s.check(&format!("fn main() {{\n    {body}\n}}\n")).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |c: usize, m: &str| Some((2, c, format!("invalid format string: {m}")));
    assert_eq!(first(r#"let _ = format!("{ x}", x = 1);"#), at(24, "expected `}`, found `x`"), "space before the argument");
    assert_eq!(first(r#"let _ = format!("{x", x = 1);"#), at(24, "expected `}` but string was terminated"), "unterminated");
    assert_eq!(first(r#"let _ = format!("ab}");"#), at(24, "unmatched `}` found"), "unmatched");
    assert_eq!(first(r#"let _ = format!("{\n x}", x = 1);"#), at(26, "expected `}`, found `x`"), "after an escape");
    assert_eq!(first(r##"let _ = format!(r#"{ \n"#);"##), at(26, "expected `}`, found `\\`"), "raw");
    assert_eq!(first(r#"let _ = format!("{ }", 1);"#), None, "white space only");
    assert_eq!(first(r#"let _ = format!("{x }", x = 1);"#), None, "white space after");
    // (a line continuation: the next line's char)
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check("fn main() {\n    let _ = format!(\"  \\\n    { \\\n    b\");\n}\n");
    assert_eq!(d.first().map(|d| (d.line, d.msg.clone())), Some((4, "invalid format string: expected `}`, found `b`".to_string())), "continuation");
}

#[test]
fn fn_value_mismatch_at_its_origin() {
    // (a fn body's value that is not of the return type, where rustc points: the tail `()`, a `loop`'s `break`, the
    // `match` arm that does not diverge, an `unsafe` block; each expected line is rustc 1.100's on the same source)
    let line = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
        s.check(src).first().map(|d| d.line)
    };
    assert_eq!(line("fn foo() -> usize {\n    ()\n}\nfn main() {}\n"), Some(2), "unit tail");
    assert_eq!(line("fn f() -> i32 {\n    loop {\n        if false { break; }\n        return 42;\n    }\n}\nfn main() {}\n"), Some(3), "break");
    assert_eq!(line("fn f(a: Option<u32>) -> u32 {\n    match a {\n        Some(v) => return v,\n        None => (),\n    }\n}\nfn main() {}\n"), Some(4), "arm");
    assert_eq!(line("fn g() -> u8 { 0 }\nfn main() {\n    match () {\n        () => g()\n    }\n}\n"), Some(4), "arm of main");
    assert_eq!(line("fn f() -> ! {\n    unsafe {\n        let _x = 1;\n    }\n}\nfn main() {}\n"), Some(2), "unsafe block");
}

#[test]
fn parse_errors_where_rustc_points() {
    // (a missing `;` before a token on a later line: right after the expression; a struct field's missing `:` in a
    // fn body's struct: at its line, not past the file. Each expected line and column is rustc 1.100's)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col))
    };
    assert_eq!(first("fn f() -> f32 {\n    3\n    5.0\n}\nfn main() {}\n"), Some((2, 6)), "missing semicolon");
    assert_eq!(first("fn main() {\n    struct Test {\n        Very\n        Bad\n        Stuff\n    }\n}\n").map(|d| d.0), Some(4), "struct in a fn body");
}

#[test]
fn dyn_compatibility() {
    // (E0038: a `dyn` of a trait that is not dyn compatible, at the `dyn`, naming the trait whose own definition
    // makes it so; each expected line, column and message is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, t: &str| Some((l, c, format!("the trait `{t}` is not dyn compatible (E0038)")));
    let use_a = "fn f(_: &dyn A) {}\nfn main() {}\n";
    assert_eq!(first("trait Bar {\n    const X: usize;\n}\nfn f(_: &dyn Bar) {}\nfn main() {}\n"), e(4, 10, "Bar"), "associated const");
    assert_eq!(first(&format!("trait A {{\n    fn new() -> u8;\n}}\n{use_a}")), e(4, 10, "A"), "no receiver");
    assert_eq!(first(&format!("trait A {{\n    fn g<T>(&self, t: T);\n}}\n{use_a}")), e(4, 10, "A"), "generic method");
    assert_eq!(first(&format!("trait A {{\n    fn eq(&self, o: &Self) -> bool;\n}}\n{use_a}")), e(4, 10, "A"), "Self parameter");
    assert_eq!(first(&format!("trait A: Clone {{}}\n{use_a}")), e(2, 10, "A"), "Clone supertrait");
    assert_eq!(first("trait Super {\n    type G<T>;\n}\ntrait Child: Super {}\nfn f(_: &dyn Child) {}\nfn main() {}\n"), e(5, 14, "Super"), "through a supertrait");
    // (a method that requires `Self: Sized`, and a projection of `Self`, keep it dyn compatible)
    assert_eq!(first("trait A {\n    fn new() -> Self where Self: Sized;\n    type T;\n    fn get(&self) -> Self::T;\n}\nfn f(_: &dyn A<T = u8>) {}\nfn main() {}\n"), None, "exempt");
}

#[test]
fn method_signature_mismatch() {
    // (E0053: an impl's method whose parameter, result or `unsafe` is not the trait's, at the part rustc points at;
    // each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, m: &str| Some((l, c, format!("method `{m}` has an incompatible type for trait `Foo` (E0053)")));
    let tr = |decl: &str, imp: &str| format!("trait Foo {{\n    {decl}\n}}\nstruct A;\nimpl Foo for A {{\n    #[inline]\n{imp}\n}}\nfn main() {{}}\n");
    assert_eq!(first(&tr("fn foo(&self, x: u8) -> bool;", "    fn foo(\n        &self,\n        x: i16,\n    ) -> bool { x > 0 }")), e(9, 12, "foo"), "parameter");
    assert_eq!(first(&tr("fn foo(&self) -> u8;", "    fn foo(&self)\n        -> i8 { 0 }")), e(8, 12, "foo"), "result");
    assert_eq!(first(&tr("fn foo(&self) -> u8;", "    unsafe fn foo(&self) -> u8 { 0 }")), e(7, 5, "foo"), "unsafe");
    assert_eq!(first(&tr("fn foo(&self) -> u8;", "    fn foo(self) -> u8 { 0 }")), e(7, 12, "foo"), "receiver");
    assert_eq!(first(&tr("fn foo(&self) -> u8;", "    fn foo(self: Box<Self>) -> u8 { 0 }")), e(7, 18, "foo"), "receiver's type");
    // (an `-> impl Trait` or `async fn` declaration: the impl may return a type of its own)
    assert_eq!(first("use std::fmt::Display;\ntrait Foo {\n    fn bar(&self) -> impl Display;\n}\nimpl Foo for i32 {\n    fn bar(&self) -> i32 { *self }\n}\nfn main() {}\n"), None, "impl Trait");
}

#[test]
fn unused_variables_where_the_lint_is_denied() {
    // (unused_variables at the levels its attributes give: the crate's, the fn's, a statement's or an arm's; each
    // expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2024, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, n: &str| Some((l, c, format!("unused variable: `{n}`")));
    let deny = "#![deny(unused_variables)]\n";
    // (an `allow` on a statement covers what is in it, a `let`'s `else` block too)
    assert_eq!(
        first(&format!("{deny}fn main() {{\n    #[allow(unused_variables)]\n    let Some(_) = Some(1) else {{\n        let x = 1;\n        return;\n    }};\n    let y = 2;\n}}\n")),
        e(8, 9, "y"),
        "allow on a statement"
    );
    assert_eq!(first("#[deny(unused)]\nfn main() {\n    let unused = ();\n}\nfn other() {\n    let fine = ();\n}\n"), e(3, 9, "unused"), "deny on the fn");
    // (an arm's or a statement's own level is read where some item of the program, or the crate, names the lint's
    // level: a `deny` on an arm alone, with none on an item, is not seen)
    assert_eq!(
        first("fn f() {\n    match 0 {\n        #[deny(unused_variables)]\n        y => (),\n    }\n}\n#[deny(unused_variables)]\nfn g() {\n    match 0 {\n        #[allow(unused_variables)]\n        z => (),\n    }\n}\nfn main() {}\n"),
        e(4, 9, "y"),
        "deny on an arm"
    );
    assert_eq!(first(&format!("{deny}fn main() {{\n    match () {{\n        () if let Some(b) = Some(()) => {{}}\n        _ => {{}}\n    }}\n}}\n")), e(4, 24, "b"), "guard");
    assert_eq!(first(&format!("{deny}fn main() {{\n    let o = Some(1);\n    if let Some(b) = o {{}}\n}}\n")), e(4, 17, "b"), "if let");
    // (a crate's `allow(unused)` does not undo its `forbid(unused_variables)`; a parameter's pattern)
    assert_eq!(first("#![allow(unused)]\n#![forbid(unused_variables)]\nstruct Inner { i: i32 }\nfn foo(Inner { i }: Inner) {}\nfn main() {}\n"), e(4, 16, "i"), "forbid");
    assert_eq!(first(&format!("{deny}fn main() {{\n    #[allow(unused_variables)]\n    let a = 1;\n    let b = {{\n        #[allow(unused_variables)]\n        let c = 3;\n        4\n    }};\n    println!(\"{{b}}\");\n}}\n")), None, "allowed");
    // (a group's `forbid` lets a member be allowed)
    assert_eq!(first("#![forbid(unused)]\n#[allow(unused_variables)]\nfn main() {\n    let a: ();\n}\n"), None, "forbid of a group");
}

#[test]
fn copy_with_a_field_that_is_not() {
    // (E0204: `Copy` for a type with a field that is not Copy, at the type's name for a derive and at the self type
    // for an impl; each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize| Some((l, c, "the trait `Copy` cannot be implemented for this type (E0204)".to_string()));
    assert_eq!(first("use std::ops::Range;\n#[derive(Copy, Clone)]\nstruct R(Range<usize>);\nfn main() {}\n"), e(3, 8), "derive");
    assert_eq!(first("struct Foo {\n    foo: Vec<u32>,\n}\nimpl Clone for Foo { fn clone(&self) -> Self { loop {} } }\nimpl Copy for Foo {}\nfn main() {}\n"), e(5, 15), "impl");
    assert_eq!(first("#[derive(Clone)]\nstruct Wrapper<T>(T);\nimpl<S> Copy for Wrapper<S> {}\nfn main() {}\n"), e(3, 18), "an impl's parameter without a bound");
    assert_eq!(first("#[derive(Clone, Copy)]\nenum E { A(String), B }\nfn main() {}\n"), e(2, 6), "enum");
    assert_eq!(first("#[derive(Clone, Copy)]\nstruct M<'a> {\n    r: &'a mut bool,\n}\nfn main() {}\n"), e(2, 8), "&mut");
    // (a derive bounds the type's parameters; PhantomData is Copy for every type, ManuallyDrop when its type is; a
    // union with a written `impl Copy` may derive Clone alone)
    let ok = "#[derive(Clone, Copy)]\nstruct Ok1<T>(T, u8, &'static str, [u16; 3], (u8, char), fn(u8) -> u8, *const u8);\n#[derive(Clone)]\nstruct W<S>(S);\nimpl<S: Copy> Copy for W<S> {}\n#[derive(Clone, Copy)]\nenum E2 { A(u8), B { x: Option<u8> }, C }\n#[derive(Clone, Copy)]\nstruct P(std::marker::PhantomData<String>, std::mem::ManuallyDrop<u8>, std::ops::RangeTo<u8>, core::cmp::Ordering);\n#[derive(Clone)]\nunion U {\n    a: u8,\n}\nimpl Copy for U {}\nfn main() {}\n";
    assert_eq!(first(ok), None, "Copy");
}

#[test]
fn gated_cfg_names() {
    // (E0658: a `cfg` predicate of a name behind a feature gate, at the name, unless the crate enables the feature;
    // each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, n: &str| Some((l, c, format!("`cfg({n})` is experimental and subject to change (E0658)")));
    assert_eq!(first("#[cfg(not(sanitize = \"thread\"))]\nfn main() {}\n"), e(1, 11, "sanitize"), "attribute");
    assert_eq!(first("fn main() {\n    let _ = cfg!(overflow_checks);\n}\n"), e(2, 18, "overflow_checks"), "cfg!");
    assert_eq!(
        first("#[cfg_attr(target(os = \"linux\"), allow(unused))]\nfn main() {}\n"),
        Some((1, 12, "compact `cfg(target(..))` is experimental and subject to change (E0658)".to_string())),
        "compact target"
    );
    // (enabled, a value of a name that is not gated, and an attribute of what a cfg_attr puts on the item)
    assert_eq!(first("#![feature(cfg_overflow_checks)]\nfn main() {\n    let _ = cfg!(overflow_checks);\n}\n"), None, "enabled");
    assert_eq!(first("#[cfg(feature = \"version\")]\nfn f() {}\n#[cfg_attr(unix, doc = \"sanitize\")]\nfn main() {}\n"), None, "not gated");
}

#[test]
fn integer_literal_where_a_float_is_expected() {
    // (an unsuffixed integer literal is never a float: E0308 where a float is expected, E0277 for arithmetic with a
    // float, at the operator; each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    assert_eq!(first("fn main() {\n    let y: f64 = 2;\n    println!(\"{y}\");\n}\n"), e(2, 18, "mismatched types: expected `f64`, found integer"), "let");
    assert_eq!(first("fn g(x: f32) -> f32 { x }\nfn main() {\n    let z = g(3);\n    println!(\"{z}\");\n}\n"), e(3, 15, "mismatched types: expected `f32`, found integer"), "argument");
    assert_eq!(first("fn f(x: f32) -> f32 {\n    x + 100\n}\nfn main() {}\n"), e(2, 7, "cannot add `{integer}` to `f32` (E0277)"), "x + 100");
    assert_eq!(first("fn f(x: f64) -> f64 {\n    100 + x\n}\nfn main() {}\n"), e(2, 9, "cannot add `f64` to `{integer}` (E0277)"), "100 + x");
    assert_eq!(first("fn g(mut x: f64) -> f64 {\n    x += 1;\n    x\n}\nfn main() {}\n"), e(2, 7, "cannot add-assign `{integer}` to `f64` (E0277)"), "x += 1");
    // (a float literal, or one with a float suffix, is one)
    assert_eq!(first("fn main() {\n    let a: f64 = 2.0 + 1e3 + 2f64;\n    let b: f32 = 3.;\n    println!(\"{a} {b}\");\n}\n"), None, "floats");
}

#[test]
fn unsized_where_sized_is_needed() {
    // (E0277: an unsized type written where a Sized one is needed: an argument of std's containers or of a type of the
    // program's whose parameter does not say `?Sized`, an enum variant's field, a struct's field before the last; each
    // expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, t: &str| Some((l, c, format!("the size for values of type `{t}` cannot be known at compilation time (E0277)")));
    assert_eq!(first("fn iceman(c: Vec<[i32]>) {}\nfn main() {}\n"), e(1, 14, "[i32]"), "Vec");
    assert_eq!(first("trait Foo {}\nimpl Foo for Option<[u8]> {}\nfn main() {}\n"), e(2, 14, "[u8]"), "impl");
    assert_eq!(first("struct A<T>(T);\nstruct B(A<[u8]>);\nfn main() {}\n"), e(2, 10, "[u8]"), "a type of the program's");
    assert_eq!(first("enum E {\n    V([Box<E>]),\n}\nfn main() {}\n"), e(2, 7, "[Box<E>]"), "variant");
    assert_eq!(first("struct S<X: ?Sized> {\n    f1: X,\n    f2: isize,\n}\nfn main() {}\n"), e(2, 9, "X"), "field before the last");
    // (a parameter that says `?Sized`, std's that allow it, the last field)
    let ok = "struct W<T: ?Sized>(Box<T>);\nstruct P<T: ?Sized> {\n    a: u8,\n    b: T,\n}\nfn f(_: W<[u8]>, _: Box<str>, _: std::rc::Rc<dyn Fn()>, _: &P<str>) {}\nfn main() {}\n";
    assert_eq!(first(ok), None, "Sized");
}

#[test]
fn method_of_an_unbounded_type_parameter() {
    // (E0599: a method of a type parameter with no bounds but markers, in a fn nothing instantiates; each expected line
    // and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, m: &str| Some((l, c, format!("no method named `{m}` found for type parameter `T` in the current scope (E0599)")));
    assert_eq!(first("fn foo<T>(t: T) {\n    t.clone();\n}\nfn main() {}\n"), e(2, 7, "clone"), "clone");
    // (behind a reference: rustc names `&T`, zustc the parameter; the place is the same)
    assert_eq!(first("fn foo<T: Send + 'static>(x: &T) {\n    x.method();\n}\nfn main() {}\n").map(|d| (d.0, d.1)), Some((2, 7)), "behind a reference");
    // (a blanket impl's method, std's `into`, a where clause of the impl's)
    assert_eq!(first("trait Ext { fn ext(&self) {} }\nimpl<T> Ext for T {}\nfn foo<T>(t: T) -> u8 {\n    t.ext();\n    let _x: T = t.into();\n    0\n}\nfn main() {}\n"), None, "blanket");
    assert_eq!(
        first("trait Trait { fn method(self) -> isize; }\nstruct W<T> { field: T }\nimpl<'a, T> Trait for &'a W<T> where &'a T: Trait {\n    fn method(self) -> isize {\n        let r: &'a T = &self.field;\n        r.method()\n    }\n}\nfn main() {}\n"),
        None,
        "where clause"
    );
}

#[test]
fn attribute_paths_that_resolve_to_nothing() {
    // (E0433: an attribute written as a path whose first segment is no tool and no module, or a module's that has no
    // such attribute; each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, m: &str| Some((l, c, format!("{m} (E0433)")));
    assert_eq!(first("#[foo::bar]\nfn main() {}\n"), e(1, 3, "cannot find module or crate `foo` in this scope"), "unknown");
    assert_eq!(first("mod existent {}\n\n#[existent::nonexistent]\nfn main() {}\n"), e(3, 13, "cannot find `nonexistent` in `existent`"), "a module's");
    assert_eq!(first("mod rustfmt {}\n\n#[rustfmt::skip]\nfn main() {}\n"), e(3, 12, "cannot find `skip` in `rustfmt`"), "a module shadows a tool");
    // (a tool's attribute)
    assert_eq!(first("#[rustfmt::skip]\n#[allow(clippy::all)]\n#[diagnostic::on_unimplemented(message = \"x\")]\ntrait T {}\nfn main() {}\n"), None, "tools");
}

#[test]
fn local_where_a_constant_goes() {
    // (E0435: a local of a fn in a value a constant holds, or in an array's length; each expected line and column is
    // rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize| Some((l, c, "attempt to use a non-constant value in a constant (E0435)".to_string()));
    assert_eq!(first("fn main() {\n    let foo = 42u32;\n    const FOO: u32 = foo;\n}\n"), e(3, 22), "a const in a fn");
    assert_eq!(first("fn f(x: isize) {\n    static CHILD: isize = x + 1;\n}\nfn main() {}\n"), e(2, 27), "a static, a parameter");
    assert_eq!(first("struct P { v: u8 }\nimpl P {\n    fn get(&self) -> u8 {\n        static V: u8 = self.v;\n        V\n    }\n}\nfn main() {}\n"), e(4, 24), "self");
    assert_eq!(first("fn main() {\n    let n: usize = 0;\n    struct Foo;\n    impl Foo {\n        const N: usize = n;\n    }\n}\n"), e(5, 26), "an associated const");
    assert_eq!(first("fn main() {\n    let foo: isize = 100;\n    enum Stuff {\n        Bar = foo,\n    }\n}\n"), e(4, 15), "a discriminant");
    assert_eq!(first("fn main() {\n    let n = 3;\n    let _a = [0u8; n];\n}\n"), e(3, 20), "a repeat's length");
    assert_eq!(first("fn main() {\n    let n = 3;\n    let _a: [u8; n];\n}\n"), e(3, 18), "an array type's length");
    assert_eq!(first("const fn test_me<T>(a: usize) -> usize {\n    const { a }\n}\nfn main() {}\n"), e(2, 13), "an inline const in a generic fn");
    // (an item of the name, a `let` of the value's own, a `let` in a block already closed)
    assert_eq!(first("const FOO: u32 = 1;\nfn main() {\n    let x = 2;\n    const BAR: u32 = FOO + 1;\n    static Z: u32 = { let x = 3; x };\n    let _ = (x, BAR, Z);\n}\n"), None, "items and the value's own let");
    assert_eq!(first("fn main() {\n    {\n        let a = 1;\n        let _ = a;\n    }\n    const A: i32 = 2;\n    let _ = A;\n}\n"), None, "a closed block's let");
}

#[test]
fn errors_in_a_fn_written_in_another() {
    // (a fn written inside another one is checked hoisted out of it: the type errors that hold wherever it is are
    // reported where it was written; each expected line (and column, where given) is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let at = |src: &str| first(src).map(|d| (d.0, d.1));
    assert_eq!(
        first("fn main() {\n    fn g(n: usize) {\n        let _x = [0; n];\n    }\n    g(1);\n}\n"),
        Some((3, 22, "attempt to use a non-constant value in a constant (E0435)".to_string())),
        "E0435"
    );
    assert_eq!(at("unsafe fn f() {}\nfn main() {\n    fn g() {\n        f();\n    }\n    g();\n}\n"), Some((4, 9)), "E0133");
    assert_eq!(first("fn main() {\n    fn g() -> Missing {\n        loop {}\n    }\n}\n").map(|d| (d.0, d.2)), Some((2, "cannot find type `Missing`".to_string())), "E0412");
    // (E0023, rustc's wording: zustc points at the pattern's path, rustc at its fields)
    assert_eq!(
        first("enum E { A(u8, u8, u8), B }\nfn main() {\n    fn g(e: E) {\n        match e { E::A(a, b) => {}, _ => {} }\n    }\n}\n").map(|d| (d.0, d.2)),
        Some((4, "this pattern has 2 fields, but the corresponding tuple variant has 3 fields (E0023)".to_string())),
        "E0023 of a variant"
    );
    assert_eq!(
        first("struct S(u8, u8);\nfn main() {\n    let S(x) = S(1, 2);\n}\n").map(|d| (d.0, d.2)),
        Some((3, "this pattern has 1 field, but the corresponding tuple struct has 2 fields (E0023)".to_string())),
        "E0023 of a struct"
    );
    // (a tuple pattern of a unit variant (E0532) or a struct variant (E0164), at its path)
    assert_eq!(
        first("enum C { A, B(u8) }\nfn g(c: C) { if let C::A(..) = c {} }\nfn main() {}\n"),
        Some((2, 21, "expected tuple struct or tuple variant, found unit variant `C::A` (E0532)".to_string())),
        "E0532"
    );
    assert_eq!(
        first("enum C { D { x: u8 }, E }\nfn g(c: C) { match c { C::D(x) => {}, _ => {} } }\nfn main() {}\n"),
        Some((2, 24, "expected tuple struct or tuple variant, found struct variant `C::D` (E0164)".to_string())),
        "E0164"
    );
    assert_eq!(
        first("use C::D;\nenum C { D { x: u8 }, A }\nfn g(c: C) { match c { D(x) => {}, _ => {} } }\nfn main() {}\n"),
        Some((3, 24, "cannot find tuple struct or tuple variant `D` in this scope (E0532)".to_string())),
        "a struct variant by its name alone"
    );
    // (`Self` of the impl around it, which rustc takes from an outer item with a warning)
    assert_eq!(first("struct S0(usize);\nimpl S0 {\n    fn foo() {\n        fn bar() -> S0 {\n            Self(0)\n        }\n    }\n}\nfn main() {}\n"), None, "Self");
}

#[test]
fn diff_markers_and_mismatched_delimiters() {
    // (rustc 1.100's lexer and parser errors on the same source: a merge's diff marker at its line's first column; a
    // closing delimiter that does not close the innermost open one at its line, where rustc labels it)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let marker = |l: usize| Some((l, 1, "encountered diff marker".to_string()));
    assert_eq!(first("fn main() {\n<<<<<<< HEAD\n    let _a = 1;\n=======\n    let _b = 2;\n>>>>>>> branch\n}\n"), marker(2), "in a body");
    assert_eq!(first("struct S {\n<<<<<<< HEAD\n    x: u8,\n=======\n    y: u8,\n>>>>>>> branch\n}\nfn main() {}\n"), marker(2), "in a struct");
    assert_eq!(first("use std::{\n<<<<<<< HEAD\n    fmt,\n=======\n    io,\n>>>>>>> branch\n};\nfn main() {}\n"), marker(2), "in a use tree");
    // (not in a string or a comment)
    assert_eq!(first("fn main() {\n    let _s = \"\n<<<<<<< HEAD\n\";\n    /*\n<<<<<<< HEAD\n    */\n}\n"), None, "in a string");
    let mismatch = |src: &str| first(src).map(|d| (d.0, d.2));
    assert_eq!(mismatch("impl A {\n    fn b(self>\n}\n\nfn main() {}\n"), Some((3, "mismatched closing delimiter: `}`".to_string())), "brace for a paren");
    assert_eq!(
        mismatch("trait Foo {\n    fn bar() {\n        let x = foo();\n    )\n}\nfn main() {}\n"),
        Some((4, "mismatched closing delimiter: `)`".to_string())),
        "paren for a brace"
    );
    // (a closing delimiter none open matches is skipped; the one left over with none open is "unexpected", in place of
    // the mismatch)
    assert_eq!(
        mismatch("fn main() {\n    if 1 < 2 {\n        let _a = vec!];\n    }\n}\n"),
        Some((5, "unexpected closing delimiter: `}`".to_string())),
        "unexpected"
    );
    // (an unclosed one at the end is the lexer's other error, before a diff marker)
    assert_eq!(first("mod tests {\n<<<<<<< HEAD\n    fn a() {\n=======\n    fn b() {\n>>>>>>> x\n    }\n}\n").map(|d| d.2), Some("this file contains an unclosed delimiter".to_string()), "unclosed");
}

#[test]
fn unnamed_imports_atomic_orderings_and_variant_bindings() {
    // (E0431, invalid_atomic_ordering and bindings_with_variant_name (E0170); each expected line and column is
    // rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let unnamed = |l: usize, c: usize| Some((l, c, "imports need to be explicitly named".to_string()));
    assert_eq!(first("use {self};\nfn main() {}\n"), unnamed(1, 6), "use {{self}}");
    assert_eq!(first("use self;\nfn main() {}\n"), unnamed(1, 5), "use self");
    assert_eq!(first("use crate::{self};\nfn main() {}\n"), unnamed(1, 13), "crate");
    // (a module's `self`, one with a name, `use ::{self}` (another error))
    assert_eq!(first("use std::fmt::{self, Write};\nuse crate::{self as y};\nfn main() {}\n"), None, "named");
    assert_eq!(
        first("use std::sync::atomic::{AtomicUsize, Ordering};\nfn main() {\n    let x = AtomicUsize::new(0);\n    x.store(1, Ordering::Acquire);\n}\n"),
        Some((4, 16, "atomic stores cannot have `Acquire` or `AcqRel` ordering".to_string())),
        "store"
    );
    assert_eq!(
        first("use std::sync::atomic::{AtomicI32, Ordering};\nfn main() {\n    let x = AtomicI32::new(0);\n    let _ = x.compare_exchange(0, 1, Ordering::SeqCst, Ordering::AcqRel);\n}\n"),
        Some((4, 56, "`compare_exchange`'s failure ordering may not be `Release` or `AcqRel`, since a failed `compare_exchange` does not result in a write".to_string())),
        "compare_exchange"
    );
    assert_eq!(
        first("use std::sync::atomic::{AtomicBool, Ordering};\nfn main() {\n    let x = AtomicBool::new(true);\n    let _ = x.load(Ordering::Acquire);\n    x.store(false, Ordering::Release);\n}\n"),
        None,
        "orderings it can have"
    );
    assert_eq!(
        first("enum E { A, B }\nfn f(e: E) -> u8 {\n    match e {\n        A => 1,\n    }\n}\nfn main() {}\n"),
        Some((4, 9, "pattern binding `A` is named the same as one of the variants of the type `E` (E0170)".to_string())),
        "a variant's name"
    );
    // (allowed by the crate, or the variants in scope by a `use`)
    assert_eq!(first("#![allow(bindings_with_variant_name)]\nenum E { A, B }\nfn f(e: E) -> u8 {\n    match e {\n        A => 1,\n    }\n}\nfn main() {}\n"), None, "allowed");
    assert_eq!(first("mod m {\n    pub enum E { A, B }\n}\nuse m::E::*;\nfn f(e: m::E) -> u8 {\n    match e {\n        A => 1,\n        B => 2,\n    }\n}\nfn main() {}\n"), None, "imported");
}

#[test]
fn stricter_bounds_than_the_trait() {
    // (E0276: an impl's method whose own type parameter has a bound the trait's method does not give; each expected
    // line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize| Some((l, c, "impl has stricter requirements than trait (E0276)".to_string()));
    assert_eq!(first("trait Foo {\n    fn foo<T>(x: T);\n}\nimpl Foo for bool {\n    fn foo<T>(x: T) where T: Copy {}\n}\nfn main() {}\n"), e(5, 30), "a where clause");
    assert_eq!(
        first("trait A {}\ntrait B: A {}\ntrait Foo {\n    fn f<T: A>(&self);\n    fn g<T: Eq>(&self);\n}\nimpl Foo for u8 {\n    fn f<T: A>(&self) {}\n    fn g<T: Ord>(&self) {}\n}\nfn main() {}\n"),
        e(9, 13),
        "Ord where the trait says Eq"
    );
    // (the trait's bounds' supertraits, and fewer bounds than the trait's)
    assert_eq!(first("trait Foo {\n    fn f<T: Ord + Copy>(&self);\n}\nimpl Foo for u8 {\n    fn f<T: Eq + Clone + PartialOrd>(&self) {}\n}\nfn main() {}\n"), None, "supertraits");
    assert_eq!(first("trait A {}\ntrait B: A {}\ntrait Foo {\n    fn f<T: B>(&self);\n}\nimpl Foo for u8 {\n    fn f<T: A>(&self) {}\n}\nfn main() {}\n"), None, "a supertrait of the program's");
}

#[test]
fn duplicate_items_in_an_impl() {
    // (E0201 in a trait's impl, E0592 in an inherent one: the second item of a name in one namespace, at its start;
    // each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    assert_eq!(
        first("struct Foo;\nimpl Foo {\n    const bar: bool = true;\n    fn bar() {}\n}\nfn main() {}\n"),
        Some((4, 5, "duplicate definitions with name `bar` (E0592)".to_string())),
        "a const and a fn"
    );
    assert_eq!(
        first("trait Bar { fn bar(&self) -> isize; }\nstruct Foo;\nimpl Bar for Foo {\n    fn bar(&self) -> isize { 1 }\n    fn bar(&self) -> isize { 2 }\n}\nfn main() {}\n"),
        Some((5, 5, "duplicate definitions with name `bar`: (E0201)".to_string())),
        "two fns of a trait's impl"
    );
    assert_eq!(
        first("trait Foo { type Bar; }\nstruct Baz;\nimpl Foo for Baz {\n    type Bar = i16;\n    type Bar = u16;\n}\nfn main() {}\n"),
        Some((5, 5, "duplicate definitions with name `Bar`: (E0201)".to_string())),
        "two types"
    );
    // (a `const fn` is a fn; items of other names)
    assert_eq!(first("struct S;\nimpl S {\n    const fn f() {}\n    const F: u8 = 0;\n}\nimpl S {\n    fn g() {}\n}\nfn main() {}\n"), None, "no duplicate");
}

#[test]
fn trait_bounds_that_name_no_trait_in_scope() {
    // (E0405 / E0404 at a fn's bound; each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, n: &str, code: &str| Some((l, c, format!("cannot find trait `{n}` in this scope ({code})")));
    assert_eq!(first("fn f<T: Nope>(_x: T) {}\nfn main() {}\n"), e(1, 9, "Nope", "E0405"), "inline");
    assert_eq!(first("fn f<T>(_x: T) where T: Nope {}\nfn main() {}\n"), e(1, 25, "Nope", "E0405"), "where clause");
    assert_eq!(first("fn f(_x: impl Nope) {}\nfn main() {}\n"), e(1, 15, "Nope", "E0405"), "impl Trait");
    // (std's traits outside its prelude, not imported: E0404 for the ones with a derive macro of the name)
    assert_eq!(first("fn foobar<T: Debug>(_x: T) {}\nfn main() {}\n"), e(1, 14, "Debug", "E0404"), "Debug");
    assert_eq!(first("fn g(_x: impl Display) {}\nfn main() {}\n"), e(1, 15, "Display", "E0405"), "Display");
    // (imported, by a path, a derive)
    assert_eq!(first("use std::fmt::Debug;\nfn f<T: Debug>(_x: T) {}\nfn main() {}\n"), None, "imported");
    assert_eq!(first("use std::fmt;\nfn f<T: fmt::Debug + Clone>(_x: T) {}\n#[derive(Debug)]\nstruct S;\nfn main() {}\n"), None, "a path and a derive");
}

#[test]
fn unstable_items_of_std() {
    // (E0658 of a `use` of an unstable item of std's; each expected line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, f: &str| Some((l, c, format!("use of unstable library feature `{f}` (E0658)")));
    assert_eq!(first("use std::ops::FnPtr;\nfn main() {}\n"), e(1, 5, "fn_static"), "a path");
    assert_eq!(first("use std::marker::{Copy, Reborrow};\nfn main() {}\n"), e(1, 25, "reborrow"), "a braced list");
    assert_eq!(first("use core::io::ErrorKind;\nfn main() {}\n"), e(1, 11, "core_io"), "core::io");
    assert_eq!(first("#[allow(unused_imports)] use core::unicode::UNICODE_VERSION;\nfn main() {}\n"), e(1, 30, "unicode_internals"), "after attributes");
    assert_eq!(
        first("use std::intrinsics;\nfn main() {}\n").map(|d| (d.0, d.1)),
        Some((1, 5)),
        "intrinsics"
    );
    // (a stable item of the module, the feature enabled)
    assert_eq!(first("use std::intrinsics::transmute;\nfn main() {}\n"), None, "a stable item");
    assert_eq!(first("#![feature(reborrow)]\nuse std::marker::Reborrow;\nfn main() {}\n"), None, "enabled");
}

#[test]
fn calls_of_the_fn_traits_methods() {
    // (E0658: `call` / `call_mut` / `call_once` of a closure or an `F: Fn..` parameter need `fn_traits`; each expected
    // line and column is rustc 1.100's on the same source)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize| Some((l, c, "use of unstable library feature `fn_traits` (E0658)".to_string()));
    assert_eq!(first("fn main() {\n    let f = || ();\n    f.call(());\n}\n"), e(3, 7), "a closure's method");
    assert_eq!(first("fn main() {\n    let f = || ();\n    Fn::call(&f, ());\n}\n"), e(3, 5), "by its trait's path");
    assert_eq!(first("fn foo<F: Fn()>(mut f: F) {\n    f.call_mut(());\n}\nfn main() {}\n"), e(2, 7), "a parameter's");
    // (the feature enabled (zustc does not call a closure's `call` yet: another error, not this one); a method of the
    // program's of the name)
    assert_ne!(first("#![feature(fn_traits)]\nfn main() {\n    let f = || ();\n    f.call(());\n}\n").map(|d| d.2), e(4, 7).map(|d| d.2), "enabled");
    assert_eq!(first("trait Call { fn call(&self, _x: ()) {} }\nimpl Call for u8 {}\nfn main() {\n    1u8.call(());\n}\n"), None, "the program's");
    assert_eq!(first("trait FnLike { fn call(&self); }\nfn c<F: FnLike>(f: F) {\n    f.call();\n}\nfn main() {}\n"), None, "a trait of the program's named Fn..");
}

#[test]
fn malformed_link_attributes() {
    // (`link_name` takes `= "name"`, `link_ordinal` an integer: E0539, at the attribute's line as rustc 1.100)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    assert_eq!(first("#[link_name]\nextern \"C\" {\n    fn bar() -> u32;\n}\nfn main() {}\n"), Some((1, "malformed `link_name` attribute input".to_string())), "link_name");
    assert_eq!(first("extern \"C\" {\n    #[link_ordinal(\"x\")]\n    fn foo();\n}\nfn main() {}\n"), Some((2, "malformed `link_ordinal` attribute input".to_string())), "link_ordinal");
    assert_eq!(first("extern \"C\" {\n    #[link_name = \"foo\"]\n    fn bar();\n}\n#[export_name = \"x\"]\npub fn f() {}\nfn main() {}\n"), None, "well formed");
}

#[test]
fn derive_on_an_item_of_another_kind() {
    // (E0774: a derive is a struct's, an enum's or a union's; at the attribute's line as rustc 1.100)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize| Some((l, "`derive` may only be applied to `struct`s, `enum`s and `union`s (E0774)".to_string()));
    assert_eq!(first("trait Foo {\n    #[derive(Clone)]\n    type Bar;\n}\nfn main() {}\n"), e(2), "a trait's item");
    assert_eq!(first("#[derive(Debug)]\nfn f() {}\nfn main() {}\n"), e(1), "a fn");
    assert_eq!(first("extern \"C\" {\n    #[derive(Copy)]\n    fn f();\n}\nfn main() {}\n"), e(2), "an extern block's item");
    assert_eq!(first("#[derive(Clone, Copy)]\nunion U { f: i32 }\n#[derive(Debug)]\nenum E {}\nfn main() {}\n"), None, "a union and an enum");
}

#[test]
fn item_declarations_rustc_rejects() {
    // (E0646, E0128, E0109, E0116, E0091, E0130, E0118, E0229, E0040: errors of an item's own declaration or a call, at the lines rustc 1.100
    // reports them)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(first("fn main() where (): Copy {}\n"), e(1, "`main` function is not allowed to have a `where` clause (E0646)"), "main's where clause");
    let fwd = "generic parameter defaults cannot reference parameters before they are declared (E0128)";
    assert_eq!(first("struct Foo<T = U, U = ()> {\n    a: T,\n    b: U,\n}\nfn main() {}\n"), e(1, fwd), "a later parameter");
    assert_eq!(first("pub struct Foo<Bar = Bar>(Bar);\nfn main() {}\n"), e(1, fwd), "the parameter itself");
    assert_eq!(first("struct Foo<T, U = Vec<T>>(T, U);\nfn main() {}\n"), None, "an earlier parameter");
    assert_eq!(first("type X = u32<i32>;\nfn main() {}\n"), e(1, "type arguments are not allowed on builtin type `u32` (E0109)"), "a builtin type");
    assert_eq!(first("type X = u32<'static>;\nfn main() {}\n"), e(1, "lifetime arguments are not allowed on builtin type `u32` (E0109)"), "a builtin type's lifetime");
    assert_eq!(first("fn is_copy<T: ::std::marker<i32>::Copy>() {}\nfn main() {}\n"), e(1, "type arguments are not allowed on module `marker` (E0109)"), "a module of std");
    let foreign = "cannot define inherent `impl` for a type outside of the crate where the type is defined (E0116)";
    assert_eq!(first("impl Vec<u8> {}\nfn main() {}\n"), e(1, foreign), "a type of std");
    assert_eq!(first("use std::rc::Rc;\npub struct Foo;\npub type Function = Rc<Foo>;\nimpl Function {}\nfn main() {}\n"), e(4, foreign), "through an alias");
    assert_eq!(first("struct Vec<T>(T);\nimpl<T> Vec<T> {\n    fn get(&self) -> &T {\n        &self.0\n    }\n}\nfn main() {}\n"), None, "the crate's own type of that name");
    assert_eq!(first("type Foo<T> = u32;\nfn main() {}\n"), e(1, "type parameter `T` is never used (E0091)"), "an unused parameter");
    assert_eq!(first("type Foo<\n    Unused\n> = u8;\nfn main() {}\n"), e(2, "type parameter `Unused` is never used (E0091)"), "at the parameter's line");
    assert_eq!(first("fn main() {\n    type E<T> = u8;\n}\n"), e(2, "type parameter `T` is never used (E0091)"), "in a fn body");
    assert_eq!(first("type Foo<'a, T: 'a, const N: usize> = [&'a T; 2];\nfn main() {}\n"), None, "lifetimes and consts may go unused");
    let pat = "patterns aren't allowed in foreign function declarations (E0130)";
    assert_eq!(first("extern \"C\" {\n    fn foo((a, b): (u32, u32));\n}\nfn main() {}\n"), e(2, pat), "a tuple pattern");
    assert_eq!(first("extern \"C\" {\n    fn foo(mut a: u32);\n}\nfn main() {}\n"), e(2, pat), "a `mut` binding");
    assert_eq!(first("extern \"C\" {\n    fn foo(a: usize, _: u8);\n}\nfn main() {}\n"), None, "names and `_`");
    assert_eq!(first("impl<T> T {\n    fn get(&self) {}\n}\nfn main() {}\n"), e(1, "no nominal type found for inherent implementation (E0118)"), "a type parameter");
    assert_eq!(first("type A<T> = B;\ntype B = _;\nfn main() {}\n").map(|d| d.0), Some(2), "an alias of an alias rustc rejects");
    let cons = "associated item constraints are not allowed here (E0229)";
    assert_eq!(first("trait A {}\nstruct Foo;\npub trait D {\n    fn f<T>(self)\n        where T<Bogus = Foo>: A;\n}\nfn main() {}\n"), e(5, cons), "on a type parameter");
    assert_eq!(first("trait Foo {\n    type A;\n}\nstruct Bar;\nfn baz<I: Foo>(x: &<I as Foo<A = Bar>>::A) {}\nfn main() {}\n"), e(5, cons), "in a qualified path");
    assert_eq!(first("fn f<I: Iterator<Item = u8>>(i: I) where I: Clone {}\nfn main() {}\n"), None, "in a bound");
    let dtor = "explicit use of destructor method (E0040)";
    let foo = "struct Foo;\nimpl Drop for Foo {\n    fn drop(&mut self) {}\n}\n";
    assert_eq!(first(&format!("{foo}fn main() {{\n    let mut x = Foo;\n    x.drop();\n}}\n")), e(7, dtor), "a method call");
    assert_eq!(first(&format!("{foo}fn main() {{\n    Drop::drop(&mut Foo);\n}}\n")), e(6, dtor), "a path call");
    assert_eq!(first(&format!("{foo}fn main() {{\n    let x = Foo;\n    drop(x);\n}}\n")), None, "std's drop");
}

#[test]
fn returns_ranges_and_receivers_rustc_rejects() {
    // (E0572, E0586, E0185 / E0186, at the lines rustc 1.100 reports them)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    let ret = "return statement outside of function body (E0572)";
    assert_eq!(first("const FOO: u32 = return 0;\nfn main() {}\n"), e(1, ret), "a const's value");
    assert_eq!(first("const X: () = {\n    return;\n};\nfn main() {}\n"), e(2, ret), "a const's block");
    assert_eq!(first("enum Bug {\n    V1 = return [0][0],\n}\nfn main() {}\n"), e(2, ret), "a discriminant");
    assert_eq!(first("fn main() {\n    [(); return];\n}\n"), e(2, ret), "a repeat's length");
    assert_eq!(first("const F: fn() -> u8 = || { return 1; };\nconst G: u8 = { fn g() -> u8 { return 2; } g() };\nfn main() {}\n"), None, "a closure's or a fn's body in a const");
    let no_end = "inclusive range with no end (E0586)";
    assert_eq!(first("fn main() {\n    let v = [1, 2, 3];\n    let x = &v[1..=];\n}\n"), e(3, no_end), "an index");
    assert_eq!(first("fn main() {\n    for _ in 1..= {}\n}\n"), e(2, no_end), "a for loop's head");
    assert_eq!(first("fn main() {\n    match 3u8 {\n        0..= => {}\n        _ => {}\n    }\n}\n"), e(3, no_end), "a pattern");
    assert_eq!(first("fn main() {\n    let v = [1, 2, 3];\n    let _ = &v[..=1];\n    let _ = &v[1..];\n}\n"), None, "ranges with an end or exclusive");
    let tr = "trait Foo {\n    fn foo();\n    fn bar(&self);\n}\nstruct Bar;\n";
    assert_eq!(first(&format!("{tr}impl Foo for Bar {{\n    fn foo(&self) {{}}\n    fn bar(&self) {{}}\n}}\nfn main() {{}}\n")), e(7, "method `foo` has a `&self` declaration in the impl, but not in the trait (E0185)"), "a receiver in the impl only");
    assert_eq!(first(&format!("{tr}impl Foo for Bar {{\n    fn foo() {{}}\n    fn bar() {{}}\n}}\nfn main() {{}}\n")), e(8, "method `bar` has a `&self` declaration in the trait, but not in the impl (E0186)"), "a receiver in the trait only");
}

#[test]
fn link_names_and_members_rustc_rejects() {
    // (E0454, E0459, E0648, E0437, E0438, at the lines rustc 1.100 reports them)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(first("#[export_name = \"\\0foo\"]\npub fn bar() {}\nfn main() {}\n"), e(1, "`export_name` may not contain null characters (E0648)"), "an export name's NUL");
    assert_eq!(first("#[export_name = \"\"]\npub fn bar() {}\nfn main() {}\n"), e(1, "`export_name` may not be empty"), "an empty export name");
    assert_eq!(first("#[link(name = \"\")]\nextern \"C\" {}\nfn main() {}\n"), e(1, "link name must not be empty (E0454)"), "an empty link name");
    assert_eq!(first("#[link(name = \"a\\0\")]\nextern \"C\" {}\nfn main() {}\n"), e(1, "link name may not contain null characters (E0648)"), "a link name's NUL");
    assert_eq!(first("extern \"C\" {\n    #[link_name = \"\"]\n    fn f();\n}\nfn main() {}\n"), e(2, "link name must not be empty (E0454)"), "an empty link_name");
    assert_eq!(first("#[link(kind = \"dylib\")]\nextern \"C\" {}\nfn main() {}\n"), e(1, "`#[link]` attribute requires a `name = \"string\"` argument (E0459)"), "a link with no name");
    assert_eq!(first("#[link(name = \"  \")]\nextern \"C\" {}\n#[export_name = \"  \"]\npub fn bar() {}\nfn main() {}\n"), None, "names of spaces");
    assert_eq!(first("trait Foo {}\nimpl Foo for i32 {\n    type Bar = bool;\n}\nfn main() {}\n"), e(3, "type `Bar` is not a member of trait `Foo` (E0437)"), "a type the trait does not declare");
    assert_eq!(first("trait Bar {}\nimpl Bar for i32 {\n    const BAR: bool = true;\n}\nfn main() {}\n"), e(3, "const `BAR` is not a member of trait `Bar` (E0438)"), "a const the trait does not declare");
    assert_eq!(first("trait Tr {\n    type A;\n    const C: u8;\n}\nimpl Tr for i32 {\n    type A = u8;\n    const C: u8 = 1;\n}\nimpl Iterator for Box<u8> {\n    type Item = u8;\n    fn next(&mut self) -> Option<u8> { None }\n}\nfn main() {}\n").map(|d| d.1.contains("not a member")), None, "declared members, a std trait");
    assert_eq!(first("trait Sealed {\n    #[doc(hidden)]\n    type Buffer: Copy;\n}\nimpl Sealed for u8 {\n    type Buffer = u8;\n}\nfn main() {}\n"), None, "a member with an attribute");
    assert_eq!(first("#[link(kind = \"wonderful_unicorn\")]\nextern \"C\" {}\nfn main() {}\n"), e(1, "malformed `link` attribute input"), "a kind rustc does not know, before the name");
    // (E0197, E0084, E0777, E0184)
    assert_eq!(first("struct Foo;\nunsafe impl Foo {}\nfn main() {}\n"), e(2, "inherent impls cannot be unsafe (E0197)"), "an unsafe inherent impl");
    assert_eq!(first("unsafe trait U {}\nstruct Foo;\nunsafe impl U for Foo {}\nfn main() {}\n"), None, "an unsafe trait's impl");
    assert_eq!(first("#[repr(i32)]\nenum Foo {}\nfn main() {}\n"), e(1, "unsupported representation for zero-variant enum (E0084)"), "a zero-variant enum's repr");
    assert_eq!(first("#[repr(i32)]\nenum Foo { A }\nenum Bar {}\nfn main() {}\n"), None, "an enum with a variant, one with no repr");
    assert_eq!(first("#[derive(\"Clone\")]\nstruct Foo;\nfn main() {}\n"), e(1, "expected path to a trait, found literal (E0777)"), "a literal in a derive");
    assert_eq!(first("#[derive(Copy, Clone)]\nstruct Foo;\nimpl Drop for Foo {\n    fn drop(&mut self) {}\n}\nfn main() {}\n"), e(2, "the trait `Copy` cannot be implemented for this type; the type has a destructor (E0184)"), "Copy and Drop");
}

#[test]
fn elided_lifetimes_and_foreign_bodies() {
    // (E0726, and a fn with a body in an extern block, at the lines rustc 1.100 reports them)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    let lt = "implicit elided lifetime not allowed here (E0726)";
    assert_eq!(first("trait MyTrait {}\nstruct Foo<'a> {\n    x: &'a u32,\n}\nimpl MyTrait for Foo {}\nfn main() {}\n"), e(5, lt), "the self type's lifetime");
    assert_eq!(first("trait MyTrait<'a> {}\nimpl MyTrait for u32 {}\nfn main() {}\n"), e(2, lt), "the trait's lifetime");
    assert_eq!(first("struct HasLifetime<'a>(&'a bool);\nasync fn error(lt: HasLifetime) {\n    if *lt.0 {}\n}\nfn main() {}\n"), e(2, lt), "an async fn's parameter");
    assert_eq!(
        first("trait MyTrait<'a> {}\nstruct Foo<'a>(&'a u8);\nimpl<'a> MyTrait<'a> for Foo<'a> {}\nimpl MyTrait<'_> for &Foo<'_> {}\nfn no_error(x: Foo) -> u8 { *x.0 }\nfn main() {}\n"),
        None,
        "lifetimes written, and a plain fn's elided one"
    );
    assert_eq!(first("extern \"C\" {\n    fn foo() -> i32 {\n        0\n    }\n}\nfn main() {}\n"), e(2, "incorrect function inside `extern` block"), "a foreign fn with a body");
}

#[test]
fn type_parameter_counts_against_the_trait() {
    // (E0049, at the impl's method as rustc 1.100)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(
        first("trait Foo {\n    fn foo<T: Default>(x: T) -> Self;\n}\nstruct Bar;\nimpl Foo for Bar {\n    fn foo(x: bool) -> Self { Bar }\n}\nfn main() {}\n"),
        e(6, "associated function `foo` has 0 type parameters but its trait declaration has 1 type parameter (E0049)"),
        "fewer"
    );
    assert_eq!(
        first("trait Fuzz {\n    fn fuzz<A: Default, B>(&self, x: A, y: B);\n}\nstruct Baz;\nimpl Fuzz for Baz {\n    fn fuzz(&self, x: bool, y: bool) {}\n}\nfn main() {}\n"),
        e(6, "method `fuzz` has 0 type parameters but its trait declaration has 2 type parameters (E0049)"),
        "a method"
    );
    assert_eq!(first("trait Foo {\n    fn foo<T: Default>(x: T) -> T;\n}\nstruct Bar;\nimpl Foo for Bar {\n    fn foo<U: Default>(x: U) -> U { x }\n}\nfn main() {}\n"), None, "the same count");
    assert_eq!(
        first("trait Foo {\n    fn foo<T>();\n}\nstruct Bar;\nimpl Foo for Bar {\n    fn foo<const N: u8>() {}\n}\nfn main() {}\n").map(|d| d.1.contains("E0049")),
        None,
        "a const parameter for a type parameter: another error"
    );
}

#[test]
fn item_errors_at_the_part_rustc_points_at() {
    // (E0072 at the type's name, E0746 at the return type, a fn's `static X: T;` at its line)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(first("#[derive(Hash)]\npub enum Elem {\n    A(Elem),\n}\nfn main() {}\n"), e(2, "recursive type `Elem` has infinite size"), "past the derive");
    assert_eq!(
        first("pub trait R {}\nfn create(_: &()) ->\n    dyn R\n{\n    unimplemented!()\n}\nfn main() {}\n"),
        e(3, "return type cannot be a trait object without pointer indirection (E0746)"),
        "the return type's line"
    );
    assert_eq!(first("fn foo() {\n    static symbol: [u32];\n}\nfn main() {}\n").map(|d| d.0), Some(2), "a static without a body in a fn");
}

#[test]
fn awaits_and_binders_where_rustc_points() {
    // (E0728 at the `await`; a `for<..>` binder as rustc's parser reads it)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    assert_eq!(
        first("fn main() {\n    async {\n        let _ = 1;\n    }.await\n}\n"),
        Some((4, "`await` is only allowed inside `async` functions and blocks (E0728)".to_string())),
        "the `await`'s line, not the next token's"
    );
    assert_eq!(first("type A = for<'a:> fn();\ntype B = for<'a, 'b> fn();\nfn main() {}\n"), None, "a binder's lifetimes, an empty bound");
    assert_eq!(first("type A = for<,> fn();\nfn main() {}\n").map(|d| d.0), Some(1), "a comma alone");
    // (rustc rejects a binder's bounds and type parameters after parsing: a later syntax error comes first)
    assert_eq!(first("type A = for<'b, 'a: 'b> fn();\nfn main() {}\n"), Some((1, "bounds cannot be used in this context".to_string())), "a binder's bounds");
    assert_eq!(first("type D = for<'a, T> fn();\nfn main() {}\n"), Some((1, "only lifetime parameters can be used in this context".to_string())), "a binder's type parameter");
    assert_eq!(first("type A = for<'a: 'b> fn();\ntype B = for<,> fn();\nfn main() {}\n").map(|d| d.0), Some(2), "the later syntax error first");
}

#[test]
fn macro_fragments_and_unit_patterns() {
    // (a matcher's fragment specifier rustc does not know, E0533, at the lines rustc 1.100 reports them)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(first("macro_rules! test {\n    ($wrong:id) => {};\n}\nfn main() {}\n"), e(2, "invalid fragment specifier `id`"), "an unknown fragment");
    assert_eq!(
        first("macro_rules! m {\n    ($($x:expr),* ; $t:ty, $p:pat_param, $l:lifetime) => { $x:id };\n}\nfn main() {}\n"),
        None,
        "known fragments, and the transcriber's text"
    );
    assert_eq!(first("fn main() {\n    macro_rules! m {\n        ($x:guard) => {};\n    }\n}\n"), e(3, "`guard` fragments in macro are unstable (E0658)"), "a gated fragment");
    assert_eq!(first("pub mod break {\n}\nfn main() {}\n"), e(1, "expected identifier, found keyword `break`"), "a keyword for a module's name");
    assert_eq!(first("mod breaks {\n    pub fn f() {}\n}\nfn main() { breaks::f(); }\n"), None, "a name a keyword starts");
    assert_eq!(first("#![some_nonexistent_attribute]\nfn main() {}\n"), e(1, "cannot find attribute `some_nonexistent_attribute` in this scope"), "an unknown inner attribute");
    assert_eq!(first("#![allow(dead_code)]\n#![recursion_limit = \"256\"]\n#![no_implicit_prelude]\nfn main() {}\n").map(|d| d.1.contains("cannot find attribute")), None, "known inner attributes");
    assert_eq!(
        first("enum E {\n    T(i32),\n    S { s: i32 },\n}\nfn f(e: E) -> u8 {\n    match e {\n        E::T => 0,\n        _ => 1,\n    }\n}\nfn main() {}\n"),
        e(7, "expected unit struct, unit variant or constant, found tuple variant `E::T` (E0533)"),
        "a tuple variant as a path pattern"
    );
}

#[test]
fn names_defined_twice_in_one_item_or_by_macro_export() {
    // (E0428 of two variants, two foreign items, two `#[macro_export]` macros, as rustc 1.100)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, n: &str| Some((l, format!("the name `{n}` is defined multiple times (E0428)")));
    assert_eq!(first("enum E {\n    A,\n    B,\n    A,\n}\nfn main() {}\n"), e(4, "A"), "two variants");
    assert_eq!(first("fn main() {\n    enum E {\n        A,\n        A(u8),\n    }\n}\n"), e(4, "A"), "in a fn's enum");
    assert_eq!(first("extern \"C\" {\n    fn foo();\n    pub\n    fn foo();\n}\nfn main() {}\n"), e(3, "foo"), "two foreign fns, at the `pub`");
    assert_eq!(first("#[macro_export]\nmacro_rules! foo { () => {} }\n#[macro_export]\nmacro_rules! foo { () => {} }\nfn main() {}\n"), e(4, "foo"), "two exported macros");
    assert_eq!(first("macro_rules! foo { () => {} }\nmacro_rules! foo { () => {} }\nfn main() { foo!(); }\n"), None, "a macro shadowing one, neither exported");
}

#[test]
fn keyword_labels_and_limit_values() {
    // (rustc 1.100's `eat_label` and limit attributes, at their place)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let kw = |l: usize, c: usize| Some((l, c, "labels cannot use keyword names".to_string()));
    assert_eq!(first("fn main() {\n    'static: loop {}\n}\n"), kw(2, 5), "a label definition");
    assert_eq!(first("fn main() {\n    'a: loop {\n        break '_;\n    }\n}\n"), kw(3, 15), "at the `break`");
    assert_eq!(first("fn main() {\n    'a: loop {\n        continue 'while;\n    }\n}\n"), kw(3, 18), "at the `continue`");
    assert_eq!(first("fn main() {\n    'r#break: loop {\n        break 'r#break;\n    }\n    'a: while false { continue 'a; }\n}\n"), None, "a raw name, and an ordinary one");
    let lim = |l: usize| Some((l, 1, "`limit` must be a non-negative integer".to_string()));
    assert_eq!(first("#![recursion_limit = \"-1\"]\nfn main() {}\n"), lim(1), "a negative limit");
    assert_eq!(first("// a limit\n#![type_length_limit = \"\"]\nfn main() {}\n"), lim(2), "an empty one");
    assert_eq!(first("#![recursion_limit = \"99999999999999999999999\"]\nfn main() {}\n"), lim(1), "one too large");
    assert_eq!(first("#![recursion_limit = \"256\"]\nfn main() {}\n"), None, "a limit");
}

#[test]
fn generic_parameter_defaults_where_rustc_rejects_them() {
    // (rustc 1.100: a const parameter's default on a fn or an impl; a default before a parameter without one)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let here = |l: usize, c: usize| Some((l, c, "defaults for generic parameters are not allowed here".to_string()));
    let trail = |l: usize, c: usize| Some((l, c, "generic parameters with a default must be trailing".to_string()));
    assert_eq!(first("fn foo<const N: usize = 5>() {}\nfn main() {}\n"), here(1, 8), "a fn's, at the `const`");
    assert_eq!(first("struct Foo<const N: usize>;\nimpl<const N: usize = 1> Foo<N> {}\nfn main() {}\n"), here(2, 6), "an impl's");
    assert_eq!(first("struct S;\nimpl S {\n    fn f<const N: u8 = 1>() {}\n}\nfn main() {}\n"), here(3, 10), "a method's");
    assert_eq!(first("struct A<T = u32, const N: usize> {\n    arg: T,\n}\nfn main() {}\n"), trail(1, 10), "a type parameter's");
    assert_eq!(first("struct Foo<const N: u8 = 3, T>(T);\nfn main() {}\n"), trail(1, 18), "a const parameter's, at its name");
    assert_eq!(first("trait Tr<T = u8, U> { fn f(&self, u: U) -> T; }\nfn main() {}\n"), trail(1, 10), "a trait's");
    assert_eq!(first("struct P<'a, T = u8, const N: usize = 2>(&'a T);\nenum E<T, U = T> { A(T, U) }\nfn main() {}\n"), None, "trailing defaults");
}

/// The trait solver's `No` of a goal with no type parameter in it (`Vec<u8>: Foo` with `impl<T: Bar> Foo for Vec<T>`
/// and no `u8: Bar`) is reported from a generic fn nothing calls: it holds whatever the fn's parameters are
#[test]
fn trait_solver_no_in_uncalled_generic_fn() {
    let src = "trait Foo {}\ntrait Bar {}\nimpl<T: Bar> Foo for Vec<T> {}\nfn needs_foo<T: Foo>(_: T) {}\nfn hello<T>(_: T) {\n    needs_foo(Vec::<u8>::new());\n}\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].line, 6, "{d:?}");
    assert!(d[0].msg.contains("Vec<u8>: Foo") && d[0].msg.contains("not satisfied"), "{d:?}");
    // (with `impl Bar for u8` the goal holds)
    let ok = src.replace("trait Bar {}\n", "trait Bar {}\nimpl Bar for u8 {}\n");
    assert!(s.check(&ok).is_empty());
}

/// An auto trait of a type parameter holds only by its bounds (also through a supertrait of the program's), and a
/// struct is Send when its fields are, also when it contains itself (coinductive)
#[test]
fn trait_solver_auto_traits_of_type_parameters() {
    let src = "fn is_send<T: Send>(_: T) {}\ntrait Shared: Send {}\nstruct List<T> { v: T, next: Option<Box<List<T>>> }\nfn by_sync<S: Sync>(s: S) {\n    is_send(s);\n}\nfn by_super<S: Shared>(s: S) {\n    is_send(s);\n}\nfn by_list<S: Send>(s: List<S>) {\n    is_send(s);\n}\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].line, 5, "{d:?}");
    assert!(d[0].msg.contains("cannot be sent between threads safely"), "{d:?}");
}

/// `-> impl Tr` of a scalar or `()` for std's traits the solver believes `implements` about: zustc accepts exactly
/// what rustc accepts (the expected column is rustc 1.100.0's answer for each)
#[test]
fn trait_solver_opaque_std_traits_like_rustc() {
    let cases: &[(&str, &str, bool)] = &[
        ("1u8", "Copy", true),
        ("1u8", "Clone", true),
        ("1u8", "std::fmt::Debug", true),
        ("1u8", "std::fmt::Display", true),
        ("1u8", "Default", true),
        ("1u8", "PartialEq", true),
        ("1u8", "Eq", true),
        ("1u8", "PartialOrd", true),
        ("1u8", "Ord", true),
        ("1u8", "std::hash::Hash", true),
        ("1u8", "Send", true),
        ("1u8", "Sync", true),
        ("1i32", "Copy", true),
        ("1i32", "Clone", true),
        ("1i32", "std::fmt::Debug", true),
        ("1i32", "std::fmt::Display", true),
        ("1i32", "Default", true),
        ("1i32", "PartialEq", true),
        ("1i32", "Eq", true),
        ("1i32", "PartialOrd", true),
        ("1i32", "Ord", true),
        ("1i32", "std::hash::Hash", true),
        ("1i32", "Send", true),
        ("1i32", "Sync", true),
        ("1u64", "Copy", true),
        ("1u64", "Clone", true),
        ("1u64", "std::fmt::Debug", true),
        ("1u64", "std::fmt::Display", true),
        ("1u64", "Default", true),
        ("1u64", "PartialEq", true),
        ("1u64", "Eq", true),
        ("1u64", "PartialOrd", true),
        ("1u64", "Ord", true),
        ("1u64", "std::hash::Hash", true),
        ("1u64", "Send", true),
        ("1u64", "Sync", true),
        ("1.0f64", "Copy", true),
        ("1.0f64", "Clone", true),
        ("1.0f64", "std::fmt::Debug", true),
        ("1.0f64", "std::fmt::Display", true),
        ("1.0f64", "Default", true),
        ("1.0f64", "PartialEq", true),
        ("1.0f64", "Eq", false),
        ("1.0f64", "PartialOrd", true),
        ("1.0f64", "Ord", false),
        ("1.0f64", "std::hash::Hash", false),
        ("1.0f64", "Send", true),
        ("1.0f64", "Sync", true),
        ("true", "Copy", true),
        ("true", "Clone", true),
        ("true", "std::fmt::Debug", true),
        ("true", "std::fmt::Display", true),
        ("true", "Default", true),
        ("true", "PartialEq", true),
        ("true", "Eq", true),
        ("true", "PartialOrd", true),
        ("true", "Ord", true),
        ("true", "std::hash::Hash", true),
        ("true", "Send", true),
        ("true", "Sync", true),
        ("'a'", "Copy", true),
        ("'a'", "Clone", true),
        ("'a'", "std::fmt::Debug", true),
        ("'a'", "std::fmt::Display", true),
        ("'a'", "Default", true),
        ("'a'", "PartialEq", true),
        ("'a'", "Eq", true),
        ("'a'", "PartialOrd", true),
        ("'a'", "Ord", true),
        ("'a'", "std::hash::Hash", true),
        ("'a'", "Send", true),
        ("'a'", "Sync", true),
        ("()", "Copy", true),
        ("()", "Clone", true),
        ("()", "std::fmt::Debug", true),
        ("()", "std::fmt::Display", false),
        ("()", "Default", true),
        ("()", "PartialEq", true),
        ("()", "Eq", true),
        ("()", "PartialOrd", true),
        ("()", "Ord", true),
        ("()", "std::hash::Hash", true),
        ("()", "Send", true),
        ("()", "Sync", true),
        ("1usize", "Copy", true),
        ("1usize", "Clone", true),
        ("1usize", "std::fmt::Debug", true),
        ("1usize", "std::fmt::Display", true),
        ("1usize", "Default", true),
        ("1usize", "PartialEq", true),
        ("1usize", "Eq", true),
        ("1usize", "PartialOrd", true),
        ("1usize", "Ord", true),
        ("1usize", "std::hash::Hash", true),
        ("1usize", "Send", true),
        ("1usize", "Sync", true),
        ("1i128", "Copy", true),
        ("1i128", "Clone", true),
        ("1i128", "std::fmt::Debug", true),
        ("1i128", "std::fmt::Display", true),
        ("1i128", "Default", true),
        ("1i128", "PartialEq", true),
        ("1i128", "Eq", true),
        ("1i128", "PartialOrd", true),
        ("1i128", "Ord", true),
        ("1i128", "std::hash::Hash", true),
        ("1i128", "Send", true),
        ("1i128", "Sync", true),
        ("1.0f32", "Copy", true),
        ("1.0f32", "Clone", true),
        ("1.0f32", "std::fmt::Debug", true),
        ("1.0f32", "std::fmt::Display", true),
        ("1.0f32", "Default", true),
        ("1.0f32", "PartialEq", true),
        ("1.0f32", "Eq", false),
        ("1.0f32", "PartialOrd", true),
        ("1.0f32", "Ord", false),
        ("1.0f32", "std::hash::Hash", false),
        ("1.0f32", "Send", true),
        ("1.0f32", "Sync", true),
    ];
    let mut bad = vec![];
    for &(v, tr, ok) in cases {
        let src = format!("fn f() -> impl {tr} {{ {v} }}\nfn main() {{ let _ = f(); }}\n");
        let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
        let d = s.check(&src);
        // (a no is the bound's own error, naming the trait: not another error of the program)
        let names_bound = |m: &str| m.contains(tr.rsplit("::").next().unwrap_or(tr)) && (m.contains("not satisfied") || m.contains("E0277"));
        if d.is_empty() != ok || (!ok && !d.iter().any(|x| names_bound(&x.msg))) {
            bad.push(format!("{v}: {tr}: rustc {ok}, zustc {d:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// A type parameter is Copy only by its bounds (a method's own where clause among them): a call needing it from a
/// method without one is E0277, reported from the generic method nothing calls
#[test]
fn trait_solver_placeholder_lacks_copy() {
    let src = "fn require_copy<T: Copy>(_x: T) {}\nstruct Bar<T> {\n    x: T,\n}\nimpl<T> Bar<T> {\n    fn needs_copy(self) where T: Copy {\n        require_copy(self.x);\n    }\n    fn fails_copy(self) {\n        require_copy(self.x);\n    }\n}\nfn main() {}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].line, 10, "{d:?}");
    assert!(d[0].msg.contains("`T: Copy` is not satisfied"), "{d:?}");
}

/// `b.clone()` of a Box whose integer literal is not settled yet keeps it open for what the clone must be
/// (`Box<isize>`), and a clone coerced to a trait object does not make the receiver one
#[test]
fn clone_of_box_keeps_its_literal_open() {
    let src = "#[derive(Clone)]\nstruct B1;\ntrait A {}\nimpl A for B1 {}\nstruct Holder { a: Box<isize> }\nfn main() {\n    let k = Box::new(22);\n    let h = Holder { a: k.clone() };\n    let v: Box<_> = Box::new(B1);\n    let _c: Box<dyn A> = v.clone();\n    let _ = (h.a, k, v);\n}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert!(d.is_empty(), "{d:?}");
}

/// A struct with an unsized tail (`Foo<[u8]>`): `&Foo<[u8; 3]>` coerces to it (through a let, a generic call, a
/// box), the literal's elements take the slice's element type, and a pattern on the pointee checks
#[test]
fn dst_struct_tail_coercions_check() {
    let src = "struct Foo<T: ?Sized> { a: i32, inner: T }\nfn g<T: Copy>(x: &Foo<[T]>) -> T { x.inner[0] }\nfn main() {\n    let f: &Foo<[u8]> = &Foo { a: 32, inner: [1, 2, 3] };\n    let Foo { a, inner } = f;\n    let _ = (a, inner.len(), g(f));\n    let _: u8 = g(&Foo { a: 0, inner: [1u8, 2] });\n    let b: Box<Foo<[u8; 2]>> = Box::new(Foo { a: 1, inner: [4, 5] });\n    let b: Box<Foo<[u8]>> = b;\n    let _ = b.inner.len();\n}\n";
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let d = s.check(src);
    assert!(d.is_empty(), "{d:?}");
}

/// A method's own `where Self: Pair<u8, u16> + Parent` (nested commas) gives `Self: Parent`; an uninferred const
/// argument takes the value a bound meets, so the impl's where clause on it is checked with that value
#[test]
fn review_where_self_nested_and_const_refined() {
    let mut s = Session2::new(Options2 { jobs: 1, ..Options2::default() });
    let src = "trait Parent { type X; }\ntrait Pair<A, B> {}\ntrait Child {\n    fn get(&self) -> <Self as Parent>::X where Self: Pair<u8, u16> + Parent;\n}\nfn main() {}\n";
    assert!(s.check(src).is_empty(), "{:?}", s.check(src));
    let src = "struct Const<const N: usize>;\ntrait Bar<const N: usize> {}\nimpl Bar<3> for () {}\ntrait Foo<const N: usize> {}\nimpl<const N: usize> Foo<N> for Const<N> where (): Bar<N> {}\nfn f(_: impl Foo<3>) {}\nfn main() { f(Const); }\n";
    assert!(s.check(src).is_empty(), "{:?}", s.check(src));
}

/// Third review of #128, each checked against rustc: a trait's own `where Self: Parent` is a supertrait for
/// `<Self as Parent>::A`; `&T` (T: Sized) and `&Tail<[u8]>` do not overlap; a std trait written as a path
/// (`core::fmt::Debug`) is that trait; an impl's `-> impl Tr` whose arms all name one type is checked; an unsized
/// field read by value is E0277
#[test]
fn review_three_cases() {
    let check = |src: &str| Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check(src);
    for src in [
        "trait Parent { type A; }\ntrait Child where Self: Parent { fn get(&self) -> <Self as Parent>::A; }\nfn main() {}\n",
        "struct Tail<T: ?Sized>(T);\ntrait Flag {}\nimpl<T> Flag for &T {}\nimpl Flag for &Tail<[u8]> {}\nfn main() {}\n",
        "trait Local {}\nstruct S;\nimpl Local for S where u8: core::fmt::Debug {}\nfn need<X: Local>(_: X) {}\nfn main() { need(S); }\n",
        "trait Parent {}\ntrait Child: Parent {}\nstruct W<T>(T);\nimpl<T: Copy> Parent for W<T> {}\nimpl<T> Child for W<T> where W<T>: Parent {}\nfn main() {}\n",
        "fn main() { let a = vec![1.0]; let b = vec![1.0f32]; assert_eq!(a, b); }\n",
    ] {
        let d = check(src);
        assert!(d.is_empty(), "{src}\n{d:?}");
    }
    // (a predicate on another subject gives nothing about the self type: `T: Parent` is not `W<T>: Parent`)
    let d = check("trait Parent {}\ntrait Child: Parent {}\nstruct W<T>(T);\nimpl<T: Copy> Parent for W<T> {}\nimpl<T> Child for W<T> where T: Parent {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].msg.contains("Parent"), "{d:?}");
    let d = check("trait Local {}\nstruct S;\nfn f(b: bool) -> impl Local { if b { S } else { S } }\nfn main() { let _ = f(true); }\n");
    assert!(d.len() == 1 && d[0].msg.contains("S: Local"), "{d:?}");
    let d = check("struct S<T: ?Sized> { a: u8, tail: T }\nfn main() { let r: &S<[u8]> = &S { a: 1, tail: [1, 2] }; let _m = r.tail; let _ = r.a; }\n");
    assert!(d.len() == 1 && d[0].msg.contains("cannot be known at compilation time"), "{d:?}");
}

/// Fourth review of #128, each checked against rustc: `size_of_val` of a raw pointer and `drop_in_place` of a
/// `*const` are E0308; a tail `if` whose arms are `S` and a call returning `S` is checked as one `S`; a qualified
/// projection whose const argument zustc cannot evaluate takes the one impl that fits, and is not the first impl by
/// self type when several do
#[test]
fn review_four_cases() {
    let check = |src: &str| Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check(src);
    let d = check("fn main() { let a = [1u8, 2, 3]; let p: *const [u8] = &a; let _ = std::mem::size_of_val(p); }\n");
    assert!(d.len() == 1 && d[0].msg.contains("mismatched types"), "{d:?}");
    let d = check("struct Tail<T: ?Sized>(u8, T);\nfn main() { let b: Box<Tail<[u8]>> = Box::new(Tail(1, [1, 2])); let p: *const Tail<[u8]> = &*b; unsafe { std::ptr::drop_in_place(p); } }\n");
    assert!(d.len() == 1 && d[0].msg.contains("mismatched types"), "{d:?}");
    let d = check("trait Local {}\nstruct S;\nfn make_s() -> S { S }\nfn f(b: bool) -> impl Local { if b { S } else { make_s() } }\nfn main() { let _ = f(true); }\n");
    assert!(d.len() == 1 && d[0].msg.contains("S: Local"), "{d:?}");
    let one = "trait Pick<const N: usize> { type Out; }\nstruct S;\nimpl Pick<2> for S { type Out = u8; }\nfn main() { let _x: <S as Pick<{ match true { true => 2, false => 3 } }>>::Out = 0u8; }\n";
    let d = check(one);
    assert!(d.is_empty(), "{d:?}");
    let two = "trait Pick<const N: usize> { type Out; }\nstruct S;\nimpl Pick<3> for S { type Out = u64; }\nimpl Pick<2> for S { type Out = u8; }\nfn main() { let _x: <S as Pick<{ match true { true => 2, false => 3 } }>>::Out = 0; }\n";
    let d = check(two);
    assert!(d.len() == 1 && d[0].msg.contains("cannot be evaluated"), "{d:?}");
    // (an impl whose trait argument is not resolved (`{ <u8 as K>::N }`, 2) may be the one: not the other impl)
    let unres = "trait K { const N: usize; }\nimpl K for u8 { const N: usize = 2; }\ntrait Pick<const N: usize> { type Out; }\nstruct S;\nimpl Pick<3> for S { type Out = u64; }\nimpl Pick<{ <u8 as K>::N }> for S { type Out = u8; }\nfn main() { let _x: <S as Pick<{ match true { true => 2, false => 3 } }>>::Out = 0; }\n";
    let d = check(unres);
    assert!(d.len() == 1 && d[0].msg.contains("cannot be evaluated"), "{d:?}");
}

/// Projections as normalizes-to goals (issue #2 step 3), each checked against rustc 1.97:
/// - a bound's bindings (`T: Tr<A = u32>`): `<t as Tr>::A`, by Tr's impls, must be the bound type (E0271);
/// - where clauses on a projection (`where T::A: Bar`, `where <T as Tr>::A: Bar`) or another type (`where W<T>:
///   Bar`) are goals of the instance;
/// - an impl's associated type meets the bounds its trait declares for it (`type A: Bar;`).
#[test]
fn projections_are_goals() {
    let check = |src: &str| Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check(src);
    let bad = |src: &str, want: &str| {
        let d = check(src);
        assert!(d.len() == 1 && d[0].msg.contains(want), "{src}\n{d:?}");
    };
    let ok = |src: &str| {
        let d = check(src);
        assert!(d.is_empty(), "{src}\n{d:?}");
    };
    let tr = "trait Tr { type A; }\nimpl Tr for u8 { type A = u16; }\nimpl Tr for i8 { type A = i16; }\n";
    // (bindings)
    bad(&format!("{tr}fn f<X: Tr<A = u32>>(_: X) {{}}\nfn main() {{ f(1u8); }}"), "type mismatch resolving `<u8 as Tr>::A == u32`");
    ok(&format!("{tr}fn f<X: Tr<A = u16>>(_: X) {{}}\nfn main() {{ f(1u8); }}"));
    ok(&format!("{tr}fn f<X>(_: X) where X: Tr<A = u16> {{}}\nfn main() {{ f(1u8); }}"));
    // (the binding names another parameter, given by the call)
    ok(&format!("{tr}fn f<X: Tr<A = Y>, Y>(_: X, _: Y) {{}}\nfn main() {{ f(1u8, 2u16); }}"));
    bad(&format!("{tr}fn f<X: Tr<A = Y>, Y>(_: X, _: Y) {{}}\nfn main() {{ f(1u8, 2u32); }}"), "E0271");
    // (an impl for a generic struct; two impls of one struct)
    bad("trait Tr { type A; }\nstruct W<T>(T);\nimpl<T> Tr for W<T> { type A = T; }\nfn f<X: Tr<A = u8>>(_: X) {}\nfn main() { f(W(1u16)); }", "E0271");
    bad("trait Tr { type A; }\nstruct W<T>(T);\nimpl Tr for W<i32> { type A = u32; }\nimpl Tr for W<u32> { type A = i32; }\nfn f<T: Tr<A = ()>>() {}\nfn main() { f::<W<i32>>(); }", "E0271");
    // (two traits with an associated type of one name: each by its own trait)
    ok("trait P { type A; }\ntrait Q { type A; }\nimpl P for u8 { type A = u16; }\nimpl Q for u8 { type A = u32; }\nfn f<X: P<A = u16> + Q<A = u32>>(_: X) {}\nfn main() { f(1u8); }");
    // (a generic trait's impls that disagree: not known, not an error)
    ok("trait Tr<T> { type A; }\nimpl Tr<u8> for i32 { type A = u8; }\nimpl Tr<u16> for i32 { type A = u16; }\nfn f<X: Tr<u16, A = u16>>(_: X) {}\nfn main() { f(1i32); }");
    // (std's iterators: a reference item is not the item; the prelude's own bindings (`sum`) are hints, not checked)
    bad("fn f<I: Iterator<Item = u8>>(_: I) {}\nfn main() { let v = vec![1u8]; f(v.iter()); }", "E0271");
    ok("fn f<I: Iterator<Item = u8>>(_: I) {}\nfn main() { let v = vec![1u8]; f(v.iter().copied()); f(v.into_iter()); }");
    ok("fn main() { let v = vec![1i32, 2]; let a: i32 = v.iter().sum(); let b = v.iter().product::<i32>(); println!(\"{a} {b}\"); }");
    // (where clauses on a projection, and on another type)
    let wb = format!("{tr}trait Bar {{}}\nimpl Bar for i16 {{}}\n");
    bad(&format!("{wb}fn f<X: Tr>(_: X) where X::A: Bar {{}}\nfn main() {{ f(1u8); }}"), "the trait bound `u16: Bar` is not satisfied");
    ok(&format!("{wb}fn f<X: Tr>(_: X) where X::A: Bar {{}}\nfn main() {{ f(1i8); }}"));
    bad(&format!("{wb}fn f<X: Tr>(_: X) where <X as Tr>::A: Bar {{}}\nfn main() {{ f(1u8); }}"), "`u16: Bar`");
    ok(&format!("{wb}fn f<X: Tr>(_: X) where <X as Tr>::A: Bar {{}}\nfn main() {{ f(1i8); }}"));
    bad("trait Bar {}\nimpl Bar for u8 {}\nfn f<I: Iterator>(_: I) where I::Item: Bar {}\nfn main() { f(vec![1u16].into_iter()); }", "`u16: Bar`");
    ok("trait Bar {}\nimpl<'a> Bar for &'a u8 {}\nfn f<I: Iterator>(_: I) where I::Item: Bar {}\nfn main() { let v = vec![1u8]; f(v.iter()); }");
    bad("trait Bar {}\nstruct W<T>(T);\nimpl Bar for W<i32> {}\nfn f<T>(_: T) where W<T>: Bar {}\nfn main() { f(1u8); }", "`W<u8>: Bar`");
    ok("trait Bar {}\nstruct W<T>(T);\nimpl Bar for W<i32> {}\nfn f<T>(_: T) where W<T>: Bar {}\nfn main() { f(1i32); }");
    // (a Box, an array, a slice, a type with a trait object: implements does not see the program's impls for them,
    // so a where clause on one is not checked)
    for (imp, pred) in [("impl Mark for [u8; 1] {}", "[T; 1]"), ("impl<T> Mark for [T; 1] {}", "[T; 1]"), ("impl Mark for Box<u8> {}", "Box<T>"), ("impl Mark for [u8] {}", "[T]")] {
        ok(&format!("trait Mark {{}}\n{imp}\nfn f<T>(_: T) where {pred}: Mark {{}}\nfn main() {{ f(1u8); }}"));
    }
    ok("trait Mark {}\ntrait D {}\nimpl Mark for Box<dyn D> {}\nfn f<T: ?Sized>(_: &T) where Box<dyn D>: Mark {}\nfn main() { f(&1u8); }");
    // (an associated type a supertrait declares: `T::A` of `T: Sub`)
    let sup = "trait Mark {}\ntrait Super { type A; }\ntrait Sub: Super {}\nimpl Super for u8 { type A = u16; }\nimpl Sub for u8 {}\nfn f<T: Sub>(_: T) where T::A: Mark {}\nfn main() { f(1u8); }";
    bad(sup, "`u16: Mark`");
    ok(&format!("impl Mark for u16 {{}}\n{sup}"));
    // (a qualified projection of a concrete type is normalized by its own trait, not by the name of the associated
    // type: P's A is u16, Q's is u32)
    let pq = "trait P { type A; }\ntrait Q { type A; }\ntrait Bar {}\nimpl P for u8 { type A = u16; }\nimpl Q for u8 { type A = u32; }\nfn f<T>(_: T) where <u8 as Q>::A: Bar {}\nfn main() { f(1i8); }";
    ok(&format!("impl Bar for u32 {{}}\n{pq}"));
    bad(&format!("impl Bar for u16 {{}}\n{pq}"), "`u32: Bar`");
    // (a binding of a where clause on another type; a qualified projection is its trait's own associated type,
    // before a supertrait's of the same name)
    bad("trait Tr { type A; }\nstruct W<T>(T);\nimpl Tr for W<u8> { type A = u16; }\nfn f<T>() where W<T>: Tr<A = u32> {}\nfn main() { f::<u8>(); }", "E0271");
    ok("trait Tr { type A; }\nstruct W<T>(T);\nimpl Tr for W<u8> { type A = u16; }\nfn f<T>() where W<T>: Tr<A = u16> {}\nfn main() { f::<u8>(); }");
    let sub = "trait Mark {}\ntrait Super { type A; }\ntrait Sub: Super { type A; }\nimpl Super for u8 { type A = u8; }\nimpl Sub for u8 { type A = u16; }\nfn f<T: Sub>() where <T as Sub>::A: Mark {}\nfn main() { f::<u8>(); }";
    bad(&format!("impl Mark for u8 {{}}\n{sub}"), "`u16: Mark`");
    ok(&format!("impl Mark for u16 {{}}\n{sub}"));
    // (an impl's value with a projection in it, `type A = T::A;` with `T: Q`, is not known: looked up by the name A
    // alone, P's u16 would be taken for Q's u32)
    let iv = "trait P { type A; }\ntrait Q { type A; }\nimpl P for u8 { type A = u16; }\nimpl Q for u8 { type A = u32; }\nstruct W<T>(T);\ntrait Tr { type A; }\nimpl<T: Q> Tr for W<T> { type A = T::A; }\n";
    ok(&format!("{iv}fn f<T>() where W<T>: Tr<A = u32> {{}}\nfn main() {{ f::<u8>(); }}"));
    ok(&format!("{iv}fn f<X: Tr<A = u32>>() {{}}\nfn main() {{ f::<W<u8>>(); }}"));
    // (an uncalled generic fn's own where clause is what its body may assume, not an obligation: checking the body
    // with placeholders does not prove it)
    ok("trait Get { fn get() -> Self; }\nfn g<'a, G: 'a>() where &'a G: Get {}\nfn main() {}");
    // (a where clause on a concrete type)
    bad("trait Bar {}\nstruct S;\nfn f<T>(_: T) where S: Bar {}\nfn main() { f(1u8); }", "`S: Bar`");
    ok("trait Bar {}\nstruct S;\nimpl Bar for S {}\nfn f<T>(_: T) where S: Bar {}\nfn main() { f(1u8); }");
    // (an impl's associated type against its trait's bounds for it)
    bad("trait Bar {}\ntrait Tr { type A: Bar; }\nimpl Tr for u8 { type A = u16; }\nfn main() {}", "`u16: Bar`");
    ok("trait Bar {}\nimpl Bar for u16 {}\ntrait Tr { type A: Bar; }\nimpl Tr for u8 { type A = u16; }\nfn main() {}");
}

/// Sized (E0277) is not asked where rustc does not ask it: a `?Sized` parameter given `str` or a struct with an unsized
/// tail, `Box<X>` of a `X: ?Sized`, `T: Clone + ?Sized` in a slice (Clone implies Sized), an unsized last tuple
/// element behind a reference, `size_of_val` of a str (checked against rustc)
#[test]
fn sized_accepts() {
    let d = Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check("trait Tr { fn n(&self) -> u8 { 1 } }\nimpl Tr for u8 {}\nstruct W<T: ?Sized>(u8, T);\nfn relaxed<X: ?Sized>(_: &X) {}\nfn boxed<X: ?Sized + Tr>(x: Box<X>) -> u8 { x.n() }\nfn cloned<T: Clone + ?Sized>(x: &[T]) -> usize { x.to_vec().len() }\nfn main() {\n    relaxed(\"abc\");\n    let w: &W<[u8]> = &W(1, [2u8, 3]);\n    relaxed(w);\n    let t: Option<&(u8, [u8])> = None;\n    let b: Box<dyn Tr> = Box::new(3u8);\n    println!(\"{} {} {} {}\", t.is_none(), boxed(b), std::mem::size_of_val(\"abcd\"), w.0 + cloned(&[1u8]) as u8);\n}\n");
    assert!(d.is_empty(), "{d:?}");
}

/// A `?Sized` parameter passed on to one that must be sized, in a generic fn nothing calls (`zustc check` checks it
/// with placeholders): E0277, as rustc
#[test]
fn sized_rejects_maybe_unsized_param() {
    let check = |src: &str| Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check(src);
    let d = check("fn sized<X>(_: &X) {}\nfn relaxed<X: ?Sized>(x: &X) {\n    sized::<X>(x);\n}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].msg.contains("`X` cannot be known"), "{d:?}");
    let d = check("struct Foo<T>(T);\nfn not_sized<T: ?Sized>() {}\nfn f<T: ?Sized>() { not_sized::<Foo<T>>() }\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].msg.contains("`T` cannot be known"), "{d:?}");
}

#[test]
fn async_main_and_test_attributes_where_rustc_rejects_them() {
    // (rustc 1.100: E0752 at the fn; the test macro's error at the `#[test]` on what is no free fn, `--test` or not)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    assert_eq!(first("async fn main() {}\n"), Some((1, 1, "`main` function is not allowed to be `async` (E0752)".to_string())), "async main");
    assert_eq!(first("#![crate_type = \"lib\"]\npub async fn main() {}\n"), None, "a library's async fn named main");
    let t = |l: usize, c: usize| Some((l, c, "the `test` attribute may only be used on a free function".to_string()));
    assert_eq!(first("struct S;\nimpl S {\n    #[test]\n    fn new() {}\n}\nfn main() {}\n"), t(3, 5), "a method");
    assert_eq!(first("#[test]\nstruct A;\nfn main() {}\n"), t(1, 1), "a struct");
    assert_eq!(first("#[derive(Clone)]\n#[test]\nconst X: u8 = 1;\nfn main() {}\n"), t(2, 1), "a const, after another attribute");
    assert_eq!(first("fn main() {\n    #[test]\n    struct A5(u32, u8);\n}\n").map(|d| (d.0, d.2)), t(2, 5).map(|d| (d.0, d.2)), "an item in a fn body, at its line");
    assert_eq!(first("#[test]\nfn t() {}\n#[test]\npub(crate) unsafe extern \"C\" fn u() {}\nmacro_rules! m { () => {} }\nfn main() {}\n"), None, "free fns");
}

#[test]
fn macro_definitions_rustc_rejects_and_rules_with_doc_comments() {
    // (rustc 1.100 reads each `macro_rules!` definition, used or not: no rule, a matcher outside delimiters, a rule cut
    // short; a rule's doc comment is a warning, and the rule after it is read)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(first("fn main() {}\n\nmacro_rules! test {}\n"), e(3, "macros must contain at least one rule"), "no rule");
    assert_eq!(first("macro_rules! foo()\nfn main() {}\n"), e(1, "macros that expand to items must be delimited with braces or followed by a semicolon"), "parentheses, no `;`");
    assert_eq!(first("macro_rules! b {\n    0 => {}\n}\nfn main() {}\n"), e(2, "invalid macro matcher; matchers must be contained in balanced delimiters"), "a bare matcher");
    assert_eq!(first("macro_rules! a { {} => }\nfn main() {}\n"), e(1, "macro definition ended unexpectedly"), "no body");
    assert_eq!(first("macro_rules! e {\n    {}\n}\nfn main() {}\n"), e(2, "expected `=>`, found end of macro arguments"), "no `=>`");
    assert_eq!(first("macro_rules! m {\n    /// the empty one\n    () => { 1 };\n    /// a value\n    ($x:expr) => { $x };\n}\nfn main() { let _ = m!() + m!(2); }\n"), None, "doc comments on rules");
}

#[test]
fn items_of_one_name_in_two_blocks_of_a_fn() {
    // (each block's item is its own: moved out of the fn, they are named apart)
    let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
    let src = "fn main() {\n    {\n        struct Bar { x: u8 }\n        let _ = Bar { x: 1 };\n    }\n    {\n        struct Bar { y: u16 }\n        let b = Bar { y: 2 };\n        let _ = b.y;\n    }\n    if true {\n        enum E { A }\n        let _ = E::A;\n    } else {\n        enum E { B }\n        let _ = E::B;\n    }\n}\n";
    assert_eq!(s.check(src).first().map(|d| (d.line, d.msg.clone())), None);
    // (an `unsafe fn` in a body is moved out too: it was left there, "nested items are not supported")
    let src = "fn main() {\n    unsafe fn g(x: u8) -> u8 {\n        x + 1\n    }\n    let _ = unsafe { g(1) };\n}\n";
    assert_eq!(s.check(src).first().map(|d| (d.line, d.msg.clone())), None, "an unsafe fn in a body");
    // (and one with other qualifiers: `async fn`, `const fn`, `extern "C" fn`)
    let src = "fn main() {\n    async fn a() {}\n    const fn c(x: u8) -> u8 {\n        x + 1\n    }\n    extern \"C\" fn e(x: u8) -> u8 {\n        x\n    }\n    const K: u8 = c(1);\n    let _ = (a(), K, e(2));\n}\n";
    assert_eq!(s.check(src).first().map(|d| (d.line, d.msg.clone())), None, "fns with qualifiers in a body");
}

#[test]
fn literals_and_comments_a_file_ends_in() {
    // (rustc 1.100's lexer errors at the literal's or the comment's start, before the delimiters it leaves open)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, m: &str| Some((l, c, m.to_string()));
    assert_eq!(first("fn main() {\n    let s = \"abc;\n}\n"), e(2, 13, "unterminated double quote string (E0765)"), "a string");
    assert_eq!(first("fn main() {\n    let s = r##\"abc\"#;\n}\n"), e(2, 13, "unterminated raw string (E0748)"), "a raw string");
    assert_eq!(first("fn main() {\n    let s = b\"abc;\n}\n"), e(2, 13, "unterminated double quote byte string (E0766)"), "a byte string");
    assert_eq!(first("fn main() {\n    /* a /* b */\n}\n"), e(2, 5, "unterminated block comment (E0758)"), "a block comment");
    assert_eq!(first("fn main() {\n    let c = '\"'; let l: &'static str = \"}\"; let r = r#\"\"\"#;\n").map(|d| d.2), Some("this file contains an unclosed delimiter".to_string()), "closed literals: the delimiter's error");
}

#[test]
fn a_struct_field_without_its_comma() {
    // (rustc 1.100: right after the field, where the comma goes)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    assert_eq!(first("pub struct S {\n    pub foo: u32\n    pub bar: u32\n}\nfn main() {}\n"), Some((2, 17, "expected `,`, or `}`, found keyword `pub`".to_string())));
    assert_eq!(first("struct F {\n    owo: bool\n    #[allow(unused)]\n    uwu: bool,\n}\nfn main() {}\n"), Some((2, 14, "expected `,`, or `}`, found `#`".to_string())));
    assert_eq!(first("struct G { a: u8 b: u8 }\nfn main() {}\n"), Some((1, 17, "expected `,`, or `}`, found `b`".to_string())));
    assert_eq!(first("struct H { a: u8, b: Vec<u8> }\nfn main() {}\n"), None);
}

#[test]
fn const_and_static_items_without_a_type() {
    // (rustc 1.100: right after the name, or after its colon)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let e = |l: usize, c: usize, kw: &str| Some((l, c, format!("missing type for `{kw}` item")));
    assert_eq!(first("const A = 1;\nfn main() {}\n"), e(1, 8, "const"), "no colon");
    assert_eq!(first("const _A: = 123;\nfn main() {}\n"), e(1, 10, "const"), "a colon");
    assert_eq!(first("static B = 2;\nfn main() {}\n"), e(1, 9, "static"), "a static");
    assert_eq!(first("fn main() {\n    const FOO = \"hello\";\n    let _ = FOO;\n}\n").map(|d| (d.0, d.2)), e(2, 0, "const").map(|d| (d.0, d.2)), "in a fn body, at its line");
    assert_eq!(first("const A: u8 = 1;\nstatic B: u8 = 2;\nfn main() {}\n"), None, "with types");
}

#[test]
fn attributes_with_no_item_after_them_in_an_impl() {
    // (rustc 1.100: "expected item after attributes", at the first)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    assert_eq!(first("struct Foo;\nimpl Foo {\n    fn foo() {}\n    #[inline]\n    #[allow(dead_code)]\n}\nfn main() {}\n"), Some((4, 5, "expected item after attributes".to_string())));
    assert_eq!(first("struct Foo;\nimpl Foo {\n    #[inline]\n    fn foo() {}\n}\nfn main() { Foo::foo(); }\n"), None);
}

#[test]
fn abis_no_one_has_and_raw_pointers_without_their_keyword() {
    // (rustc 1.100: E0703 at the ABI's string; a raw pointer type's `const` or `mut`, at the `*`)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    assert_eq!(first("pub extern \"fictional\" fn lol() {}\nfn main() {}\n"), Some((1, 12, "invalid ABI: found `fictional` (E0703)".to_string())));
    assert_eq!(first("extern \"\\x43\" fn foo() {}\nextern \"C-unwind\" { fn bar(); }\nfn main() {}\n"), None, "an escaped C, C-unwind");
    let p = |l: usize, c: usize| Some((l, c, "expected `mut` or `const` keyword in raw pointer type".to_string()));
    assert_eq!(first("fn foo(_: *()) {}\nfn main() {}\n"), p(1, 11), "a bare pointer");
    assert_eq!(first("fn main() {\n    let x = 5u8;\n    let p: *const u8 = &x;\n    let _d: **const u8 = &p;\n}\n"), p(4, 13), "a pointer to a pointer");
}

#[test]
fn extern_statics_in_a_static_value() {
    // (rustc 1.100's E0080 for the value, at its line; a fn reads one in `unsafe`)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    assert_eq!(first("extern \"C\" {\n    pub static symbol: u32;\n}\nstatic CRASH: u32 = symbol;\nfn main() {}\n"), Some((4, "cannot access extern static `symbol` (E0080)".to_string())));
    assert_eq!(first("extern \"C\" {\n    static symbol: u32;\n}\nstatic OK: u32 = 1;\nfn main() {\n    let _ = unsafe { symbol };\n}\n"), None);
    // (its address is no read; a read through it is, `*&X`)
    assert_eq!(first("extern \"C\" {\n    static Q: [u8; 0];\n}\nstatic R: &'static [u8; 0] = unsafe { &Q };\nfn main() {}\n"), None, "its address");
    assert_eq!(first("extern \"C\" {\n    static Q: u32;\n}\npub static B: u32 = *&Q;\nfn main() {}\n").map(|d| d.0), Some(4), "a read through its address");
}

#[test]
fn macro_parameters_no_matcher_binds() {
    // (rustc 1.100, where the rule is used: at the `$name` in its body; an inner `macro_rules!`'s own `$x` is fine)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    assert_eq!(first("macro_rules! m {\n    (begin $ard:ident end) => {\n        [$arg]\n    };\n}\nfn main() {\n    let x = 1;\n    let _ = m![begin x end];\n}\n"), Some((3, "cannot find macro parameter `$arg` in this scope".to_string())), "a typo");
    assert_eq!(first("macro_rules! m {\n    (begin $($ard:ident),* end) => {\n        [$($arg),*]\n    };\n}\nfn main() {\n    let x = 1;\n    let _ = m![begin x end];\n}\n"), Some((3, "attempted to repeat an expression containing no syntax variables matched as repeating at this depth".to_string())), "a repetition");
    assert_eq!(first("macro_rules! outer {\n    ($n:ident) => {\n        macro_rules! $n {\n            ($x:expr) => { $x + 1 };\n        }\n    };\n}\nouter!(inner);\nfn main() {\n    let _ = inner!(2);\n}\n"), None, "an inner macro's own parameter");
}

#[test]
fn a_for_loop_over_a_struct_with_its_own_next() {
    // (rustc 1.100: E0277 at the expression; an Iterator impl's `next` is fine)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let s = "struct M { x: isize }\nimpl M {\n    fn next(&mut self) -> Option<isize> { Some(self.x) }\n}\nfn main() {\n    let bogus = M { x: 1 };\n    for x in bogus {\n        let _ = x;\n    }\n}\n";
    assert_eq!(first(s), Some((7, 14, "`M` is not an iterator (E0277)".to_string())));
    let ok = "struct C(u8);\nimpl Iterator for C {\n    type Item = u8;\n    fn next(&mut self) -> Option<u8> { None }\n}\nfn main() {\n    for x in C(3) { let _ = x; }\n}\n";
    assert_eq!(first(ok), None);
}

#[test]
fn renamed_groups_and_derives_by_std_paths() {
    // (rustc 1.100: a group or a glob has no name to rename; a derive of a name std's root does not have. `extern "C"
    // mod foo;` is srcmap's, missing_mod_tests)
    let first = |src: &str| {
        let mut s = Session2::new(Options2 { jobs: 1, edition: 2021, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.col, d.msg.clone()))
    };
    let m = "mod foo {\n    pub fn bar() {}\n}\nfn main() {}\n";
    assert_eq!(first(&format!("use foo::{{bar}} as baz;\n{m}")), Some((1, 16, "expected `;`, found keyword `as`".to_string())));
    assert_eq!(first(&format!("use foo::* as baz;\n{m}")), Some((1, 12, "expected `;`, found keyword `as`".to_string())));
    assert_eq!(first(&format!("use foo::{{bar as baz}};\n{m}")).map(|e| e.2).filter(|e| e.contains("found keyword")), None);
    let e = first("#[derive(\n    core::RustcDecodable,\n)]\nstruct S;\nfn main() {}\n");
    assert_eq!(e.map(|e| (e.0, e.2)), Some((2, "cannot find `RustcDecodable` in `core`".to_string())));
    assert_eq!(first("#[derive(core::clone::Clone, ::std::marker::Copy)]\nstruct S;\nfn main() {}\n"), None);
}

#[test]
fn macro_matchers_rustc_rejects_what_follows_a_fragment_in_and_names_bound_twice() {
    // (rustc 1.100's check_matcher: `expr` is followed by `=>` `,` `;` only, `ty` / `path` by a few more, `pat` by
    // `|` before edition 2021 only; a name bound twice in one matcher)
    let first = |src: &str, edition: u16| {
        let mut s = Session2::new(Options2 { jobs: 1, edition, ..Options2::default() });
        s.check(src).first().map(|d| (d.line, d.msg.clone()))
    };
    let e = |l: usize, m: &str| Some((l, m.to_string()));
    assert_eq!(first("macro_rules! m {\n    ($e:expr +) => ()\n}\nfn main() {}\n", 2021), e(2, "`$e:expr` is followed by `+`, which is not allowed for `expr` fragments"));
    assert_eq!(first("macro_rules! m {\n    ( $a:expr $($b:tt)* ) => { };\n}\nfn main() {}\n", 2021), e(2, "`$a:expr` is followed by `$b:tt`, which is not allowed for `expr` fragments"));
    assert_eq!(first("macro_rules! m {\n    ($($ty:ty)-+) => ();\n}\nfn main() {}\n", 2021), e(2, "`$ty:ty` is followed by `-`, which is not allowed for `ty` fragments"));
    let or = "macro_rules! m { ($x:pat | $y:pat) => {} }\nfn main() {}\n";
    assert_eq!(first(or, 2021), e(1, "`$x:pat` is followed by `|`, which is not allowed for `pat` fragments"));
    assert_eq!(first(or, 2018), None);
    let ok = "macro_rules! m {\n    ($e:expr => $t:ty, $p:path { $b:block }) => {};\n    ($($e:expr),* ; $($t:ty)|*) => {};\n    ($($x:pat_param)|+ if $c:expr) => {};\n}\nfn main() {}\n";
    assert_eq!(first(ok, 2021), None);
    assert_eq!(first("macro_rules! m {\n    ($a:ident, $($a:ident),*) => {};\n}\nfn main() {}\n", 2021), e(2, "duplicate matcher binding"));
    assert_eq!(first("macro_rules! m {\n    ($a:ident) => {};\n    ($a:path) => {};\n}\nfn main() {}\n", 2021), None);
}

/// Review of #180, each checked against rustc: a struct whose tail is a `?Sized` parameter is unsized by value in a
/// signature (E0277); a program trait whose supertrait is Clone makes a `?Sized` parameter sized; a closure does not
/// make its parent's `?Sized` parameter sized
#[test]
fn sized_review_cases() {
    let check = |src: &str| Session2::new(Options2 { jobs: 1, no_cache: true, ..Options2::default() }).check(src);
    let d = check("struct W<T: ?Sized>(T);\nfn bad<T: ?Sized>(_: W<T>) {}\nfn main() {}\n");
    assert!(d.len() == 1 && d[0].msg.contains("`T` cannot be known"), "{d:?}");
    for src in [
        "struct W<T: ?Sized>(T);\nfn good<T: ?Sized>(_: &W<T>) {}\nfn main() {}\n",
        "trait Dup: Clone {}\nfn f<T: Dup + ?Sized>(_: &T) { let _v: Vec<T> = Vec::new(); }\nfn main() {}\n",
        "use std::fmt::Display;\nfn f<T: ?Sized + Display>(x: &T) -> String { let c = |y: &T| format!(\"{y}\"); c(x) }\nfn main() { println!(\"{}\", f(\"abc\")); }\n",
    ] {
        let d = check(src);
        assert!(d.is_empty(), "{src}\n{d:?}");
    }
}
