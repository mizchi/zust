//! A block whose value borrows one of its own locals (E0597): `{ let v = ..; let r = &v; r }`. The local is dropped
//! at the end of the block, and the reference read after it points into freed memory. The same for a guard
//! (`{ let c = RefCell::new(..); let g = c.borrow(); g }`) and through `Option::unwrap` (`{ Some(&v).unwrap() }`).
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ todo: a loan held by a local, or by a value a by-value method returns, is not followed out of the block's tail
//@ expect: reject
fn main() {
    let outer = {
        let v = vec![String::from("short")];
        let r = &v; //~ ERROR E0597
        r
    };
    println!("{}", outer[0]);
}
