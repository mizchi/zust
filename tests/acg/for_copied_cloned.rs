//! `for x in v.iter().copied()` / `.cloned()`: the elements are copied (cloned) out of a borrow, v stays as it is; `for x
//! in v` takes v
fn main() {
    let v = vec![1, 2, 3];
    let mut s = 0;
    for x in v.iter().copied() {
        s += x;
    }
    println!("{} {}", s, v.len());
    let w = vec![String::from("a"), String::from("bc")];
    let mut n = 0;
    for x in w.iter().cloned() {
        n += x.len();
    }
    for (i, x) in w.iter().cloned().enumerate() {
        n += i + x.len();
    }
    println!("{} {} {}", n, w.len(), w[1]);
    let a = [String::from("x"), String::from("yz")];
    let mut m = 0;
    for x in a.iter().cloned() {
        m += x.len();
    }
    println!("{} {}", m, a[0]);
    let mut t = 0;
    for x in w {
        t += x.len();
    }
    println!("{}", t);
}
