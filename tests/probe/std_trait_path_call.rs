// A trait's method called through its std path, `std::<module>::<Trait>::<method>(..)` (#153: each was "cannot find
// value (unsupported path)"), as its two-segment form `Trait::method(..)` is: fmt::Write and io::Write, Clone,
// Iterator (a closure argument, a turbofish), ToString, Ord / PartialOrd, Add; `core::` and `::std::` too. std's
// functions keep their own paths: `std::mem::swap` beside a fn of the program named `swap`
fn swap(x: u8) -> u8 {
    x + 1
}

fn main() {
    let mut s = String::new();
    std::fmt::Write::write_str(&mut s, "a").unwrap();
    std::fmt::Write::write_char(&mut s, 'b').unwrap();
    core::fmt::Write::write_str(&mut s, "c").unwrap();
    println!("{s}");
    let mut v: Vec<u8> = Vec::new();
    std::io::Write::write_all(&mut v, b"xy").unwrap();
    println!("{v:?}");
    let n = std::clone::Clone::clone(&5u8);
    let t = core::clone::Clone::clone(&String::from("t"));
    let u = ::std::clone::Clone::clone(&vec![1, 2]);
    println!("{n} {t} {u:?}");
    let w = vec![3, 1, 2];
    println!("{}", std::iter::Iterator::count(w.iter()));
    println!("{}", std::iter::Iterator::sum::<i32>(w.iter()));
    let d: Vec<i32> = std::iter::Iterator::map(w.iter(), |x| x * 10).collect();
    println!("{d:?}");
    println!("{}", std::string::ToString::to_string(&42));
    println!("{:?} {:?}", std::cmp::Ord::cmp(&1, &2), std::cmp::PartialOrd::partial_cmp(&2.5, &1.0));
    println!("{}", std::ops::Add::add(40, 2));
    let (mut a, mut b) = (1, 2);
    std::mem::swap(&mut a, &mut b);
    println!("{a} {b} {}", swap(7));
}
