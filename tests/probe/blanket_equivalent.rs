// equivalent 1.0.3's traits (#261): a generic trait's blanket impl `impl<Q, K> Equivalent<K> for Q where Q: Eq, K:
// Borrow<Q>`, and a trait whose supertrait it is (`Comparable<K>: Equivalent<K>`), implemented for every `Q: Ord`.
// That impl needs `Q: Equivalent<K>`, which the blanket impl gives under the impl's own where clauses (`Q: Ord` is
// `Q: Eq`); a lookup uses it through a bound, `q.equivalent(k)` of `Q: Equivalent<K>`
use std::borrow::Borrow;
use std::cmp::Ordering;

pub trait Equivalent<K: ?Sized> {
    fn equivalent(&self, key: &K) -> bool;
}

impl<Q: ?Sized, K: ?Sized> Equivalent<K> for Q
where
    Q: Eq,
    K: Borrow<Q>,
{
    fn equivalent(&self, key: &K) -> bool {
        PartialEq::eq(self, key.borrow())
    }
}

pub trait Comparable<K: ?Sized>: Equivalent<K> {
    fn compare(&self, key: &K) -> Ordering;
}

impl<Q: ?Sized, K: ?Sized> Comparable<K> for Q
where
    Q: Ord,
    K: Borrow<Q>,
{
    fn compare(&self, key: &K) -> Ordering {
        Ord::cmp(self, key.borrow())
    }
}

fn find<K, Q: ?Sized + Equivalent<K>>(keys: &[K], q: &Q) -> Option<usize> {
    for (i, k) in keys.iter().enumerate() {
        if q.equivalent(k) {
            return Some(i);
        }
    }
    None
}

fn order<K, Q: ?Sized + Comparable<K>>(k: &K, q: &Q) -> Ordering {
    q.compare(k)
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Id(u32);

fn main() {
    let keys = vec![String::from("a"), String::from("b"), String::from("c")];
    println!("{:?}", find(&keys, "b"));
    println!("{:?}", find(&keys, "z"));
    println!("{} {}", Id(3).equivalent(&Id(3)), Id(3).equivalent(&Id(4)));
    println!("{}", 3u32.equivalent(&3u32));
    println!("{:?}", order(&String::from("m"), "a"));
    println!("{:?}", order(&5i64, &7i64));
    println!("{:?}", order(&Id(2), &Id(1)));
    let nums = [4u8, 8, 15];
    println!("{:?}", find(&nums, &15u8));
}
