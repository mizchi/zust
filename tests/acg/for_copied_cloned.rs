//! `for x in v.iter().copied()` / `.cloned()`: the elements are copied (cloned) out of a borrow, v stays as it is; `for x
//! in v` takes v
struct C(i32);
impl Clone for C {
    fn clone(&self) -> C {
        println!("clone {}", self.0);
        C(self.0 + 10)
    }
}

fn main() {
    // (a user's `Clone` is called for each element)
    let cs = vec![C(1), C(2)];
    let mut k = 0;
    for x in cs.iter().cloned() {
        k += x.0;
    }
    println!("{} {}", k, cs.len());
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
    // (a Vec or an array a call makes: dropped when the loop is done, by value or borrowed)
    let mk = || vec![String::from("p"), String::from("qq")];
    let mut u = 0;
    for x in mk().iter().cloned() {
        u += x.len();
    }
    for x in mk().iter() {
        u += x.len();
    }
    for x in mk() {
        u += x.len();
    }
    for x in [String::from("r")].iter().cloned() {
        u += x.len();
    }
    for x in mk().iter_mut() {
        x.push('!');
        u += x.len();
    }
    println!("{}", u);
    // (the loop's own local does not hide one of the program's)
    let __exr_for_tmp = [9];
    for _ in [1] {
        println!("{}", __exr_for_tmp[0]);
    }
}
