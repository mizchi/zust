//! A raw pointer is not Send (E0277): moving one into another thread is how safe code would share unsynchronized
//! memory.
//@ pattern: Send / Sync
//@ expect: reject
fn main() {
    let mut x = 0u64;
    let p = &mut x as *mut u64;
    let h = std::thread::spawn(move || { //~ ERROR E0277
        unsafe { *p += 1 };
    });
    h.join().unwrap();
    println!("{x}");
}
