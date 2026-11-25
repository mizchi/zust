//! Time measurement (the time of each stage with ZUSTC_PROF=1). Does not use std::time (so that zustc itself can compile it)

unsafe extern "C" {
    #[link_name = "clock_gettime"]
    fn c_clock_gettime(clk: i32, ts: *mut i64) -> i32;
}

/// A monotonic clock (ns)
pub fn now_ns() -> u64 {
    let mut ts = [0i64; 2];
    // SAFETY: CLOCK_MONOTONIC (1). ts has the same layout as timespec
    unsafe { c_clock_gettime(1, ts.as_mut_ptr()) };
    ts[0] as u64 * 1_000_000_000 + ts[1] as u64
}

/// Prints the time of each stage (only when on)
pub struct Timer {
    pub on: bool,
    pub last: u64,
    pub start: u64,
}

impl Timer {
    pub fn lap(&mut self, what: &str, extra: String) {
        if !self.on {
            return;
        }
        let t = now_ns();
        eprintln!("  {:<10} {:>8.2} ms  {}", what, (t - self.last) as f64 / 1e6, extra);
        self.last = t;
    }
}
