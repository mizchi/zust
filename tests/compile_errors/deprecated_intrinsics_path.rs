#![deny(deprecated)]
fn main() {
    let x: u32 = unsafe { std::mem::transmute(1.0f32) };
    let _ = x;
    // std::intrinsics::copy
    let s = "std::intrinsics::copy";
    unsafe { core::intrinsics::write_bytes(&mut 0u8 as *mut u8, 0, 1) };
    let _ = s;
}
