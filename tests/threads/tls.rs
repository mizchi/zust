// thread_local!: each thread has its own value, made at its first use there; a spawned thread's values are dropped
// when it ends (newest first), before its join returns
use std::cell::{Cell, RefCell};
use std::thread;

struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

thread_local! {
    static N: Cell<u32> = Cell::new(1);
    static V: RefCell<Vec<u32>> = RefCell::new(Vec::new());
    static A: RefCell<Option<D>> = RefCell::new(None);
    static B: RefCell<Option<D>> = RefCell::new(None);
}

fn main() {
    N.set(10);
    V.with_borrow_mut(|v| v.push(0));
    let hs: Vec<_> = (1..5u32)
        .map(|k| {
            thread::spawn(move || {
                let before = N.get();
                N.set(before + k);
                for i in 0..k {
                    V.with_borrow_mut(|v| v.push(i));
                }
                (before, N.get(), V.with_borrow(|v| v.len()))
            })
        })
        .collect();
    for h in hs {
        println!("{:?}", h.join().unwrap());
    }
    let h = thread::spawn(|| {
        A.with(|a| *a.borrow_mut() = Some(D(1)));
        B.with(|b| *b.borrow_mut() = Some(D(2)));
        println!("child ends");
    });
    h.join().unwrap();
    println!("joined");
    println!("main {} {:?}", N.get(), V.with_borrow(|v| v.clone()));
}
