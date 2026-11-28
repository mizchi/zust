// A place read or borrowed shared through a user `Deref` calls `deref`, one written or borrowed `&mut` calls
// `deref_mut`
use std::cell::Cell;
use std::ops::{Deref, DerefMut};

struct Counter<T> {
    imm: Cell<usize>,
    mutable: usize,
    value: T,
}

impl<T> Deref for Counter<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.imm.set(self.imm.get() + 1);
        &self.value
    }
}

impl<T> DerefMut for Counter<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.mutable += 1;
        &mut self.value
    }
}

struct Point {
    x: i32,
    y: i32,
}

fn main() {
    let mut p = Counter {
        imm: Cell::new(0),
        mutable: 0,
        value: Point { x: 0, y: 0 },
    };
    let _ = p.x;
    let a = p.y;
    let _ = &p.x;
    println!("{} {} {a}", p.imm.get(), p.mutable);
    let _ = &mut p.y;
    p.x += 3;
    p.y = 5;
    println!("{} {} {} {}", p.imm.get(), p.mutable, p.value.x, p.value.y);
    let mut v = Counter {
        imm: Cell::new(0),
        mutable: 0,
        value: [1, 2, 3],
    };
    v[0] += 10;
    let b = v[1];
    println!("{} {} {b} {}", v.imm.get(), v.mutable, v.value[0]);
    // (a scrutinee bound by `ref mut` is reached by `deref_mut`, by `ref` by `deref`)
    let mut c = Counter {
        imm: Cell::new(0),
        mutable: 0,
        value: 7,
    };
    match *c {
        ref mut n => *n += 1,
    }
    let ref mut z = *c;
    *z += 1;
    match *c {
        ref n => println!("n {n}"),
    }
    println!("{} {} {}", c.imm.get(), c.mutable, c.value);
}
