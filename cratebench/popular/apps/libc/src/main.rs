use std::ffi::CStr;

fn main() {
    unsafe {
        let pid = libc::getpid();
        println!("pid positive: {}", pid > 0);
        let s = libc::strlen(c"hello libc".as_ptr());
        println!("strlen {}", s);
        let p = libc::malloc(64) as *mut u8;
        println!("malloc non-null: {}", !p.is_null());
        libc::memset(p as *mut libc::c_void, 0x41, 63);
        *p.add(63) = 0;
        println!("memset len {}", CStr::from_ptr(p as *const libc::c_char).to_bytes().len());
        libc::free(p as *mut libc::c_void);
        let mut ts: libc::timespec = std::mem::zeroed();
        println!("clock_gettime ok: {}", libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) == 0);
        println!("abs {}", libc::abs(-7));
        println!("sizes {} {}", std::mem::size_of::<libc::c_int>(), std::mem::size_of::<libc::size_t>());
    }
    println!("done");
}
