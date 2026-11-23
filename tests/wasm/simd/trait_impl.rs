// An impl of a trait for `v128` (memchr's `impl Vector for v128`): its consts through `Self`, its methods called
// through a type parameter and on a temporary vector whose method name another trait has too (`and`), `p.cast()`
// into an intrinsic's pointer, and an associated type's method as a value (`V::Mask::first_offset`)
use core::arch::wasm32::*;

trait MoveMask: Copy {
    fn and(self, o: Self) -> Self;
    fn has_non_zero(self) -> bool;
    fn first_offset(self) -> usize;
}

#[derive(Clone, Copy)]
struct SensibleMoveMask(u32);

impl MoveMask for SensibleMoveMask {
    fn and(self, o: SensibleMoveMask) -> SensibleMoveMask {
        SensibleMoveMask(self.0 & o.0)
    }
    fn has_non_zero(self) -> bool {
        self.0 != 0
    }
    fn first_offset(self) -> usize {
        self.0.trailing_zeros() as usize
    }
}

trait Vector: Copy {
    const BYTES: usize;
    const ALIGN: usize;
    type Mask: MoveMask;
    unsafe fn splat(b: u8) -> Self;
    unsafe fn load_unaligned(p: *const u8) -> Self;
    unsafe fn cmpeq(self, o: Self) -> Self;
    unsafe fn and(self, o: Self) -> Self;
    unsafe fn movemask(self) -> Self::Mask;
}

impl Vector for v128 {
    const BYTES: usize = 16;
    const ALIGN: usize = Self::BYTES - 1;
    type Mask = SensibleMoveMask;
    unsafe fn splat(b: u8) -> v128 {
        u8x16_splat(b)
    }
    unsafe fn load_unaligned(p: *const u8) -> v128 {
        v128_load(p.cast())
    }
    unsafe fn cmpeq(self, o: v128) -> v128 {
        u8x16_eq(self, o)
    }
    unsafe fn and(self, o: v128) -> v128 {
        v128_and(self, o)
    }
    unsafe fn movemask(self) -> SensibleMoveMask {
        SensibleMoveMask(u8x16_bitmask(self) as u32)
    }
}

struct Pair<V> {
    v1: V,
    v2: V,
}

impl<V: Vector> Pair<V> {
    unsafe fn find(&self, h: &[u8], mask: V::Mask) -> Option<usize> {
        let topos = V::Mask::first_offset;
        let start = h.as_ptr();
        let mut cur = start;
        let end = start.add(h.len() - 1);
        while cur.add(V::BYTES) <= end {
            let c1 = V::load_unaligned(cur);
            let c2 = V::load_unaligned(cur.add(1));
            let offsets = c1.cmpeq(self.v1).and(c2.cmpeq(self.v2)).movemask().and(mask);
            if offsets.has_non_zero() {
                return Some(cur.offset_from(start) as usize + topos(offsets));
            }
            cur = cur.add(1 * V::BYTES);
        }
        None
    }
}

fn main() {
    let h = b"the quick brown fox jumps over the lazy dog, then the fox naps";
    unsafe {
        let p = Pair { v1: u8x16_splat(b'f'), v2: <v128 as Vector>::splat(b'o') };
        println!("{} {}", <v128 as Vector>::BYTES, <v128 as Vector>::ALIGN);
        println!("{:?}", p.find(h, SensibleMoveMask(!0)));
        println!("{:?}", p.find(&h[20..], SensibleMoveMask(!0)));
        println!("{:?}", p.find(b"no match in this haystack at all!", SensibleMoveMask(!0)));
    }
}
