//! A local declared before a loop, partly moved out of in it (`let a = x.0;`) and assigned in it (`x = ..`,
//! `x.0 = ..`): code lowered before the move runs again after it on the loop's next pass. zustc makes the flags a
//! split of the local needs before the loop (presplit_loop), so that code knows them: each value is dropped once, as
//! rustc's build drops it (the counts printed), none twice and none leaked.
//@ pattern: drops
//@ expect: run
use std::cell::Cell;
struct D<'a>(usize, &'a Vec<Cell<u32>>);
impl Drop for D<'_> {
    fn drop(&mut self) {
        let c = &self.1[self.0];
        c.set(c.get() + 1);
    }
}
fn check(n: &Vec<Cell<u32>>) {
    for (i, c) in n.iter().enumerate() {
        if c.get() > 1 {
            println!("{i} dropped {} times", c.get());
        }
    }
    let mut counts = String::new();
    for c in n.iter() {
        counts.push(char::from(b'0' + c.get() as u8));
    }
    println!("{counts}");
}
fn main() {
    let n: Vec<Cell<u32>> = (0..64).map(|_| Cell::new(0)).collect();
    {
        let mut x = (D(0, &n), D(1, &n));
        for i in 0..3 {
            x = (D(2 + i, &n), D(10 + i, &n));
            let a = x.0;
            drop(a);
        }
    }
    check(&n);
    {
        let mut x = (D(20, &n), D(21, &n));
        let mut i = 0;
        loop {
            if i == 2 {
                break;
            }
            x.1 = D(30 + i, &n);
            let a = x.0;
            drop(a);
            x.0 = D(40 + i, &n);
            i += 1;
        }
    }
    check(&n);
    {
        let mut x = (D(50, &n), D(51, &n));
        for i in 0..2 {
            let y = x;
            drop(y);
            x = (D(52 + i, &n), D(56 + i, &n));
            if i == 1 {
                break;
            }
            let a = x.0;
            drop(a);
            x.0 = D(60 + i, &n);
        }
    }
    check(&n);
}
