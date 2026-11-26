// Option<Option<T>>::flatten, now a prelude method (#142: it was "no method named `unwrap_or` found", from a lowering
// that wrote `o.unwrap_or(None)`), chained, and beside an iterator's flatten
fn main() {
    let x: Option<Option<u32>> = Some(Some(3));
    println!("{:?} {:?}", x.flatten(), Some(None::<u8>).flatten());
    let n: Option<Option<String>> = None;
    println!("{:?}", n.flatten());
    let s: Option<Option<String>> = Some(Some("ab".to_string()));
    println!("{:?}", s.flatten().map(|t| t.len()));
    let deep: Option<Option<Option<i32>>> = Some(Some(Some(5)));
    println!("{:?}", deep.flatten().flatten());
    let d1 = deep.flatten();
    println!("{:?}", d1.flatten());
    let b: Option<Option<u8>> = Some(Some(1));
    println!("{:?}", b.clone().flatten().map(|v| v + 1));
    println!("{:?}", Some(b).flatten().flatten());
    let v = vec![Some(1), None, Some(3)];
    println!("{:?}", v.into_iter().flatten().collect::<Vec<_>>());
}
