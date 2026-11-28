// A type whose const parameter has a default, used without arguments (#260: const-oid's `ObjectIdentifier<const
// MAX_SIZE: usize = DEFAULT_MAX_SIZE>`): in an impl's self type, a trait impl's, a signature, a field, a `let`, an
// alias; a default that names a const, a literal one, one after a type parameter's default, one that names an earlier
// const parameter, and on an enum. In an expression's path the arguments are inferred, not the defaults (`let b:
// Buf<5> = Buf::default();`), and those it writes are the type's (`Cap::<5>::default()` is a `Cap<5>`)
use std::str::FromStr;

const DEFAULT_MAX: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Oid<const MAX: usize = DEFAULT_MAX> {
    len: u8,
    bytes: [u8; MAX],
}

impl Oid {
    const MAX_SIZE: usize = DEFAULT_MAX;

    fn new(s: &str) -> Result<Self, ()> {
        let mut bytes = [0u8; DEFAULT_MAX];
        let b = s.as_bytes();
        if b.len() > Self::MAX_SIZE {
            return Err(());
        }
        bytes[..b.len()].copy_from_slice(b);
        Ok(Oid { len: b.len() as u8, bytes })
    }
}

impl<const MAX: usize> Oid<MAX> {
    fn len(&self) -> usize {
        self.len as usize
    }
    fn cap(&self) -> usize {
        MAX
    }
}

impl FromStr for Oid {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Self::new(s)
    }
}

fn first(o: &Oid) -> u8 {
    o.bytes[0]
}

#[derive(Debug)]
struct Buf<const N: usize = 3> {
    data: [u8; N],
}

impl<const N: usize> Default for Buf<N> {
    fn default() -> Self {
        Buf { data: [0; N] }
    }
}

#[derive(Debug)]
struct Pair<T = u16, const K: usize = 2> {
    items: [T; K],
}

impl Default for Pair {
    fn default() -> Self {
        Pair { items: [1, 2] }
    }
}

#[derive(Debug)]
enum Slot<const N: usize = 2> {
    Empty,
    Full([i32; N]),
}

struct Holder {
    buf: Buf,
    pair: Pair,
    slot: Slot,
}

#[derive(Debug, Default)]
struct W<T = u8>(T);

impl<T> W<T> {
    fn new(t: T) -> Self {
        W(t)
    }
}

// (a derived Default: `Cap::<5>::default()` is a Cap<5>, not the default Cap<3>)
#[derive(Debug, Default)]
struct Cap<const N: usize = 3> {
    used: u8,
}

impl<const N: usize> Cap<N> {
    fn cap(&self) -> usize {
        N
    }
}

// (M's default is N's argument: `Twice` is `Twice<2, 2>`, also in an impl whose own `N` is another)
#[derive(Debug)]
struct Twice<const N: usize = 2, const M: usize = N> {
    a: [u8; N],
    b: [u8; M],
}

impl<const N: usize, const M: usize> Twice<N, M> {
    fn nm(&self) -> (usize, usize) {
        (self.a.len() * 10 + N, self.b.len() * 10 + M)
    }
}

struct Outer<const N: usize>;

impl<const N: usize> Outer<N> {
    fn bare(&self) -> Twice {
        Twice { a: [N as u8, 0], b: [1, 2] }
    }
}

type Short = Buf<1>;
type Bytes<const N: usize = 2> = [u8; N];

fn total(p: &Pair) -> u16 {
    p.items.iter().sum()
}

fn make() -> Slot {
    Slot::Full([5, 6])
}

fn main() {
    let o: Oid = "abc".parse().unwrap();
    println!("{} {} {}", o.len(), o.cap(), first(&o));
    let p = Oid::new("xy").unwrap();
    println!("{:?}", p);
    let q: Oid<2> = Oid { len: 1, bytes: [7, 8] };
    println!("{} {}", q.cap(), q.bytes[1]);
    println!("{}", Oid::from_str("toolong").is_err());
    let h = Holder { buf: Buf::default(), pair: Pair::default(), slot: Slot::Empty };
    println!("{:?} {:?} {:?}", h.buf, h.pair, h.slot);
    println!("{}", total(&h.pair));
    println!("{:?}", make());
    let s: Short = Buf { data: [9] };
    println!("{:?}", s);
    let b: Buf<5> = Buf::default();
    println!("{}", b.data.len());
    let pp: Pair<u8> = Pair { items: [3, 4] };
    println!("{:?}", pp);
    let bs: Bytes = [1, 2];
    println!("{:?}", bs);
    let w: W<i32> = W::default();
    let v = W::new(5u64);
    let u: W = W::new(3);
    println!("{:?} {:?} {:?} {:?}", w, v, u, W::<u8>::default());
    let c = Cap::<5>::default();
    let d: Cap<7> = Cap::<7>::default();
    let e: Cap = Cap::default();
    println!("{} {} {} {} {}", c.cap(), d.cap(), e.cap(), Cap::<2>::default().cap(), c.used + d.used + e.used);
    let h: Cap<6> = Cap::<_>::default();
    let v: Vec<u8> = Vec::<_>::default();
    println!("{} {:?}", h.cap(), v);
    let t: Twice = Twice { a: [1, 2], b: [3, 4] };
    let tw: Twice<3> = Twice { a: [1, 2, 3], b: [4, 5, 6] };
    println!("{:?} {:?} {:?} {:?}", t.nm(), tw.nm(), Outer::<5>.bare().nm(), Outer::<5>.bare());
}
