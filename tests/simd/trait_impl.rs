// A trait implemented for a SIMD vector type, called through generics (memchr's `impl Vector for __m128i`), with
// the other patterns memchr's sources use: a raw pointer's `cast::<T>()`, `wrapping_shl`, `Box::from` of an array,
// an alias's `BITS`, a glob-imported variant named like a prelude type, and `impl DoubleEndedIterator`
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

pub trait Vector: Copy {
    const BYTES: usize;
    unsafe fn splat(b: u8) -> Self;
    unsafe fn first(self) -> u8;
}

#[cfg(target_arch = "x86_64")]
mod x86sse2 {
    use super::Vector;
    use std::arch::x86_64::*;
    impl Vector for __m128i {
        const BYTES: usize = 16;
        unsafe fn splat(b: u8) -> __m128i {
            _mm_set1_epi8(b as i8)
        }
        unsafe fn first(self) -> u8 {
            _mm_cvtsi128_si32(self) as u8
        }
    }
}

fn via<V: Vector>(b: u8) -> (u8, usize) {
    unsafe { (V::splat(b).first(), V::BYTES) }
}

type Mask = u16;
const MAX_LEN: usize = (Mask::BITS - 1) as usize;

#[derive(Debug)]
enum Ordering2 {
    Accept,
    Skip,
    Push,
}

fn cmp(a: u8, b: u8) -> Ordering2 {
    use self::Ordering2::*;
    if a < b {
        Accept
    } else if a > b {
        Skip
    } else {
        Push
    }
}

struct Count {
    a: u32,
    b: u32,
}

impl Iterator for Count {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.a < self.b {
            self.a += 1;
            Some(self.a - 1)
        } else {
            None
        }
    }
}

impl DoubleEndedIterator for Count {
    fn next_back(&mut self) -> Option<u32> {
        if self.a < self.b {
            self.b -= 1;
            Some(self.b)
        } else {
            None
        }
    }
}

fn main() {
    #[cfg(target_arch = "x86_64")]
    {
        let (a, n) = via::<__m128i>(7);
        println!("{} {}", a, n);
        let v: __m128i = unsafe { _mm_set1_epi8(9) };
        println!("{}", unsafe { v.first() });
    }
    let bytes = [1u8, 2, 3, 4, 5];
    let p: *const u8 = bytes.as_ptr();
    let w = unsafe { p.cast::<u32>().read_unaligned() };
    let h = 5u32.wrapping_shl(1).wrapping_add(u32::from(bytes[2]));
    println!("{} {}", w, h);
    let masks: Box<[Mask; 4]> = Box::from([!0; 4]);
    println!("{} {}", masks[1], MAX_LEN);
    println!("{:?} {:?} {:?}", cmp(1, 2), cmp(2, 1), cmp(1, 1));
    let v: Vec<u32> = Count { a: 0, b: 3 }.rev().collect();
    println!("{:?} {:?}", v, Count { a: 0, b: 5 }.next_back());
}
