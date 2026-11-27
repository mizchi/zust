// `std::sync::Once`: call_once runs its closure once (in a static, a field, and from several threads), is_completed,
// call_once_force with its OnceState, Debug
use std::cell::Cell;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

static INIT: Once = Once::new();
static COUNT: AtomicUsize = AtomicUsize::new(0);

struct Lazy {
    once: Once,
    val: Cell<u32>,
}

impl Lazy {
    fn get(&self, f: impl FnOnce() -> u32) -> u32 {
        if !self.once.is_completed() {
            self.once.call_once(|| self.val.set(f()));
        }
        self.val.get()
    }
}

fn main() {
    println!("{}", INIT.is_completed());
    for _ in 0..3 {
        INIT.call_once(|| {
            COUNT.fetch_add(1, Ordering::SeqCst);
        });
    }
    println!("{} {}", INIT.is_completed(), COUNT.load(Ordering::SeqCst));
    let l = Lazy { once: Once::new(), val: Cell::new(0) };
    println!("{}", l.get(|| 42));
    println!("{}", l.get(|| 7));
    let o = Once::new();
    o.call_once_force(|st| println!("poisoned: {}", st.is_poisoned()));
    o.call_once_force(|_| println!("not again"));
    println!("{}", o.is_completed());
    let handles: Vec<_> = (0..3)
        .map(|i| {
            std::thread::spawn(move || {
                static T: Once = Once::new();
                // (which thread runs it is the scheduler's choice: the threads run in parallel)
                T.call_once(|| println!("thread {} ran it", if i < 3 { "one" } else { "no" }));
                T.is_completed()
            })
        })
        .collect();
    for h in handles {
        println!("{}", h.join().unwrap());
    }
    println!("{:?}", Once::new());
    println!("{:#?}", INIT);
}
