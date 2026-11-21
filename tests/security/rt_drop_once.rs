//! CVE-2021-31162 (`Vec::from_iter` in-place collection could drop elements twice) and the like:
//! every value is dropped exactly once on every path that moves, collects, truncates or drains.
//@ pattern: drops (each value dropped once: the class of CVE-2021-31162)
//@ expect: run
use std::cell::Cell;
struct D<'a>(u32, &'a Cell<u32>);
impl Drop for D<'_> {
    fn drop(&mut self) {
        self.1.set(self.1.get() + 1);
    }
}
fn main() {
    let drops = Cell::new(0);
    {
        let v: Vec<D> = (0..10).map(|i| D(i, &drops)).collect();
        let w: Vec<D> = v.into_iter().filter(|d| d.0 % 2 == 0).collect();
        println!("kept {} dropped {}", w.len(), drops.get());
    }
    println!("after scope {}", drops.get());
    drops.set(0);
    {
        let mut v: Vec<D> = (0..10).map(|i| D(i, &drops)).collect();
        v.truncate(7);
        let d: Vec<D> = v.drain(1..3).collect();
        v.retain(|x| x.0 != 5);
        let mut it = v.into_iter();
        it.next();
        println!("{} {}", d.len(), drops.get());
    }
    println!("after scope {}", drops.get());
    drops.set(0);
    {
        let mut slot = Some(D(2, &drops));
        let r = std::mem::replace(&mut slot, None);
        println!("{} {}", drops.get(), slot.is_none());
        drop(r);
        let mut a = D(3, &drops);
        let mut b = D(4, &drops);
        std::mem::swap(&mut a, &mut b);
        println!("{} {} {}", a.0, b.0, drops.get());
    }
    println!("after scope {}", drops.get());
}
