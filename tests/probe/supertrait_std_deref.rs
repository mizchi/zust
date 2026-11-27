// stable_deref_trait 1.2.1's list of impls (#261): a marker trait whose supertrait is std's Deref. Each impl for a std
// type needs that type's own `Deref` impl, which std has for String, Vec, Box, Rc, Arc, the cell and lock guards and
// references (the prelude gives most of them inherent methods instead)
use std::borrow::Cow;
use std::cell::{Ref, RefCell, RefMut};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, MutexGuard, RwLockReadGuard, RwLockWriteGuard};

pub unsafe trait StableDeref: Deref {}
pub unsafe trait CloneStableDeref: StableDeref + Clone {}

unsafe impl<T: ?Sized> StableDeref for Box<T> {}
unsafe impl<T> StableDeref for Vec<T> {}
unsafe impl StableDeref for String {}
unsafe impl StableDeref for CString {}
unsafe impl StableDeref for OsString {}
unsafe impl StableDeref for PathBuf {}
unsafe impl<'a> StableDeref for Cow<'a, str> {}
unsafe impl<'a, T: Clone> StableDeref for Cow<'a, [T]> {}
unsafe impl<'a> StableDeref for Cow<'a, Path> {}
unsafe impl<'a> StableDeref for Cow<'a, CStr> {}
unsafe impl<'a> StableDeref for Cow<'a, OsStr> {}
unsafe impl<T: ?Sized> StableDeref for Rc<T> {}
unsafe impl<T: ?Sized> CloneStableDeref for Rc<T> {}
unsafe impl<T: ?Sized> StableDeref for Arc<T> {}
unsafe impl<T: ?Sized> CloneStableDeref for Arc<T> {}
unsafe impl<'a, T: ?Sized> StableDeref for Ref<'a, T> {}
unsafe impl<'a, T: ?Sized> StableDeref for RefMut<'a, T> {}
unsafe impl<'a, T: ?Sized> StableDeref for MutexGuard<'a, T> {}
unsafe impl<'a, T: ?Sized> StableDeref for RwLockReadGuard<'a, T> {}
unsafe impl<'a, T: ?Sized> StableDeref for RwLockWriteGuard<'a, T> {}
unsafe impl<'a, T: ?Sized> StableDeref for &'a T {}

// (a type of the program's with Deref of its own)
struct Name(String);

impl Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

unsafe impl StableDeref for Name {}

fn stable<T: StableDeref>(_: &T) -> bool {
    true
}

fn shared<T: CloneStableDeref>(t: &T) -> T {
    t.clone()
}

fn main() {
    let s = String::from("hello");
    println!("{} {}", stable(&s), s.len());
    let v = vec![1u8, 2, 3];
    println!("{} {}", stable(&v), v.len());
    let b = Box::new(5u32);
    println!("{} {}", stable(&b), *b);
    let r = Rc::new(7i32);
    let r2 = shared(&r);
    println!("{} {} {}", stable(&r), *r2, Rc::strong_count(&r));
    let a = Arc::new(8i64);
    println!("{} {}", stable(&a), *shared(&a));
    let c = RefCell::new(9u8);
    println!("{} {}", stable(&c.borrow()), *c.borrow());
    let x = 11u16;
    println!("{}", stable(&&x));
    let n = Name(String::from("zust"));
    println!("{} {}", stable(&n), n.len());
}
