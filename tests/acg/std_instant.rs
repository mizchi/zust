// std::time::Instant: now, elapsed, duration_since and its checked / saturating forms, checked_add / checked_sub,
// `+ Duration`, `+= Duration`, `- Duration`, `- Instant` (#250: parking_lot_core, crossbeam-utils)
use std::time::{Duration, Instant};
fn main() {
    let a = Instant::now();
    let mut x = 0u64;
    for i in 0..100000u64 { x = x.wrapping_add(i * i); }
    let b = Instant::now();
    println!("{} {}", b >= a, x);
    let d: Duration = b - a;
    println!("{}", d == b.duration_since(a));
    println!("{}", a.elapsed() >= d);
    let later = a + Duration::from_millis(5);
    println!("{:?}", later.duration_since(a));
    let mut c = a;
    c += Duration::from_secs(1);
    println!("{:?} {:?}", c - Duration::from_secs(1) == a, a.checked_duration_since(c));
    println!("{:?}", c.checked_sub(Duration::from_secs(1)).map(|t| t == a));
    println!("{:?}", a.saturating_duration_since(c));
    // (an Instant moved far ahead: its Debug's seconds are past i64 before the clock's base is taken off)
    let far = Instant::now().checked_add(Duration::from_secs(9223372032559808512));
    println!("{} {}", far.is_some(), far.map_or(false, |f| format!("{:?}", f).starts_with("Instant { tv_sec: 92233720")));
}
