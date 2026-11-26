// Temporaries: `let _ = &t();` extends t()'s as a named let does, also through a field, an index or a block's tail;
// `let ref x = f(&t());` keeps only the value bound; an indexed temporary (`[A(1)][0]`, `f()[1]`) is
// dropped at the statement's end
struct A(u32);
impl Drop for A {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
struct B<T> {
    f: T,
}

fn id<T>(x: T) -> T {
    x
}
fn extension() {
    {
        let _x = &((B { f: A(1) }).f);
        println!("end1");
    }
    {
        let _ = &((B { f: A(2) }).f);
        println!("end2");
    }
    {
        let _ = &(([A(3)])[0]);
        println!("end3");
    }
    {
        let _ = &A(4);
        println!("end4");
    }
    {
        let _x = { { &A(5) } };
        println!("end5");
    }
    {
        let _ = { { &A(6) } };
        println!("end6");
    }
    {
        let ref _x = id(&A(7)).0;
        println!("end7");
    }
}
fn arr() -> [A; 2] {
    [A(10), A(11)]
}
fn indexed() {
    let n = [A(1), A(2)][0].0;
    println!("n {n}");
    let m = arr()[1].0;
    println!("m {m}");
    let b = Box::new([A(20)])[0].0;
    println!("b {b}");
    for i in 0..2 {
        let k = [A(40 + i), A(50)][0].0;
        println!("k {k}");
    }
    let r = &[A(60)][0];
    println!("r {}", r.0);
    println!("end");
}
fn main() {
    extension();
    indexed();
}
