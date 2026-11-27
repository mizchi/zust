// std::thread::Builder (the crate survey's #250 `Builder`: crossbeam-utils, crossbeam-epoch, crossbeam-deque,
// rayon-core): a name, which the thread's Thread::name gives, a stack size, and spawn's io::Result
use std::io;
use std::thread::{self, Builder};

struct ScopedBuilder {
    builder: thread::Builder,
}

impl ScopedBuilder {
    // (crossbeam-utils writes `self.builder = self.builder.name(name); self`, which needs #224's fix of a
    // reassigned field moved out of)
    fn name(self, name: String) -> ScopedBuilder {
        ScopedBuilder { builder: self.builder.name(name) }
    }
    fn stack_size(self, size: usize) -> ScopedBuilder {
        ScopedBuilder { builder: self.builder.stack_size(size) }
    }
    fn spawn<F: FnOnce() -> u32 + Send + 'static>(self, f: F) -> io::Result<thread::JoinHandle<u32>> {
        self.builder.spawn(f)
    }
}

fn main() -> io::Result<()> {
    let h = Builder::new()
        .name("worker-1".to_string())
        .stack_size(64 * 1024)
        .spawn(|| {
            let t = thread::current();
            format!("{:?} {}", t.name(), t.id() == thread::current().id())
        })?;
    println!("{}", h.join().unwrap());
    let h2 = thread::Builder::new().spawn(|| thread::current().name().map(|s| s.to_string()))?;
    let name2 = h2.thread().name().map(|s| s.to_string());
    println!("{:?} {:?}", h2.join().unwrap(), name2);
    let b = ScopedBuilder { builder: Builder::new() }.name(String::from("scoped")).stack_size(1 << 20);
    let h3 = b.spawn(|| 7)?;
    let name3 = h3.thread().name().map(|s| s.to_string());
    println!("{:?} {}", name3, h3.join().unwrap());
    println!("{:?}", thread::current().name());
    Ok(())
}
