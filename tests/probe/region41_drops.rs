// Part 41 of the region check: Drop impls the definition allows (their self type's own parameters, bounds the
// definition gives or its fields imply), run; and `ref` bindings of closure parameters used where they live
#![allow(dead_code)]
use std::cell::RefCell;
use std::fmt::Display;

struct Guard<'a, T: ?Sized + Display + 'a> {
    name: &'a str,
    val: &'a T,
}
impl<'a, T: ?Sized + Display> Drop for Guard<'a, T> {
    fn drop(&mut self) {
        println!("drop {} = {}", self.name, self.val);
    }
}

struct Sorted<T: Ord>(Vec<T>);
impl<T: Ord + PartialOrd> Drop for Sorted<T> {
    fn drop(&mut self) {
        println!("sorted drop of {} items", self.0.len());
    }
}

struct Log<'a>(&'a RefCell<Vec<String>>, u8);
impl Drop for Log<'_> {
    fn drop(&mut self) {
        self.0.borrow_mut().push(format!("log {}", self.1));
    }
}

struct Pair<'a, 'b: 'a, T: 'b>(&'a &'b T);
impl<'x, 'y: 'x, U: 'y> Drop for Pair<'x, 'y, U> {
    fn drop(&mut self) {
        println!("pair drop");
    }
}

fn lens(v: Vec<String>) -> Vec<usize> {
    v.into_iter().map(|ref s| s.len()).collect()
}

fn main() {
    let s = String::from("text");
    {
        let _g = Guard { name: "s", val: s.as_str() };
        let _h = Guard { name: "n", val: &7u8 };
    }
    let mut v = Sorted(vec![3, 1, 2]);
    v.0.sort();
    println!("{:?}", v.0);
    drop(v);
    let cell = RefCell::new(vec![]);
    {
        let _a = Log(&cell, 1);
        let _b = Log(&cell, 2);
    }
    println!("{:?}", cell.borrow());
    let x = 5u8;
    let r = &x;
    {
        let _p = Pair(&r);
    }
    println!("{:?}", lens(vec![String::from("ab"), String::from("cde")]));
    let f = |(ref a, b): (String, u8)| a.len() + b as usize;
    println!("{}", f((String::from("xyz"), 2)));
}
