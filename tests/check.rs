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
    assert!(d[0].msg.contains("cannot find type `T`"), "{d:?}");
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
    assert_eq!(first(src).map(|d| (d.0, d.2)), Some((3, "trait takes 1 generic argument but 2 generic arguments were supplied (E0107)".to_string())), "in_fn_body");
    let src = "trait Tr<'a, T, U = u8> {}\nimpl<'a> Tr<'a, i32> for u8 {}\nimpl<'a> Tr<'a, i32, u16> for u16 {}\nfn main() {}\n";
    assert_eq!(first(src), None, "lifetime_and_default_ok");
    let src = "trait It { type Item; }\nimpl It for u8 { type Item = u8; }\nfn main() {}\n";
    assert_eq!(first(src), None, "assoc_type_ok");
}
