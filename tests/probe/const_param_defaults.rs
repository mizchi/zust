// A type whose const parameter has a default, used without arguments (#260: const-oid's `ObjectIdentifier<const
// MAX_SIZE: usize = DEFAULT_MAX_SIZE>`): in an impl's self type, a trait impl's, a signature, a field, a `let`, an
// alias; a default that names a const, a literal one, one after a type parameter's default, and on an enum. In an
// expression's path the arguments are inferred, not the defaults (`let b: Buf<5> = Buf::default();`)
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
}
