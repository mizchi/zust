// A panic in a spawned thread ends that thread only: its message names it '<unnamed>', the note on RUST_BACKTRACE
// comes with the first panic only, its thread_local! values are dropped, and its join gives Err (Debug `Any { .. }`).
// The other threads, and main, go on
use std::cell::RefCell;
use std::thread;

struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

thread_local! {
    static T: RefCell<Option<D>> = RefCell::new(None);
}

fn main() {
    let h = thread::spawn(|| {
        T.with(|t| *t.borrow_mut() = Some(D("child tls")));
        let v: Vec<i32> = vec![];
        v[3]
    });
    println!("err {}", h.join().is_err());
    let h = thread::spawn(|| -> u8 {
        let x: u8 = 200;
        let y = x + std::hint::black_box(100);
        y
    });
    println!("{:?}", h.join());
    let ok = thread::spawn(|| 7).join();
    println!("{:?}", ok);
    let h = thread::spawn(|| -> () { panic!("boom {}", 3) });
    let e = h.join().unwrap_err();
    println!("{:?}", e);
    println!("main goes on");
}
