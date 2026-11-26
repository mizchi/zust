// A blanket impl over a (sized) type parameter beside impls for std's unsized types: `impl<T> AsMut<T>` and
// `impl AsMut<Path>` / `AsMut<OsStr>` / `AsMut<CStr>` / `AsMut<[T]>` / `AsMut<str>` don't overlap (either's lib.rs)
use std::ffi::{CStr, OsStr};
use std::path::Path;

enum Either<L, R> {
    Left(L),
    Right(R),
}
use Either::*;

macro_rules! impl_specific_mut {
    ($t:ty) => {
        impl<L, R> AsMut<$t> for Either<L, R>
        where
            L: AsMut<$t>,
            R: AsMut<$t>,
        {
            fn as_mut(&mut self) -> &mut $t {
                match self {
                    Left(l) => l.as_mut(),
                    Right(r) => r.as_mut(),
                }
            }
        }
    };
}
impl_specific_mut!(str);
impl_specific_mut!(::std::path::Path);
impl_specific_mut!(::std::ffi::OsStr);
impl_specific_mut!(::std::ffi::CStr);

impl<L, R, Target> AsMut<Target> for Either<L, R>
where
    L: AsMut<Target>,
    R: AsMut<Target>,
{
    fn as_mut(&mut self) -> &mut Target {
        match self {
            Left(l) => l.as_mut(),
            Right(r) => r.as_mut(),
        }
    }
}

impl<L, R, Target> AsMut<[Target]> for Either<L, R>
where
    L: AsMut<[Target]>,
    R: AsMut<[Target]>,
{
    fn as_mut(&mut self) -> &mut [Target] {
        match self {
            Left(l) => l.as_mut(),
            Right(r) => r.as_mut(),
        }
    }
}

// (each impl through a bound, as either's `check_t!`: `Either<String, String>: AsMut<str>` takes the `str` impl, whose
// where clauses String meets)
fn check_mut<T: AsMut<str>>() {}
fn check_slice_mut<T: AsMut<[Item]>, Item>() {}
fn check_path_mut<T: AsMut<Path>>() {}

// (std's AsMut of String / Vec / arrays, through a bound and on the type)
fn first_to_nine<T: AsMut<[u8]>>(t: &mut T) {
    t.as_mut()[0] = 9;
}

fn str_len<T: AsMut<str>>(t: &mut T) -> usize {
    t.as_mut().len()
}

fn main() {
    check_mut::<Either<String, String>>();
    check_slice_mut::<Either<Vec<u8>, [u8; 2]>, u8>();
    check_path_mut::<Either<Box<Path>, Box<Path>>>();
    let mut v = vec![1u8, 2];
    first_to_nine(&mut v);
    let mut a = [3u8, 4];
    first_to_nine(&mut a);
    let mut s = String::from("abc");
    let n = str_len(&mut s);
    let m: &mut str = s.as_mut();
    let w: &mut [u8] = v.as_mut();
    w[1] = 7;
    let e: Either<u8, u8> = if n > 9 { Right(2) } else { Left(1) };
    let k = match e {
        Left(x) | Right(x) => x,
    };
    let _: Option<(&OsStr, &CStr)> = None;
    println!("{:?} {:?} {} {} {}", v, a, n, m.len(), k);
}
