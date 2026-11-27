// `usize::max_value()` / `i8::min_value()`: the integer types' deprecated associated fns, their `MAX` / `MIN` (#265:
// each was "cannot find value (unsupported path)"). In fns (ipnet's netmasks, minimal-lexical's `into_i32`), in a
// const a static and a const pattern read, and in an impl's const its const fn reads (getrandom's `FD_UNINIT` and
// `LazyUsize::UNINIT`), and in an array's length
use std::sync::atomic::{AtomicUsize, Ordering};

const FD_UNINIT: usize = usize::max_value();
static FD: AtomicUsize = AtomicUsize::new(FD_UNINIT);

struct LazyUsize(AtomicUsize);

impl LazyUsize {
    const UNINIT: usize = usize::max_value();

    const fn new() -> Self {
        Self(AtomicUsize::new(Self::UNINIT))
    }

    fn unsync_init(&self, init: impl FnOnce() -> usize) -> usize {
        let mut val = self.0.load(Ordering::Relaxed);
        if val == Self::UNINIT {
            val = init();
            self.0.store(val, Ordering::Relaxed);
        }
        val
    }
}

fn get_fd() -> Option<usize> {
    match FD.load(Ordering::Relaxed) {
        FD_UNINIT => None,
        val => Some(val),
    }
}

fn into_i32(value: usize) -> i32 {
    if value > i32::max_value() as usize { i32::max_value() } else { value as i32 }
}

fn netmask_u128(prefix_len: u8) -> u128 {
    u128::max_value().checked_shl((128 - prefix_len) as u32).unwrap_or(u128::min_value())
}

fn netmask_u32(prefix_len: u8) -> u32 {
    u32::max_value().checked_shl(32 - prefix_len as u32).unwrap_or(0)
}

fn main() {
    println!("{:?}", get_fd());
    FD.store(3, Ordering::Relaxed);
    println!("{:?}", get_fd());
    let lazy = LazyUsize::new();
    println!("{} {}", lazy.unsync_init(|| 7), lazy.unsync_init(|| 8));
    println!("{} {} {}", into_i32(5), into_i32(usize::max_value()), into_i32(1 << 40));
    println!("{:x} {:x} {:x} {}", netmask_u128(120), netmask_u128(0), netmask_u32(24), netmask_u32(0));
    println!("{} {} {} {}", i8::min_value(), i64::max_value(), u16::max_value(), isize::min_value());
    let table = [0u8; u8::max_value() as usize + 1];
    println!("{} {}", table.len(), u8::max_value().wrapping_add(1));
}
