// Inherent methods of one name in impls for two instances of a type: either's `cloned` / `copied` on
// `Either<&L, &R>` and on `Either<&mut L, &mut R>`, and `impl W<i32>` beside `impl W<u8>`. The receiver chooses
#[derive(Debug, Clone, Copy, PartialEq)]
enum Either<L, R> {
    Left(L),
    Right(R),
}
use Either::{Left, Right};

impl<L, R> Either<&L, &R> {
    fn cloned(self) -> Either<L, R>
    where
        L: Clone,
        R: Clone,
    {
        match self {
            Left(l) => Left(l.clone()),
            Right(r) => Right(r.clone()),
        }
    }
    fn copied(self) -> Either<L, R>
    where
        L: Copy,
        R: Copy,
    {
        match self {
            Left(l) => Left(*l),
            Right(r) => Right(*r),
        }
    }
}

impl<L, R> Either<&mut L, &mut R> {
    fn cloned(self) -> Either<L, R>
    where
        L: Clone,
        R: Clone,
    {
        match self {
            Left(l) => Left(l.clone()),
            Right(r) => Right(r.clone()),
        }
    }
    fn copied(self) -> Either<L, R>
    where
        L: Copy,
        R: Copy,
    {
        match self {
            Left(l) => Left(*l),
            Right(r) => Right(*r),
        }
    }
}

struct W<T>(T);
impl W<i32> {
    fn f(&self) -> i32 {
        self.0 + 1
    }
}
impl W<u8> {
    fn f(&self) -> u8 {
        self.0 * 2
    }
}
impl<T> W<&T> {
    fn g(&self) -> u32 {
        1
    }
}
impl<T> W<&mut T> {
    fn g(&self) -> u32 {
        2
    }
}

// (a method with a scalar method's name, and overloads that take self differently)
struct V<T>(T);
impl V<i32> {
    fn checked_add(&self, x: i32) -> i32 {
        self.0 + x
    }
    fn h(self) -> i32 {
        self.0 + 1
    }
}
impl V<u8> {
    fn checked_add(&self, x: u8) -> u8 {
        self.0 * x
    }
    fn h(&self) -> u8 {
        self.0 * 2
    }
}

fn main() {
    let (a, b) = (String::from("x"), 5u8);
    let e: Either<&String, &u8> = Left(&a);
    println!("{:?}", e.cloned());
    let f: Either<&String, &u8> = Right(&b);
    println!("{:?}", f.cloned());
    let (mut c, mut d) = (3i32, 4u8);
    let g: Either<&mut i32, &mut u8> = Left(&mut c);
    println!("{:?}", g.copied());
    let h: Either<&mut i32, &mut u8> = Right(&mut d);
    println!("{:?}", h.cloned());
    let k: Either<&i32, &u8> = Left(&7);
    println!("{:?}", k.copied());
    println!("{} {}", W(1i32).f(), W(3u8).f());
    let w = W(10u8);
    println!("{}", w.f());
    let mut x = 0;
    println!("{} {}", W(&5).g(), W(&mut x).g());
    println!("{} {}", V(1i32).checked_add(2), V(3u8).checked_add(2));
    let v = V(3u8);
    let r = &v;
    println!("{} {} {}", V(1i32).h(), r.h(), v.h());
}
