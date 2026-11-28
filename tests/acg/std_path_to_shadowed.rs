// A program's own type named as a prelude type (Duration, Instant, as chrono and others write) beside std's, which it
// names by path: `std::time::Duration::from_millis`, `use .. as StdDuration`, a `use` group's alias, in a module, and
// std's Instant's Debug under its own name
use std::time::{Duration as StdDuration, Instant as StdInstant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Duration {
    secs: i64,
}

impl Duration {
    fn from_std(d: StdDuration) -> Duration {
        Duration { secs: d.as_secs() as i64 }
    }
}

mod clock {
    pub struct Instant(pub u8);
    pub fn std_secs(ms: u64) -> u64 {
        ::std::time::Duration::from_millis(ms).as_secs()
    }
}

fn main() {
    let mine = Duration { secs: -3 };
    let s = std::time::Duration::from_millis(1500);
    let t = StdDuration::from_secs(2);
    println!("{:?} {} {:?}", mine, s.as_secs(), t);
    println!("{:?}", Duration::from_std(core::time::Duration::from_secs(7)));
    let c = clock::Instant(4);
    let now = StdInstant::now();
    println!("{} {} {}", c.0, clock::std_secs(2500), format!("{:?}", now).starts_with("Instant { tv_sec: "));
    // (a std path in a string or a comment is text: std::time::Duration)
    println!("{}", "std::time::Duration");
}
