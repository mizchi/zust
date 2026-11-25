// A block's value that is a `Copy` aggregate read out of what the block drops: copied out before the drops, as rustc
// evaluates the tail first. Also its deref coercion to the type expected, and `return v[0];`
fn f1() -> (i64, i64) { let v = vec![(1, 2)]; v[0] }
fn f2() -> (i64, i64) { let b = Box::new((3, 4)); *b }
fn f3() -> (i64, i64) { let b = Box::new(((5, 6), 1)); b.0 }
fn f4() -> (i64, i64) { let v = vec![((7, 8), 1)]; v[0].0 }
fn f5() -> (i64, i64) { let v = vec![(9, 10)]; if v.len() > 0 { v[0] } else { (0, 0) } }
fn f6() -> (i64, i64) { let v = vec![(11, 12)]; return v[0]; }
fn f7() -> (i64, i64) { let r = { let v = vec![(13, 14)]; v[0] }; r }
fn f8() -> [u8; 3] { let v = vec![[1u8, 2, 3]]; v[0] }
fn g1(s: &String) -> &str { let v = vec![s.as_str()]; v[0] }
fn g2<'a>(s: &'a String) -> &'a str { let parts: Vec<&str> = s.split('|').collect(); let c: &&str = &parts[1]; c }
fn g3<'a>(s: &'a String) -> &'a str { let parts: Vec<_> = s.split('|').collect(); let c = &parts[0]; c }
fn main() {
    println!("{:?} {:?} {:?} {:?}", f1(), f2(), f3(), f4());
    println!("{:?} {:?} {:?} {:?}", f5(), f6(), f7(), f8());
    let s = "ab|cd".to_string();
    println!("{} {} {}", g1(&s), g2(&s), g3(&s));
}
