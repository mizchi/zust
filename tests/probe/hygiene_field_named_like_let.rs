// A macro body's `let core` is renamed for hygiene, but not the fields named core: `res.core.core`, `Self { core: .. }`,
// the shorthand `Outer { core }` (chacha20 0.10's impl_chacha_rng!)
struct Inner { state: [u32; 2] }
struct Outer { core: Inner }
macro_rules! imp {
    ($T:ident) => {
        struct $T { core: Outer }
        impl $T {
            fn new(x: u32) -> Self {
                let core = Inner { state: [x, 0] };
                let mut res = Self { core: Outer { core } };
                res.core.core.state[1] = 5;
                res
            }
            fn twice(core: u32) -> u32 {
                core * 2
            }
            fn get(&self) -> u32 {
                self.core.core.state[0] + self.core.core.state[1]
            }
        }
    };
}
imp!(R8);
fn main() {
    println!("{} {}", R8::new(3).get(), R8::twice(4));
}
