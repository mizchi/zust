// std's items imported by their own name that are not the prelude's of that name (#260): fmt's alias `Result`, which
// has no parameters (`use core::fmt::{Debug, Formatter, Result};`, as bytes's src/fmt/debug.rs and hex.rs do, also
// in a macro its parent module defines), io's `Result<T>` and fmt's / io's `Error` in a type's place. The import is
// the type only: a glob's `fn Result` and the module's own are still the calls `Result()`, and in a block it shadows
// the module's own `type Result<T>`
use std::fmt::Result as FmtResult;
use std::fmt::{Display, Formatter, Result};

macro_rules! show_impl {
    ($tr:ident, $ty:ty) => {
        impl $tr for $ty {
            fn fmt(&self, f: &mut Formatter<'_>) -> Result {
                $tr::fmt(&Bytes(self.0), f)
            }
        }
    };
}

struct Bytes<'a>(&'a [u8]);
struct Wrap<'a>(&'a [u8]);
struct Name(&'static str);

impl Display for Name {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(self.0)
    }
}

fn check() -> FmtResult {
    Ok(())
}

mod debug {
    use core::fmt::{Debug, Formatter, Result};

    use super::{Bytes, Wrap};

    impl Debug for Bytes<'_> {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            write!(f, "b\"")?;
            for &b in self.0 {
                if b.is_ascii_graphic() {
                    write!(f, "{}", b as char)?;
                } else {
                    write!(f, "\\x{:02x}", b)?;
                }
            }
            write!(f, "\"")
        }
    }

    show_impl!(Debug, Wrap<'_>);
}

mod hex {
    use core::fmt::{self, LowerHex, Result, UpperHex};

    pub struct H(pub u8);

    impl LowerHex for H {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result {
            write!(f, "{:02x}", self.0)
        }
    }

    impl UpperHex for H {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result {
            write!(f, "{:02X}", self.0)
        }
    }

    // (fmt's `Error` in a type's place)
    pub fn first(r: core::result::Result<(), fmt::Error>) -> bool {
        r.is_ok()
    }
}

mod errors {
    use std::fmt::Error;

    pub fn ok() -> Result<u8, Error> {
        Ok(7)
    }
}

mod io_errors {
    use std::io::{self, Error, Result};

    pub fn size(p: &str) -> Result<usize> {
        std::fs::read(p).map(|v| v.len())
    }

    pub fn twice(p: &str) -> io::Result<usize> {
        size(p).map(|n| n * 2)
    }

    pub fn kind(r: &core::result::Result<usize, Error>) -> bool {
        r.is_err()
    }
}

// (a glob's value of the name: `Result()` is v's fn, not fmt's alias)
mod glob_value {
    mod v {
        #[allow(non_snake_case)]
        pub fn Result() -> u8 {
            7
        }
    }

    use core::fmt::{Display, Formatter, Result};
    use v::*;

    pub struct G;

    impl Display for G {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            write!(f, "g{}", Result())
        }
    }
}

// (the module's own `fn Result`, a value, beside the import of the type)
mod own_fn {
    use core::fmt::Result;

    #[allow(non_snake_case)]
    pub fn Result() -> u8 {
        4
    }

    pub struct F;

    impl core::fmt::Display for F {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result {
            write!(f, "f{}", Result())?;
            Result::Ok(())
        }
    }
}

// (a block's import shadows the module's own alias there, as a type; its `fn Result` is still the call)
mod own_alias {
    pub type Result<T> = core::result::Result<T, String>;

    #[allow(non_snake_case)]
    pub fn Result() -> u8 {
        3
    }

    pub struct Q;

    impl core::fmt::Display for Q {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            use core::fmt::Result;
            fn inner(f: &mut core::fmt::Formatter<'_>, n: u8) -> Result {
                write!(f, "q{n}")
            }
            inner(f, Result())
        }
    }

    pub fn parse(s: &str) -> Result<u8> {
        s.parse::<u8>().map_err(|e| e.to_string())
    }
}

fn main() {
    println!("{:?}", Bytes(b"hi\n"));
    println!("{:?}", Wrap(b"ok"));
    println!("{} {:?}", Name("name"), check());
    println!("{:x} {:X}", hex::H(171), hex::H(205));
    println!("{}", hex::first(Ok(())));
    println!("{:?}", errors::ok());
    let r = io_errors::twice("/nonexistent/zust-probe");
    println!("{} {}", io_errors::kind(&r), io_errors::size("/nonexistent/zust-probe").is_err());
    println!("{} {} {}", glob_value::G, own_fn::F, own_alias::Q);
    println!("{:?} {:?}", own_alias::parse("12"), own_alias::parse("x"));
}
