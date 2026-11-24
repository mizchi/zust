use core::convert::Infallible;
use rand_core::{Rng, SeedableRng, TryRng};

// A small xorshift generator written against rand_core's traits (implement TryRng with an Infallible error;
// rand_core gives Rng to every such type).
struct Xs(u64);

impl TryRng for Xs {
    type Error = Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Infallible> { Ok(self.try_next_u64()? as u32) }
    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        Ok(x)
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Infallible> {
        rand_core::utils::fill_bytes_via_next_word(dst, || self.try_next_u64())
    }
}

impl SeedableRng for Xs {
    type Seed = [u8; 8];
    fn from_seed(seed: [u8; 8]) -> Self { Xs(u64::from_le_bytes(seed) | 1) }
}

fn main() {
    let mut r = Xs::seed_from_u64(42);
    let v: Vec<u64> = (0..4).map(|_| r.next_u64()).collect();
    println!("{:?}", v);
    let mut buf = [0u8; 13];
    r.fill_bytes(&mut buf);
    println!("{:?}", buf);
    println!("u32 {}", r.next_u32());
    println!("done");
}
