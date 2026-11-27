// Crate items zustc lost (the crate survey's #254, the cases beside primitive names and macro `self`):
// - a macro body with `if let E::V = r` (encoding_rs): `E` is a path's segment, no binding of the body's own
// - an alias with a default type parameter used without it (miniz_oxide's `type Result<T, E = Error>`)
// - `use m::sigaction;` where m has `fn sigaction` and a glob brings `struct sigaction` (libc, as
//   signal-hook-registry imports it): the type is the struct, the call the fn
// - a macro body's `let x: ::std::primitive::u8` (a type that starts with `::`): `x` is still the body's own
//   binding, apart from the caller's `x`; and `let One::V(z)` / `if let E::V = ..` in an expression's body
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
            while let DecoderResult::OutputFull = r {
                n += 10;
                break;
            }
            (r, n)
        }
    };
}
decode_fn!(decode);
macro_rules! abs_ty {
    ($e:expr) => {{ let x: ::std::primitive::u8 = 1; $e + x }};
}
#[derive(Clone, Copy)]
enum One {
    V(u8),
}
macro_rules! abs_ty_mut {
    ($e:expr) => {{
        let mut x: ::core::primitive::u8 = 2;
        x += 1;
        let One::V(z) = One::V(4);
        let mut n = 0u8;
        if let DecoderResult::OutputFull = decode(1).0 {
            n += 1;
        }
        $e + x + z + n
    }};
}
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
        pub fn h(x: u8) -> Res<Vec<u8>> {
            if x > 1 { Ok(vec![x; 2]) } else { Err(Error) }
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
    println!("{:?} {:?}", decode(0), decode(1));
    let mut b = deflate::compress::B { n: 1 };
    println!("{:?} {:?}", b.flush(), deflate::compress::g(2));
    b.n = 0;
    println!("{:?} {:?} {:?}", b.flush(), deflate::compress::h(3), deflate::compress::h(0));
    let p = Prev { info: sigaction { sa_flags: 3 } };
    println!("{} {}", p.clone().info.sa_flags, libc::sigaction(2, &p.info));
    println!("{}", sigaction(5, std::ptr::null()));
    let x = 10u8;
    println!("{} {}", abs_ty!(x), abs_ty_mut!(x * 2));
}
