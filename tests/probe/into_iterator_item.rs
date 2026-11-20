// A type parameter given only by `I: IntoIterator<Item = A>`, with a Vec or a HashMap argument: the prelude's
// collections have an `into_iter` method, and the Item is that of the iterator it returns
use std::collections::HashMap;

fn first<A, I: IntoIterator<Item = A>>(a: I) -> Option<A> {
    a.into_iter().next()
}

fn pairwise<A, B, I, J>(a: I, b: J) -> Vec<(A, B)>
where
    I: IntoIterator<Item = A>,
    J: IntoIterator<Item = B>,
{
    a.into_iter().zip(b).collect()
}

fn total<K, I: IntoIterator<Item = (K, u64)>>(a: I) -> u64 {
    a.into_iter().map(|(_, v)| v).sum()
}

fn main() {
    println!("{:?}", first(vec![3u32, 4]));
    println!("{:?}", pairwise(vec![1u8, 2, 3], vec![String::from("a"), String::from("b")]));
    let mut m = HashMap::new();
    m.insert("x", 5u64);
    m.insert("y", 7);
    println!("{}", total(m));
}
