// An attribute's names are no items (#251): `#[derive(Eq, ..)]` beside the `pub type Eq<A, B>` a glob brings
// (typenum's `operator_aliases::*`) is core's derive; so are `Debug` / `Clone` / `Hash` beside a glob's type, struct
// and fn of those names, and `Ord` beside the module's own alias. `#[repr(C)]` beside a `struct C`, `#[default]`
// beside a `fn default` and `write!` beside a `fn write` keep their meaning. Each name is defined in two modules, so
// that the flattening renames them; and the same beside the items of a fn body, which the hoisting renames
#![allow(dead_code, non_snake_case, non_camel_case_types)]
use std::collections::HashSet;
use std::fmt::Write as _;

mod operator_aliases {
    pub type Eq<A, B> = (A, B);
    pub type Cmp<A, B> = Result<A, B>;
    pub type Le<A, B> = Option<(B, A)>;
}

mod names {
    pub type Debug = u8;
    pub struct Clone(pub u8);
    pub fn Hash() -> u8 {
        7
    }
    pub struct C;
    pub fn write() -> u8 {
        3
    }
    pub fn default() -> u8 {
        9
    }
}

mod other {
    pub type Eq<A, B> = (B, A);
    pub type Cmp = u8;
    pub type Le = u16;
    pub type Debug = u16;
    pub struct Clone;
    pub fn Hash() {}
    pub struct C;
    pub fn write() {}
    pub fn default() {}
    pub type Ord = i8;
}

mod local {
    pub type Ord = u32;
    #[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
    pub struct K(pub Ord);
}

use names::*;
use operator_aliases::*;

/// typenum's lib.rs:91
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash, Debug, Default)]
pub struct Greater;

#[derive(Debug, Clone, Hash, PartialEq, core::cmp::Eq)]
struct S(u8);

#[derive(::core::fmt::Debug, std::clone::Clone, core::default::Default)]
struct T {
    n: u8,
}

#[repr(C)]
struct R {
    a: u8,
    b: u32,
    c: u8,
}

#[derive(Debug, Default, PartialEq)]
enum E {
    #[default]
    A,
    B,
}

// (a fn's own `struct PartialOrd` and `fn print`, beside another fn's of those names: the hoisting renames them)
fn nested_a() -> u8 {
    struct PartialOrd(u8);
    fn print() -> u8 {
        1
    }
    PartialOrd(print()).0
}

fn nested_b() -> bool {
    struct PartialOrd(u8);
    fn print() -> u8 {
        2
    }
    #[derive(PartialEq, PartialOrd)]
    struct N(u8);
    print!("{} ", print());
    N(PartialOrd(print()).0) < N(3)
}

fn main() {
    // (the aliases are still the types)
    let e: Eq<u8, u16> = (1u8, 2u16);
    let c: Cmp<u8, u16> = Ok(3u8);
    let l: Le<u8, u16> = Some((4u16, 5u8));
    println!("{:?} {:?} {:?}", e, c, l);
    // (the derives are core's)
    let g = Greater;
    println!("{:?} {} {:?} {:?}", g, g == Greater, g.cmp(&Greater), Greater::default());
    let mut gs = HashSet::new();
    gs.insert(g);
    gs.insert(Greater);
    let d: Debug = 5;
    let cl = Clone(d);
    let s = S(cl.0);
    let mut ss = HashSet::new();
    ss.insert(s.clone());
    ss.insert(S(5));
    ss.insert(S(6));
    println!("{:?} {} {} {}", s.clone(), s == S(5), gs.len(), ss.len());
    println!("{:?} {:?}", T { n: 6 }.clone(), T::default());
    let k = local::K(8);
    println!("{:?} {} {:?}", k, k < local::K(9), k.max(local::K(2)));
    // (the glob's fns of those names)
    println!("{} {} {}", Hash(), write(), default());
    // (`#[repr(C)]`: its layout, not the struct C)
    println!("{}", std::mem::size_of::<R>());
    // (`#[default]`: the variant)
    println!("{:?} {}", E::default(), E::default() == E::A);
    // (`write!`: std's macro, not the glob's fn)
    let mut out = String::new();
    write!(out, "{}-{}", write(), Hash()).unwrap();
    println!("{}", out);
    println!("{} {}", nested_a(), nested_b());
}
