// Impls for a program's own alias of a std type the prelude makes an alias of another (#261): `type MyOs = OsString`
// is OsString, whose impl is apart from String's in rustc; so is a generic alias's instance holding one, and an alias
// of Path beside str
use std::ffi::OsString;
use std::path::Path;

type MyOs = OsString;
type Pair<T> = (T, OsString);
type P = Path;

trait Tr {
    fn n(&self) -> u8 {
        3
    }
}

impl Tr for MyOs {}
impl Tr for String {}
impl Tr for Pair<u8> {}
impl Tr for (u8, String) {}
impl Tr for Box<P> {}
impl Tr for Box<str> {}

fn main() {
    println!("{}", String::from("a").n());
    println!("{}", (1u8, String::new()).n());
}
