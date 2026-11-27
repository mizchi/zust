// `I::Item` / `I::IntoIter` of `I: IntoIterator` in an instance of a generic fn, as fastrand's `choice` (#313): with
// `I = &[u8]` the result is `Option<&u8>`, which the caller dereferences. std's IntoIterator impls for `&[T]`,
// `&[T; N]`, `&Vec<T>`, `&mut Vec<T>`, `[T; N]`, a Vec and an iterator (a Range) give them; `.len()` of the iterator
// is ExactSizeIterator's, the prelude's slice and Vec iterators' among them
struct Rng(u64);

impl Rng {
    fn usize(&mut self, r: std::ops::Range<usize>) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        r.start + (self.0 >> 33) as usize % (r.end - r.start)
    }

    // (fastrand's, as written there)
    fn choice<I>(&mut self, iter: I) -> Option<I::Item>
    where
        I: IntoIterator,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let len = iter.len();
        if len == 0 {
            return None;
        }
        let index = self.usize(0..len);
        iter.nth(index)
    }

    fn alphanumeric(&mut self) -> char {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        *self.choice(CHARS).unwrap() as char
    }
}

// (the free fn form, with a qualified projection)
fn first<I>(iter: I) -> Option<<I as IntoIterator>::Item>
where
    I: IntoIterator,
{
    iter.into_iter().next()
}

// (the iterator type itself, named by the projection)
fn iter_of<I: IntoIterator>(iter: I) -> I::IntoIter {
    iter.into_iter()
}

// (`.len()` through the bound alone)
fn exact_len<E: ExactSizeIterator>(e: &E) -> usize {
    e.len()
}

fn remaining<I>(iter: I) -> usize
where
    I: IntoIterator,
    I::IntoIter: ExactSizeIterator,
{
    let mut it = iter.into_iter();
    let n = it.len();
    it.next();
    n * 10 + it.len()
}

#[derive(Debug, Clone, PartialEq)]
struct P {
    x: i32,
}

fn main() {
    let mut r = Rng(7);
    let s: String = (0..12).map(|_| r.alphanumeric()).collect();
    println!("{}", s);

    // &[u8], &[T] of a struct, &Vec<T>, &[T; N], [T; N] by value, a Range, a Vec by value
    let bytes: &[u8] = b"xyz";
    let b = *r.choice(bytes).unwrap();
    println!("{}", b as char);
    let ps = vec![P { x: 1 }, P { x: 2 }, P { x: 3 }];
    let p: &P = r.choice(&ps).unwrap();
    println!("{:?}", p);
    let q: &P = r.choice(&ps[1..]).unwrap();
    println!("{}", q.x);
    let arr = [10u16, 20, 30, 40];
    let a: u16 = *r.choice(&arr).unwrap();
    println!("{}", a);
    let a2: u16 = r.choice(arr).unwrap();
    println!("{}", a2);
    let n: u32 = r.choice(5u32..9).unwrap();
    println!("{}", n);
    let owned: P = r.choice(ps.clone()).unwrap();
    println!("{:?}", owned);
    let words = vec!["a", "bb", "ccc"];
    let w: &&str = r.choice(&words).unwrap();
    println!("{}", w.len());
    let empty: Vec<i64> = Vec::new();
    println!("{:?}", r.choice(&empty));
    println!("{:?}", r.choice(0..0));

    // &mut Vec<T>: the items are `&mut T`
    let mut ms = vec![1i32, 2, 3];
    if let Some(m) = r.choice(&mut ms) {
        *m += 100;
    }
    println!("{:?}", ms);

    println!("{:?} {:?} {:?}", first(&arr), first(bytes).copied(), first(&ps).map(|p| p.x));
    println!("{:?}", first(vec![String::from("s")]));
    // (`&mut [T; N]` / `&mut [T]`: `&mut T`)
    let mut ar = [1u8, 2, 3];
    if let Some(x) = first(&mut ar) {
        *x += 10;
    }
    let sl: &mut [u8] = &mut ar[1..];
    if let Some(x) = first(sl) {
        *x += 20;
    }
    println!("{:?}", ar);
    // (the projection written with a concrete type)
    let qi: <&[u16; 4] as IntoIterator>::Item = &arr[3];
    let qv: <Vec<u8> as IntoIterator>::Item = 7;
    println!("{} {}", qi, qv);

    let mut it = iter_of(&ps);
    it.next();
    println!("{} {:?}", it.len(), it.next());
    let it2 = iter_of(bytes);
    println!("{}", it2.len());

    // ExactSizeIterator's len on the slice / Vec iterators, through a bound
    let mut v = vec![1u8, 2, 3, 4, 5];
    println!("{}", exact_len(&v.iter()));
    println!("{}", exact_len(&v.iter_mut()));
    println!("{}", exact_len(&bytes.iter()));
    println!("{}", exact_len(&arr.iter()));
    println!("{}", exact_len(&v.clone().into_iter()));
    println!("{}", exact_len(&(0..7)));
    println!("{} {} {}", exact_len(&(-128i8..127)), exact_len(&(-3i32..4)), exact_len(&(9u16..2)));
    println!("{} {} {}", remaining(&v), remaining(bytes), remaining(0..3));
    println!("{}", remaining(&mut v));
}
