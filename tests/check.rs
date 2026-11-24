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
