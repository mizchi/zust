// Names in modules that zustc's renaming of colliding items lost (the crate survey's #254, as one program):
// - a fn named like a primitive type (getrandom's `pub fn u32() -> Result<u32, Error>`, chrono's `fn char` beside
//   `|c: char|`, fastrand's `fn u8` beside `0u8`): the type, and a number literal's suffix, keep their names
// - a macro body that calls a macro with `*self => $i` (num-traits' `float_to_int_unchecked!`), or has
//   `if let E::V = r` (encoding_rs): neither `self` nor `E` is a binding of the body's own
// - an alias with a default type parameter used without it (miniz_oxide's `type Result<T, E = Error>`)
// - `use m::sigaction;` where m has `fn sigaction` and a glob brings `struct sigaction` (libc): the type is the struct
mod getrandom {
    mod backends {
        pub fn inner_u32() -> Result<u32, ()> {
            Ok(5)
        }
    }
    mod ext {
        #[allow(dead_code)]
        pub fn u32() -> Result<u32, ()> {
            Ok(6)
        }
    }
    pub fn u32() -> Result<u32, ()> {
        backends::inner_u32()
    }
    pub fn h(x: u32) -> Option<u32> {
        Some(x)
    }
}
mod chrono {
    pub mod scan {
        #[allow(dead_code)]
        pub(super) fn char(s: &str, c1: u8) -> Result<&str, ()> {
            let _ = c1;
            Ok(s)
        }
        pub fn digits(s: &str) -> usize {
            s.chars().filter(|c: &char| c.is_ascii_digit()).count()
        }
    }
    #[allow(dead_code)]
    pub fn char() -> u8 {
        1
    }
}
mod fastrand {
    pub mod global_rng {
        #[allow(dead_code)]
        pub fn u8() -> u8 {
            7
        }
        pub fn low() -> char {
            0x41u8 as char
        }
    }
    #[allow(dead_code)]
    pub fn u8() -> u8 {
        8
    }
}
macro_rules! conv {
    ($float:expr => $int:ty) => {
        ($float as $int)
    };
}
macro_rules! to_int {
    ($T:ident) => {
        trait ToI {
            fn to_i(&self) -> Option<i64>;
        }
        impl ToI for $T {
            fn to_i(&self) -> Option<i64> {
                if *self > -1.0 {
                    return Some(conv!(*self => i64));
                }
                let r = match 3 {
                    x => x + 1,
                };
                Some(r)
            }
        }
    };
}
to_int!(f32);
#[derive(Debug, Clone, Copy, PartialEq)]
enum DecoderResult {
    InputEmpty,
    OutputFull,
}
macro_rules! decode_fn {
    ($name:ident) => {
        fn $name(x: u8) -> (DecoderResult, usize) {
            let r = if x == 0 { DecoderResult::InputEmpty } else { DecoderResult::OutputFull };
            let mut n = 0usize;
            if let DecoderResult::InputEmpty = r {
                n += 1;
            }
            (r, n)
        }
    };
}
decode_fn!(decode);
mod deflate {
    pub mod compress {
        #[derive(Debug)]
        pub struct Error;
        type Res<T, E = Error> = Result<T, E>;
        pub struct B {
            pub n: u8,
        }
        impl B {
            pub fn flush(&mut self) -> Res<()> {
                if self.n > 0 { Ok(()) } else { Err(Error) }
            }
        }
        pub fn g(x: u8) -> Res<u8, u8> {
            Ok(x)
        }
    }
}
mod libc {
    mod new {
        pub(crate) mod glibc {
            mod bits {
                pub(crate) use super::sysdeps::linux::bits::sigaction;
            }
            pub(crate) mod signal {
                pub use crate::libc::new::glibc::bits::sigaction::*;
                pub fn sigaction(signum: i32, act: *const sigaction) -> i32 {
                    let _ = act;
                    signum
                }
            }
            mod sysdeps {
                pub(crate) mod linux {
                    pub(crate) mod bits {
                        #[allow(non_camel_case_types)]
                        pub(crate) mod sigaction {
                            #[allow(non_camel_case_types)]
                            #[derive(Clone, Copy)]
                            pub struct sigaction {
                                pub sa_flags: i32,
                            }
                        }
                    }
                }
            }
            pub use self::signal::*;
        }
        pub use self::glibc::*;
    }
    pub use self::new::*;
}
use libc::sigaction;
#[derive(Clone)]
struct Prev {
    info: sigaction,
}
fn main() {
    println!("{:?} {:?}", getrandom::u32(), getrandom::h(2));
    println!("{} {}", chrono::scan::digits("12ab3"), fastrand::global_rng::low());
    println!("{:?} {:?}", 2.5f32.to_i(), (-3.0f32).to_i());
    println!("{:?} {:?}", decode(0), decode(1));
    let mut b = deflate::compress::B { n: 1 };
    println!("{:?} {:?}", b.flush(), deflate::compress::g(2));
    let p = Prev { info: sigaction { sa_flags: 3 } };
    println!("{} {}", p.clone().info.sa_flags, libc::sigaction(2, &p.info));
}
