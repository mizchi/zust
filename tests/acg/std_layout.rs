// std::alloc's Layout (from_size_align, array, extend, pad_to_align, its error's Display) and alloc / realloc / alloc_zeroed /
// dealloc, also of a type aligned past malloc's 16 bytes (the crate survey's #250: smallvec, bumpalo, zlib-rs)
use std::alloc::{alloc, alloc_zeroed, dealloc, realloc, Layout};
#[repr(align(64))]
struct Big([u8; 64]);
fn main() {
    let l = Layout::from_size_align(24, 8).unwrap();
    println!("{} {} {:?}", l.size(), l.align(), Layout::from_size_align(8, 3).is_err());
    let a = Layout::array::<u32>(5).unwrap();
    println!("{} {}", a.size(), a.align());
    let (e, off) = Layout::new::<u8>().extend(Layout::new::<u64>()).unwrap();
    println!("{} {} {}", e.size(), e.align(), off);
    println!("{}", e.pad_to_align().size());
    unsafe {
        let p = alloc(l);
        *p = 7;
        let q = realloc(p, l, 64);
        println!("{}", *q);
        dealloc(q, Layout::from_size_align(64, 8).unwrap());
        let z = alloc_zeroed(Layout::new::<Big>()) as *mut Big;
        println!("{} {}", (z as usize) % 64, (*z).0[63]);
        dealloc(z as *mut u8, Layout::new::<Big>());
    }
    println!("{}", Layout::from_size_align(1, 0).unwrap_err());
}
