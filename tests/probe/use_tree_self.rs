// `self` in a `use` tree, and paths inside a tree's braces (#254): `use super::Sign::{self, Minus};` brings the enum
// itself (num-bigint), `use crate::util::{alphabet::{self, ByteClasses}};` the module (regex-automata), and in
// `pub use crate::{.., unicode::CaseFoldError};` beside `use crate::unicode;` the path goes on from the tree's
// prefix (regex-syntax), while a group right after `use` (`use {a::q as b};`) names from the scope. A `self` entry
// brings the module only: `use crate::a::m::{self};` leaves the call `m()` to the glob's fn, not a's fn `m`. Sign,
// Unit and CaseFoldError are defined twice, so the flattening renames them
mod bigint {
    use self::Sign::{Minus, NoSign, Plus};

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Sign {
        Minus,
        NoSign,
        Plus,
    }

    pub fn neg(s: Sign) -> Sign {
        match s {
            Sign::Minus => Plus,
            Sign::NoSign => NoSign,
            Sign::Plus => Minus,
        }
    }

    pub mod multiplication {
        use super::Sign::{self, Minus, NoSign, Plus};

        pub fn mul(a: Sign, b: Sign) -> Sign {
            match (a, b) {
                (NoSign, _) | (_, NoSign) => NoSign,
                (Plus, Plus) | (Minus, Minus) => Plus,
                (Plus, Minus) | (Minus, Plus) => Minus,
            }
        }

        pub fn sign_of(n: i32) -> Sign {
            if n < 0 { Sign::Minus } else if n == 0 { Sign::NoSign } else { Sign::Plus }
        }
    }
}

mod util {
    pub mod alphabet {
        #[derive(Debug)]
        pub struct Unit(pub u8);
        pub struct ByteClasses(pub [u8; 4]);
        impl ByteClasses {
            pub fn get(&self, b: u8) -> Unit {
                Unit(self.0[b as usize % 4])
            }
        }
    }
}

mod dense {
    use crate::util::{alphabet::{self, ByteClasses}};

    pub fn first(c: &ByteClasses) -> (alphabet::Unit, alphabet::Unit) {
        (c.get(0), c.get(3))
    }
}

mod unicode {
    #[derive(Debug)]
    pub struct CaseFoldError(pub ());

    pub fn fold(n: u8) -> Result<u8, CaseFoldError> {
        if n > 3 { Err(CaseFoldError(())) } else { Ok(n + 1) }
    }
}

mod hir {
    use crate::unicode;

    pub use crate::{
        hir::visitor::{visit, Visitor},
        unicode::CaseFoldError,
    };

    pub mod visitor {
        pub struct Visitor(pub u8);
        pub fn visit(v: &Visitor) -> u8 {
            v.0 * 2
        }
    }

    pub fn fold(n: u8) -> core::result::Result<u8, CaseFoldError> {
        unicode::fold(visit(&Visitor(n)))
    }
}

mod a {
    pub mod m {
        pub fn inner() -> u8 {
            3
        }
    }
    #[allow(dead_code)]
    pub fn m() -> u8 {
        1
    }
}

mod b {
    pub fn m() -> u8 {
        2
    }
}

mod user {
    use crate::a::m::{self};
    use crate::b::*;
    use crate::util as u;
    use {u::alphabet as al};

    pub fn run() -> (u8, u8) {
        (m() * 10 + m::inner(), al::ByteClasses([5, 6, 7, 8]).get(1).0)
    }
}

mod other {
    #[derive(Debug)]
    pub struct Unit;
    #[derive(Debug)]
    pub enum Sign {
        Zero,
    }
    #[derive(Debug)]
    pub struct CaseFoldError;
}

fn main() {
    use bigint::multiplication::{mul, sign_of};
    println!("{:?} {:?} {:?}", bigint::neg(bigint::Sign::Plus), mul(sign_of(-2), sign_of(5)), mul(sign_of(0), sign_of(1)));
    let c = util::alphabet::ByteClasses([1, 2, 3, 4]);
    println!("{:?} {:?} {:?} {:?}", dense::first(&c), other::Unit, other::Sign::Zero, other::CaseFoldError);
    println!("{:?} {:?} {:?}", hir::fold(1), hir::fold(2).is_err(), user::run());
}
