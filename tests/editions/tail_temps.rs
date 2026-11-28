// The temporaries of a block's tail: dropped after the block's locals before edition 2024, before them from 2024
struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
impl D {
    fn v(&self) -> i32 {
        1
    }
}
fn mk(s: &'static str) -> D {
    D(s)
}
fn a() -> i32 {
    let _l = D("a-local");
    mk("a-temp").v()
}
fn b() {
    let x = {
        let _l = D("b-local");
        mk("b-temp").v()
    };
    println!("b {x}");
}
fn c(f: bool) -> i32 {
    if f {
        let _l = D("c-local");
        mk("c-temp").v()
    } else {
        let _m = D("c-else-local");
        mk("c-else-temp").v()
    }
}
fn d() {
    for i in 0..2 {
        let _l = D("d-local");
        println!("iter {i} {}", mk("d-temp").v());
    }
}
fn e(x: Option<i32>) -> i32 {
    match x {
        Some(_) => {
            let _l = D("e-local");
            mk("e-temp").v()
        }
        None => 0,
    }
}
fn main() {
    a();
    b();
    c(true);
    c(false);
    d();
    e(Some(1));
}
