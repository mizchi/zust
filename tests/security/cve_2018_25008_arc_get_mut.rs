//! CVE-2018-25008 (std, fixed in 1.29.0): `Arc::get_mut` checked "am I the only owner" with too weak
//! synchronization, so it could hand out `&mut T` while another thread still read through a clone. The contract:
//! `get_mut` is Some only while there is exactly one Arc.
//@ cve: CVE-2018-25008
//@ expect: run
use std::sync::Arc;
fn main() {
    let mut a = Arc::new(String::from("one"));
    println!("{}", Arc::get_mut(&mut a).is_some());
    let b = a.clone();
    println!("{} {}", Arc::get_mut(&mut a).is_some(), Arc::strong_count(&a));
    let h = std::thread::spawn(move || -> usize { b.as_ref().len() });
    println!("{}", h.join().unwrap());
    if let Some(s) = Arc::get_mut(&mut a) {
        s.push_str(" only");
    }
    println!("{} {}", a, Arc::strong_count(&a));
}
