// either's Deref: `impl<L: Deref, R: Deref<Target = L::Target>> Deref for Either<L, R>`, `is_str(&value)` of an
// `Either<String, &str>` (std's Deref of String / Vec / references for the bounds), and str's methods through it
use std::ops::{Deref, DerefMut};
enum Either<L, R> { Left(L), Right(R) }
use Either::*;
impl<L, R> Deref for Either<L, R>
where
    L: Deref,
    R: Deref<Target = L::Target>,
{
    type Target = L::Target;
    fn deref(&self) -> &Self::Target {
        match self { Left(inner) => &**inner, Right(inner) => &**inner }
    }
}
impl<L, R> DerefMut for Either<L, R>
where
    L: DerefMut,
    R: DerefMut<Target = L::Target>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self { Left(inner) => &mut *inner, Right(inner) => &mut *inner }
    }
}
fn is_str(s: &str) -> usize { s.len() }
fn main() {
    let value: Either<String, &str> = Left(String::from("test"));
    println!("{}", is_str(&value));
    let v2: Either<String, &str> = Right("hi");
    println!("{} {}", is_str(&v2), v2.len());
    let mut v3: Either<Vec<u8>, Vec<u8>> = Left(vec![1, 2]);
    let s: &[u8] = &v3;
    println!("{:?} {}", s, v3.len());
}
