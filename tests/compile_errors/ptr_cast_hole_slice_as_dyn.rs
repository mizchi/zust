use std::fmt::Debug;

fn main() {
    let f: Box<[u8]> = Box::new([0]);
    let b: Box<dyn Debug> = Box::new(1u8);
    let f_raw: *mut [u8] = Box::into_raw(f);
    let mut b_raw: *mut dyn Debug = Box::into_raw(b);
    b_raw = f_raw as *mut _;
    let _ = b_raw;
}
