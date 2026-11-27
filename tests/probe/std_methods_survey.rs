// std methods and impls the top crates call that the prelude lacked (#256): a Mutex's PoisonError and its into_inner
// (critical-section), MaybeUninit::assume_init_read, Waker::will_wake (atomic-waker), String::as_mut_vec
// (data-encoding), io::Error::from_raw_os_error (errno), ChunksExact::len (simd-adler32), a slice iterator's
// size_hint (mime), Chars::rfold (unicode-width), `From<&[T]>` / `From<&str>` / `From<String>` for Cow
// (percent-encoding, shlex), `x.into()` of a `T: Into<u128>` with nothing expected of it (foldhash) and a raw
// pointer's `cast()` of an associated const of a type parameter (base64)
use std::borrow::Cow;
use std::io;
use std::mem::MaybeUninit;
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Waker};

static GLOBAL: Mutex<()> = Mutex::new(());

struct Sponge {
    sponge: u128,
    len: u8,
}

impl Sponge {
    fn write_num<T: Into<u128>>(&mut self, x: T) {
        let bits: usize = 8 * core::mem::size_of::<T>();
        if self.len as usize + bits > 128 {
            self.sponge = x.into();
            self.len = bits as u8;
        } else {
            self.sponge |= x.into() << self.len;
            self.len += bits as u8;
        }
    }
}

trait Lut {
    const LUT: [i8; 4];
}

struct A;

impl Lut for A {
    const LUT: [i8; 4] = [1, 2, 3, 4];
}

unsafe fn load(p: *const u32) -> u32 {
    unsafe { p.read_unaligned() }
}

// (an inherent `into(&self) -> &Self` called on a `&S` where a `&S` is expected: that method, which prints, not the
// value itself as std's `From<T> for T` (#280's review))
struct Own(u8);

impl Own {
    fn into(&self) -> &Own {
        println!("Own::into {}", self.0);
        self
    }
}

// (`true.into()` into its own type in a fn with a `T: Into<u64>` bound: a bool, not the bound's u64 (#280's review))
fn always<T: Into<u64>>(_: T) -> bool {
    true.into()
}

fn first_word<L: Lut>() -> u32 {
    let lut = L::LUT;
    unsafe { load(lut.as_ptr().cast()) }
}

fn width<S: DoubleEndedIterator<Item = char>>(s: S) -> usize {
    s.rfold(0usize, |sum, c| sum * 3 + c as usize % 5)
}

fn quote(b: &[u8]) -> Cow<'_, [u8]> {
    if b.is_empty() {
        return b"''"[..].into();
    }
    Cow::Borrowed(b)
}

fn text(b: bool) -> Cow<'static, str> {
    if b { "".into() } else { String::from("owned").into() }
}

fn from_bytes(b: &[u8]) -> Cow<'_, str> {
    std::str::from_utf8(b).unwrap().into()
}

#[derive(Debug)]
struct Errno(i32);

impl std::fmt::Display for Errno {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "errno {}", self.0)
    }
}

// (with the Error trait in the program, `std::io::Error::f` is still io::Error's)
impl std::error::Error for Errno {}

impl From<Errno> for io::Error {
    fn from(e: Errno) -> io::Error {
        std::io::Error::from_raw_os_error(e.0)
    }
}

fn os_error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn main() {
    let guard = match GLOBAL.lock() {
        Ok(g) => g,
        Err(err) => err.into_inner(),
    };
    let mut slot: MaybeUninit<MutexGuard<'static, ()>> = MaybeUninit::uninit();
    slot.write(guard);
    #[allow(let_underscore_lock)]
    let _ = unsafe { slot.assume_init_read() };
    let m = Mutex::new(5);
    println!("{}", m.into_inner().unwrap());
    let x = MaybeUninit::new(42u32);
    println!("{}", unsafe { x.assume_init_read() });
    let mut y: MaybeUninit<String> = MaybeUninit::uninit();
    y.write(String::from("s"));
    unsafe { y.assume_init_mut().push('!') };
    let s = unsafe { y.assume_init_read() };
    println!("{s}");

    let w = Waker::noop();
    let other = Waker::noop();
    let old: Option<&Waker> = Some(other);
    match old {
        Some(o) if o.will_wake(w) => println!("same task"),
        _ => println!("another task"),
    }
    let cx = Context::from_waker(w);
    println!("{}", cx.waker().will_wake(other));

    let mut out = String::from("ab");
    unsafe {
        let v = out.as_mut_vec();
        let n = v.len();
        v.resize(n + 2, b'c');
        v.push(b'd');
    }
    println!("{out}");

    let e = os_error(2);
    let n: io::Error = Errno(13).into();
    println!("{:?} {:?}", e.raw_os_error(), n.raw_os_error());
    println!("{} / {}", e, std::io::Error::from_raw_os_error(13));
    println!("{:?}", n);
    // (a raw code of 0 or below is an OS error too, apart from the kinds the prelude gives its own errors (#280's
    // review): it comes back from raw_os_error)
    for c in [0, -1, -3, -4] {
        let e = io::Error::from_raw_os_error(c);
        println!("{:?} {}", e.raw_os_error(), e);
    }
    println!("{:?}", io::Error::from_raw_os_error(0));

    let data = [1u8, 2, 3, 4, 5, 6, 7];
    let mut chunks = data.chunks_exact(2);
    println!("{}", chunks.len());
    chunks.next();
    println!("{} {:?}", chunks.len(), chunks.remainder());
    let pairs = [(1, 2), (3, 4), (5, 6)];
    let mut it = pairs.iter();
    println!("{:?}", it.size_hint());
    it.next();
    println!("{:?}", it.size_hint());
    println!("{} {}", width("abc".chars()), width("h\u{e9}!".chars()));

    println!("{:?} {:?}", quote(b"").into_owned(), quote(b"x").into_owned());
    println!("{:?} {:?} {:?}", text(true), text(false), from_bytes(b"xy"));

    let mut h = Sponge { sponge: 0, len: 0 };
    h.write_num(3u8);
    h.write_num(5u16);
    h.write_num(7u32);
    h.write_num(9u64);
    h.write_num(1u64);
    println!("{} {}", h.sponge, h.len);
    println!("{}", first_word::<A>());
    println!("{} {}", always(true), always(3u8));
    let own = Own(3);
    let r: &Own = &own;
    let back: &Own = r.into();
    println!("{}", back.0);
}
