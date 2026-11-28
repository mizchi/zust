// simdutf8's `type SimdU8Value = helpers::SimdU8Value<__m256i>; impl SimdU8Value { unsafe fn from_32_cut_off_leading(..) }`
// in one module and the same for `__m128i` in another: inherent associated functions without `self` (and consts), of
// one name, in impls for two instances of a generic type, and its `impl From<__m256i> for SimdU8Value` beside the
// `__m128i` one. Each path (through an alias, `Self`, a turbofish, a qualified self type) calls its own instance's
struct V<T>(T);
mod a {
    pub type V = super::V<u32>;
    impl V {
        pub fn make(x: u8) -> Self {
            super::V(x as u32 * 2)
        }
        pub fn twice(x: u8) -> Self {
            Self::make(x + x)
        }
        pub fn get(&self) -> u32 {
            self.0
        }
        pub fn wrap(x: u32) -> Self {
            Self::from(x)
        }
        pub const ONE: u32 = 1;
    }
    impl From<u32> for V {
        fn from(x: u32) -> Self {
            super::V(x + 1)
        }
    }
    pub fn run() -> u32 {
        V::make(3).get() + V::twice(1).0 + V::ONE + V::from(10).0 + V::wrap(20).0
    }
}
mod b {
    pub type V = super::V<u64>;
    impl V {
        pub fn make(x: u8) -> Self {
            super::V(x as u64 * 3)
        }
        pub fn twice(x: u8) -> Self {
            Self::make(x + x)
        }
        pub fn get(&self) -> u64 {
            self.0 + 100
        }
        pub fn wrap(x: u64) -> Self {
            Self::from(x)
        }
        pub const ONE: u64 = 1000;
    }
    impl From<u64> for V {
        fn from(x: u64) -> Self {
            super::V(x + 2)
        }
    }
    pub fn run() -> u64 {
        V::make(3).get() + V::twice(1).0 + V::ONE + V::from(10).0 + V::wrap(20).0
    }
}
fn main() {
    println!("{} {}", a::run(), b::run());
    println!("{} {}", V::<u32>::make(1).0, V::<u64>::make(1).0);
    let x: V<u64> = V::<u64>::make(2);
    println!("{} {}", x.get(), <V<u32>>::twice(5).get());
    println!("{} {}", V::<u32>::ONE, <V<u64>>::ONE);
    println!("{} {}", V::<u32>::wrap(7).0, V::<u64>::wrap(7).0);
}
