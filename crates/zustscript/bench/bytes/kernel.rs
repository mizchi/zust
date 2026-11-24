// A small, fixed-size Wasm-owned region for the byte ABI size probe.
use std::sync::atomic::{AtomicU8, Ordering};

#[unsafe(no_mangle)]
pub static BUFFER: [AtomicU8; 16] = [const { AtomicU8::new(0) }; 16];

#[unsafe(no_mangle)]
pub extern "C" fn invert(len: usize) {
    for byte in BUFFER.iter().take(len) {
        byte.fetch_xor(255, Ordering::Relaxed);
    }
}
