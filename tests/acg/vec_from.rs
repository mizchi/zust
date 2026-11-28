// `Vec::from(x)` / `x.into()` into a Vec, for each of std's `From` impls: an array (moved in, a literal one or a
// value), a `&[T]` / `&[T; N]` / `&mut [T]` (cloned), a `&str` / String (their bytes), a Vec
struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn total(v: Vec<u64>) -> u64 {
    v.iter().sum()
}
fn main() {
    let a = Vec::from([1, 2, 3]);
    println!("{:?} {}", a, a.len());
    let arr = [String::from("x"), String::from("y")];
    let b = Vec::from(arr);
    println!("{:?}", b);
    let s = [4u8, 5, 6];
    let c = Vec::from(&s[1..]);
    let d = Vec::from(&s);
    let mut m = [7, 8];
    let e = Vec::from(&mut m[..]);
    println!("{:?} {:?} {:?}", c, d, e);
    let f = Vec::from("hi");
    let g = Vec::from(String::from("yo"));
    println!("{:?} {:?}", f, g);
    let h: Vec<u16> = [9, 10].into();
    let i: Vec<u8> = "ab".into();
    let j: Vec<u8> = String::from("cd").into();
    println!("{:?} {:?} {:?}", h, i, j);
    println!("{}", total(Vec::from([1, 2, 3])));
    println!("{}", total([4, 5].into()));
    let ds = Vec::from([D(1)]);
    println!("made {}", ds.len());
    let rep = Vec::from([0u8; 4]);
    println!("{:?}", rep);
    let v2 = Vec::from(vec![1, 2]);
    println!("{:?}", v2);
    // (a literal beside a block: of the block's type; the elements in order)
    let v3 = Vec::from([1, { 2u8 }]);
    let mut n = 1;
    let v4 = Vec::from([n, {
        n = 2;
        n
    }]);
    println!("{:?} {:?} {n}", v3, v4);
    // (a turbofish whose type has a `->`: `Vec::<fn() -> u8>::from`, an array variable)
    fn seven() -> u8 {
        7
    }
    let fa: [fn() -> u8; 2] = [seven, seven];
    let fv = Vec::<fn() -> u8>::from(fa);
    println!("{} {}", fv.len(), fv[1]());
    drop(ds);
    println!("end");
}
