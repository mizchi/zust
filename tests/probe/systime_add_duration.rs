// A SystemTime moved by a Duration (httpdate's `UNIX_EPOCH + Duration::from_secs(n)`): `+` / `-` / `+=` / `-=`
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn main() {
    let a = at(1475419451);
    let mut b = a;
    b += Duration::from_millis(1500);
    let c = b - Duration::from_secs(1);
    let mut d = c;
    d -= Duration::from_nanos(500_000_000);
    println!("{:?}", a.duration_since(UNIX_EPOCH).unwrap().as_secs());
    println!("{:?}", b.duration_since(a).unwrap());
    println!("{:?} {}", c.duration_since(a).unwrap(), d == a);
    println!("{} {}", SystemTime::UNIX_EPOCH + Duration::ZERO == UNIX_EPOCH, a > c - Duration::from_secs(10));
}
