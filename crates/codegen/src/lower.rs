//! Turns a recorded instruction sequence (zust_ir::Recorder) into machine code: rebuilds it into minclif's FunctionBuilder, and
//! makes machine code either with minclif as is (register allocation + x86-64) or with the Cranelift backend (zust-clif)

use crate::ir::{Recorder, Reloc, StackSlot};

/// A backend that makes (machine code, relocations, traps) from a minclif recording (arguments: recording, slots zeroed at the entry, whether -O)
pub type ClBackend = fn(&crate::minclif::FunctionBuilder, &[StackSlot], bool) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String>;

/// Turns a recording into machine code. With minclif if cl is None
pub fn lower(rec: &Recorder, late: &[StackSlot], opt: bool, cl: Option<ClBackend>) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String> {
    let (mut fb, sm) = crate::minclif::from_recording(rec);
    // (Recording slot numbers to minclif ones)
    let late: Vec<StackSlot> = late.iter().map(|s| StackSlot(sm[s.0 as usize])).collect();
    let late = &late[..];
    match cl {
        // (Cranelift backend: turns the recorded instruction sequence into Cranelift IR and compiles it)
        Some(cl) => cl(&fb, late, opt),
        None => {
            if fb.has_simd() {
                return Err("minclif has no SIMD yet: build with the Cranelift backend (the default) or for wasm32 (issue #30)".into());
            }
            fb.set_zero(late);
            let t = fb.traps();
            let (c, r) = fb.finish_zeroing(late);
            Ok((c, r, t))
        }
    }
}

