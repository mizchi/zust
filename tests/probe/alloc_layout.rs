// `core::alloc::Layout` and LayoutError as smallvec and bumpalo use them (from_size_align and its checks, new, array,
// for_value, pad_to_align, align_to, extend, Debug / Display), and `std::alloc::{alloc, alloc_zeroed, realloc, dealloc}`
use std::alloc::{self, Layout, LayoutError};
use std::ptr::NonNull;

#[derive(Debug)]
enum AllocErr {
    CapacityOverflow,
}

impl From<LayoutError> for AllocErr {
    fn from(_: LayoutError) -> Self {
        AllocErr::CapacityOverflow
    }
}

// (as smallvec's: `Layout::array` written by hand)
fn layout_array<T>(n: usize) -> Result<Layout, AllocErr> {
    let size = std::mem::size_of::<T>().checked_mul(n).ok_or(AllocErr::CapacityOverflow)?;
    let align = std::mem::align_of::<T>();
    Ok(Layout::from_size_align(size, align)?)
}

#[allow(dead_code)]
#[repr(align(64))]
struct Wide([u8; 64]);

fn main() {
    let l = Layout::new::<u64>();
    println!("{} {}", l.size(), l.align());
    println!("{:?}", l);
    let a = Layout::array::<u32>(5).unwrap();
    println!("{} {}", a.size(), a.align());
    println!("{:?}", Layout::from_size_align(3, 3));
    match Layout::from_size_align(8, 0) {
        Ok(_) => println!("ok"),
        Err(e) => println!("{} / {:?}", e, e),
    }
    println!("{}", Layout::from_size_align(usize::MAX, 8).is_err());
    println!("{:?}", Layout::from_size_align(isize::MAX as usize - 7, 8).map(|l| l.size()));
    println!("{:?}", Layout::from_size_align(isize::MAX as usize - 6, 8).is_err());
    println!("{}", Layout::array::<u64>(usize::MAX / 4).is_err());
    let w = Layout::new::<Wide>();
    println!("{} {}", w.size(), w.align());
    let v = [1u16, 2, 3];
    let fv = Layout::for_value(&v);
    println!("{} {}", fv.size(), fv.align());
    let p = Layout::from_size_align(5, 4).unwrap().pad_to_align();
    println!("{} {}", p.size(), p.align());
    println!("{:?}", Layout::from_size_align(5, 4).unwrap().align_to(16).map(|l| (l.size(), l.align())));
    let (e, off) = Layout::new::<u8>().extend(Layout::new::<u32>()).unwrap();
    println!("{} {} {}", e.size(), e.align(), off);
    println!("{}", Layout::new::<u32>() == Layout::from_size_align(4, 4).unwrap());
    let la = layout_array::<u16>(4).unwrap();
    println!("{} {}", la.size(), la.align());
    println!("{:?}", layout_array::<u64>(usize::MAX).map(|l| l.size()));
    unsafe {
        let p = alloc::alloc(la) as *mut u16;
        for i in 0..4 {
            p.add(i).write(i as u16 * 10);
        }
        let q = alloc::realloc(p as *mut u8, la, 16) as *mut u16;
        let nla = Layout::from_size_align(16, 2).unwrap();
        for i in 4..8 {
            q.add(i).write(i as u16 * 10);
        }
        let s = std::slice::from_raw_parts(q, 8);
        println!("{:?}", s);
        alloc::dealloc(q as *mut u8, nla);
        let z = alloc::alloc_zeroed(Layout::array::<u32>(3).unwrap()) as *mut u32;
        println!("{:?}", std::slice::from_raw_parts(z, 3));
        alloc::dealloc(z as *mut u8, Layout::array::<u32>(3).unwrap());
        let big = Layout::from_size_align(256, 128).unwrap();
        let b = NonNull::new(alloc::alloc(big)).unwrap();
        println!("{}", b.as_ptr() as usize % 128);
        alloc::dealloc(b.as_ptr(), big);
    }
}
