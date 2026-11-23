// What memchr 2.8.3 needed (besides its SIMD impls): `Box::from` of a slice, the deref coercion `&&T` -> `&T` at
// a fn's tail and a match arm, a method named as another module's fn, `p.cast()` without a turbofish,
// `offset_from` (a `*mut` from a `*const` too), `unwrap_unchecked`, `Self::C` in an impl's const, `V::Mask::m` as a value, `p.add(2 * T::C)`,
// and the prelude's `Skip { .. }` beside a glob-imported variant `Skip`
mod other {
    pub fn find(h: &[u8]) -> usize {
        h.len()
    }
    // (a method of an impl for a fn pointer type keeps its name, as the trait's)
    pub trait T {
        fn find(&self) -> u8;
    }
    impl T for fn() -> u8 {
        fn find(&self) -> u8 {
            self() + 10
        }
    }
    fn seven() -> u8 {
        7
    }
    // (and of an impl whose head has a const argument, `S<{ 1 }>`)
    pub struct C<const N: usize>;
    impl T for C<{ 1 }> {
        fn find(&self) -> u8 {
            20
        }
    }
    pub fn via_const_arg() -> u8 {
        C::<1>.find()
    }
    pub fn via_fn_ptr() -> u8 {
        let f: fn() -> u8 = seven;
        T::find(&f)
    }
    // (a fn nested in a body keeps its calls: renamed with them, not taken for the module's `find`)
    pub fn run(h: &[u8]) -> usize {
        fn find(h: &[u8]) -> usize {
            h.len() * 100
        }
        find(h)
    }
}

mod memmem {
    pub fn find(h: &[u8], n: &[u8]) -> Option<usize> {
        Finder::new(n).find(h)
    }
    pub struct Finder<'n> {
        n: &'n [u8],
    }
    impl<'n> Finder<'n> {
        pub fn new(n: &'n [u8]) -> Finder<'n> {
            Finder { n }
        }
        pub fn find(&self, h: &[u8]) -> Option<usize> {
            h.windows(self.n.len()).position(|w| w == self.n)
        }
    }
}

enum Imp<'a> {
    Borrowed(&'a [u8]),
    Owned(Box<[u8]>),
}

impl<'a> Imp<'a> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Imp::Owned(ref x) => x,
            Imp::Borrowed(x) => x,
        }
    }
    fn into_owned(self) -> Imp<'static> {
        match self {
            Imp::Borrowed(b) => Imp::Owned(Box::from(b)),
            Imp::Owned(b) => Imp::Owned(b),
        }
    }
}

trait MoveMask: Copy {
    fn first_offset(self) -> usize;
}
#[derive(Clone, Copy)]
struct Mask(u32);
impl MoveMask for Mask {
    fn first_offset(self) -> usize {
        self.0.trailing_zeros() as usize
    }
}
trait Vector: Copy {
    const BYTES: usize;
    const ALIGN: usize;
    type Mask: MoveMask;
    fn mask(self) -> Self::Mask;
}
#[derive(Clone, Copy)]
struct V8(u32);
impl Vector for V8 {
    const BYTES: usize = 8;
    const ALIGN: usize = Self::BYTES - 1;
    type Mask = Mask;
    fn mask(self) -> Mask {
        Mask(self.0)
    }
}
struct Plain;
impl Plain {
    const N: usize = 3;
    const TWICE: usize = Self::N * 2;
}

fn offset_of<V: Vector>(v: V) -> usize {
    let topos = V::Mask::first_offset;
    topos(v.mask())
}

fn lz<V: Vector>() -> u32 {
    let n = 2 * V::BYTES;
    n.leading_zeros()
}

unsafe fn second_chunk<V: Vector>(p: *const u8) -> *const u8 {
    p.add(1 * V::BYTES)
}

fn read_u32(p: *const u32) -> u32 {
    unsafe { p.read_unaligned() }
}

#[derive(Debug, Clone, Copy)]
enum SuffixOrdering {
    Accept,
    Skip,
    Push,
}

fn cmp(a: u8, b: u8) -> SuffixOrdering {
    use self::SuffixOrdering::*;
    if a < b {
        Accept
    } else if a > b {
        Skip
    } else {
        Push
    }
}

fn main() {
    println!("{:?} {:?} {} {} {}", memmem::find(b"abcdef", b"cd"), memmem::find(b"abc", b"x"), other::find(b"qq"), other::run(b"qq"), other::via_fn_ptr() + other::via_const_arg());

    let a: &[u8] = &[1, 2, 3];
    let b: Box<[u8]> = Box::<[u8]>::from(a);
    let o = Imp::Borrowed(&a[1..]).into_owned();
    println!("{:?} {:?} {:?}", &b[..], o.as_slice(), Imp::Borrowed(a).as_slice());

    // (u32 words: the reads through `p.cast()` are aligned)
    let mut words = [1u32.to_le(), 2u32.to_le(), 9, 9];
    let p: *const u8 = words.as_ptr().cast();
    let x: u32 = unsafe { *p.cast() };
    let y = read_u32(unsafe { p.add(4) }.cast());
    let q = unsafe { second_chunk::<V8>(p) };
    let d = unsafe { q.offset_from(p) };
    let back = unsafe { p.offset_from(q) };
    let n = unsafe { usize::try_from(d).unwrap_unchecked() };
    let s = unsafe { Some(7u8).unwrap_unchecked() };
    let pm: *mut u8 = words.as_mut_ptr().cast();
    let mixed = unsafe { pm.add(12).offset_from(p) };
    println!("{} {} {} {} {} {} {}", x, y, d, back, n, s, mixed);

    println!("{} {} {} {} {}", V8::BYTES, <V8 as Vector>::ALIGN, Plain::TWICE, offset_of(V8(8)), lz::<V8>());

    let v: Vec<u8> = [1u8, 2, 3, 4].iter().skip(2).copied().collect();
    println!("{:?} {:?} {:?} {:?}", v, cmp(1, 2), cmp(2, 1), cmp(1, 1));
}
