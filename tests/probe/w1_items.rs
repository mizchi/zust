// What wasm-bindgen 0.2.130 needs of a crate's items (W1 of #38): inherent methods of one name on disjoint
// instances, glob re-exports through module chains, nested `unsafe extern "C" fn`, the program's `From` / `TryFrom`
// impls for numbers, impls told apart by their parameters' bounds, operators' impls for references with their own
// `Output`s, and `<T as PartialEq<R>>::eq`.

// (inherent methods of one name in impls for disjoint instances: the receiver picks)
mod disjoint {
    pub struct W<T>(pub T);
    impl W<u8> {
        pub fn get(&self) -> u32 {
            self.0 as u32 + 1
        }
    }
    impl W<u16> {
        pub fn get(&self) -> u32 {
            self.0 as u32 + 2
        }
    }
    #[derive(Debug)]
    pub struct N<T>(pub Option<T>);
    impl<T> N<T> {
        pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> N<U> {
            N(self.0.map(f))
        }
    }
    impl<T: Copy> N<&T> {
        pub fn copied(self) -> N<T> {
            self.map(|&v| v)
        }
    }
    impl<T: Copy> N<&mut T> {
        pub fn copied(self) -> N<T> {
            self.map(|&mut v| v)
        }
    }
    pub fn run() {
        let a = W(1u8);
        let b = W(1u16);
        let x = 5u8;
        let mut y = 7u32;
        println!("{} {} {:?} {:?}", a.get(), b.get(), N(Some(&x)).copied().0, N(Some(&mut y)).copied().0);
    }
}

// (a module whose name starts with underscores, and const generics in a module)
pub mod __rt {
    pub fn one() -> u32 {
        1
    }
    pub const fn flat_byte_slices<const RESULT_LEN: usize, const SIZE: usize>(slices: [&[u8]; SIZE]) -> [u8; RESULT_LEN] {
        let mut result = [0; RESULT_LEN];
        let mut slice_index = 0;
        let mut result_offset = 0;
        while slice_index < slices.len() {
            let mut i = 0;
            let slice = slices[slice_index];
            while i < slice.len() {
                result[result_offset] = slice[i];
                i += 1;
                result_offset += 1;
            }
            slice_index += 1;
        }
        result
    }
}

// (names a glob re-export brings in, through chains of them; a name defined in two modules is renamed in both)
mod convert {
    mod traits {
        pub struct WasmRet<T> {
            pub v: T,
        }
        pub fn mk(v: u32) -> WasmRet<u32> {
            WasmRet { v }
        }
        pub trait IntoAbi {
            fn into_abi(self) -> u32;
        }
    }
    pub use self::traits::*;
}
mod other {
    pub struct WasmRet {
        pub w: u8,
    }
    pub fn mk(w: u8) -> WasmRet {
        WasmRet { w }
    }
    pub trait IntoAbi {
        fn into_abi(self) -> u64;
    }
    impl IntoAbi for u8 {
        fn into_abi(self) -> u64 {
            self as u64 * 100
        }
    }
}
pub use convert::*;
pub struct JsValue(pub u32);
mod rt {
    use crate::convert::WasmRet;
    pub fn top() -> WasmRet<u32> {
        WasmRet { v: 5 }
    }
    pub fn via_path() -> crate::convert::WasmRet<u32> {
        crate::convert::WasmRet { v: 6 }
    }
    pub fn via_root() -> crate::WasmRet<u32> {
        crate::mk(7)
    }
    pub fn nested() -> u32 {
        fn inner(x: u32) -> WasmRet<u32> {
            WasmRet { v: x }
        }
        inner(8).v
    }
    // (a nested `unsafe extern "C" fn` moves out with its module's names)
    pub fn shim_call() -> u32 {
        #[inline(never)]
        unsafe extern "C" fn shim<T: Copy>(x: T) -> WasmRet<T> {
            WasmRet { v: x }
        }
        unsafe { shim::<u32>(9).v }
    }
}
mod closure {
    use crate::{convert::*, JsValue};
    pub struct Owned(pub u32);
    impl IntoAbi for Owned {
        fn into_abi(self) -> u32 {
            self.0 + 1
        }
    }
    pub fn mk(j: JsValue) -> WasmRet<u32> {
        WasmRet { v: Owned(j.0).into_abi() }
    }
}

// (the program's `From` / `TryFrom` impls for numbers, and two impls of one trait for one type with their own
// associated types)
mod conv {
    use std::convert::TryFrom;
    pub struct S(pub u8);
    pub struct J(pub i64);
    impl From<J> for f64 {
        fn from(v: J) -> Self {
            v.0 as f64
        }
    }
    impl From<&J> for f64 {
        fn from(v: &J) -> Self {
            v.0 as f64 + 0.5
        }
    }
    impl TryFrom<i32> for S {
        type Error = String;
        fn try_from(v: i32) -> Result<Self, Self::Error> {
            if (0..256).contains(&v) { Ok(S(v as u8)) } else { Err(format!("bad {}", v)) }
        }
    }
    impl TryFrom<J> for u32 {
        type Error = J;
        fn try_from(v: J) -> Result<Self, Self::Error> {
            if v.0 >= 0 { Ok(v.0 as u32) } else { Err(v) }
        }
    }
    impl TryFrom<&J> for u32 {
        type Error = i64;
        fn try_from(v: &J) -> Result<Self, Self::Error> {
            if v.0 >= 0 { Ok(v.0 as u32 + 1) } else { Err(v.0) }
        }
    }
    pub fn run() {
        let f: f64 = J(4).into();
        println!("{} {} {} {}", f64::from(J(3)), f64::from(&J(3)), f, f64::from(3u8));
        let ok = S::try_from(5).map(|s| s.0);
        let bad = S::try_from(500).map(|s| s.0);
        let r: Result<S, String> = 7.try_into();
        println!("{:?} {:?} {}", ok, bad, r.map(|s| s.0).unwrap_or(0));
        println!("{:?} {:?} {:?}", u32::try_from(J(3)).ok(), u32::try_from(&J(3)).ok(), u32::try_from(&J(-3)).err());
        println!("{:?}", u8::try_from(300i32).is_err());
    }
}

// (impls told apart by a parameter's bound: `(): Into<V>` does not hold, so `Result<T, E>`'s impl is not
// `Result<(), E>`'s)
mod bounds {
    #[derive(Debug)]
    pub struct V(pub i32);
    impl From<i32> for V {
        fn from(v: i32) -> Self {
            V(v)
        }
    }
    impl From<bool> for V {
        fn from(v: bool) -> Self {
            V(v as i32 * 100)
        }
    }
    pub trait IntoJsResult {
        fn into_js_result(self) -> Result<V, V>;
    }
    impl<T: Into<V>, E: Into<V>> IntoJsResult for Result<T, E> {
        fn into_js_result(self) -> Result<V, V> {
            match self {
                Ok(e) => Ok(e.into()),
                Err(e) => Err(e.into()),
            }
        }
    }
    impl<E: Into<V>> IntoJsResult for Result<(), E> {
        fn into_js_result(self) -> Result<V, V> {
            match self {
                Ok(()) => Ok(V(-1)),
                Err(e) => Err(e.into()),
            }
        }
    }
    pub trait Marker {}
    impl Marker for u8 {}
    pub trait Tag {
        fn tag(&self) -> u32;
    }
    impl<T: Marker> Tag for Option<T> {
        fn tag(&self) -> u32 {
            1
        }
    }
    impl Tag for Option<u16> {
        fn tag(&self) -> u32 {
            2
        }
    }
    // (an impl for `&T` beside one for `Box<[T]>`: a Box is no reference. The `&T` one forwards to T's, the only
    // kind zustc compiles (main's #144 check))
    pub trait Erasable {
        fn erased(&self) -> usize;
    }
    impl Erasable for u8 {
        fn erased(&self) -> usize {
            1
        }
    }
    impl<T: Erasable> Erasable for &T {
        fn erased(&self) -> usize {
            (**self).erased()
        }
    }
    impl<T: Erasable> Erasable for Box<[T]> {
        fn erased(&self) -> usize {
            self.len() * 100
        }
    }
    pub fn run() {
        let a: Result<i32, bool> = Ok(5);
        let b: Result<(), bool> = Ok(());
        let c: Result<(), i32> = Err(7);
        println!("{:?} {:?} {:?}", a.into_js_result(), b.into_js_result(), c.into_js_result());
        let bx: Box<[u8]> = vec![1u8, 2].into_boxed_slice();
        println!("{} {} {} {}", Some(1u8).tag(), Some(2u16).tag(), 3u8.erased(), bx.len());
    }
}

// (`<J as PartialEq<str>>::eq(a, b)`: the impl for that right-hand side, through the deref coercion of `&&str`)
mod peq {
    #[derive(Debug)]
    pub struct J(pub String);
    impl J {
        pub fn from_str(s: &str) -> J {
            J(s.to_string())
        }
    }
    impl PartialEq for J {
        fn eq(&self, o: &J) -> bool {
            self.0 == o.0
        }
    }
    impl PartialEq<str> for J {
        fn eq(&self, other: &str) -> bool {
            *self == J::from_str(other)
        }
    }
    impl<'a> PartialEq<&'a str> for J {
        fn eq(&self, other: &&'a str) -> bool {
            <J as PartialEq<str>>::eq(self, other)
        }
    }
    impl PartialEq<String> for J {
        fn eq(&self, other: &String) -> bool {
            <J as PartialEq<str>>::eq(self, other)
        }
    }
    pub fn run() {
        let j = J::from_str("a");
        println!("{} {} {} {} {}", <J as PartialEq<str>>::eq(&j, "a"), <J as PartialEq<str>>::ne(&j, "a"), j == "a", j == "b".to_string(), j == J::from_str("a"));
    }
}

// (an operator's impls for two right-hand sides, each with its own `Output`; and two traits' `Output` for one type)
mod ops {
    use std::ops::{Add, Neg, Not};
    pub struct S(pub i64);
    pub struct A(pub i64);
    pub struct B(pub i64);
    impl Add<A> for S {
        type Output = i64;
        fn add(self, a: A) -> Self::Output {
            self.0 + a.0
        }
    }
    impl Add<B> for S {
        type Output = String;
        fn add(self, b: B) -> Self::Output {
            format!("{}+{}", self.0, b.0)
        }
    }
    pub struct J(pub i32);
    impl Not for &J {
        type Output = bool;
        fn not(self) -> Self::Output {
            self.0 == 0
        }
    }
    impl Neg for &J {
        type Output = J;
        fn neg(self) -> Self::Output {
            J(-self.0)
        }
    }
    pub fn run() {
        let j = J(3);
        println!("{} {} {} {}", S(1) + A(2), S(1) + B(2), !&j, (-&j).0);
        use crate::K;
        println!("{} {} {} {}", (&K(6) & &K(3)).0, (&K(6) & K(5)).0, (K(6) & &K(12)).0, (K(7) & K(14)).0);
        println!("{} {} {} {}", (&K(1) << &K(3)).0, (&K(1) << K(4)).0, (K(1) << &K(5)).0, (K(3) << K(2)).0);
    }
}

// (wasm-bindgen's forwarding impls, at the top as in its lib.rs: `<&'static K as BitAnd<&'static K>>::Output` is
// BitAnd's, not Not's)
pub struct K(pub i32);
impl std::ops::Not for &K {
    type Output = bool;
    fn not(self) -> Self::Output {
        self.0 == 0
    }
}
impl std::ops::BitAnd for &K {
    type Output = K;
    fn bitand(self, rhs: Self) -> Self::Output {
        K(self.0 & rhs.0)
    }
}
macro_rules! forward_deref_binop {
    (impl $imp:ident, $method:ident for $t:ty) => {
        impl<'a> $imp<$t> for &'a $t {
            type Output = <&'static $t as $imp<&'static $t>>::Output;
            fn $method(self, other: $t) -> <&'static $t as $imp<&'static $t>>::Output {
                $imp::$method(self, &other)
            }
        }
        impl $imp<&$t> for $t {
            type Output = <&'static $t as $imp<&'static $t>>::Output;
            fn $method(self, other: &$t) -> <&'static $t as $imp<&'static $t>>::Output {
                $imp::$method(&self, other)
            }
        }
        impl $imp<$t> for $t {
            type Output = <&'static $t as $imp<&'static $t>>::Output;
            fn $method(self, other: $t) -> <&'static $t as $imp<&'static $t>>::Output {
                $imp::$method(&self, &other)
            }
        }
    };
}
impl std::ops::Shl for &K {
    type Output = K;
    fn shl(self, rhs: Self) -> Self::Output {
        K(self.0 << rhs.0)
    }
}
use std::ops::{BitAnd, Shl};
forward_deref_binop!(impl BitAnd, bitand for K);
forward_deref_binop!(impl Shl, shl for K);

fn main() {
    disjoint::run();
    ops::run();
    let parts: [&[u8]; 2] = [b"ab", b"cde"];
    let f: [u8; 5] = __rt::flat_byte_slices::<5, 2>(parts);
    println!("{} {:?}", __rt::one(), f);
    {
        use other::IntoAbi;
        let o = other::mk(1);
        println!("{} {} {} {} {} {} {} {}", rt::top().v, rt::via_path().v, rt::via_root().v, rt::nested(), rt::shim_call(), closure::mk(JsValue(4)).v, o.w, 7u8.into_abi());
    }
    conv::run();
    bounds::run();
    peq::run();
}
