// A fn named like a braced struct (`fn AddFlags(..) -> AddFlags`): `&AddFlags(1)` calls it when it runs, as rustc
// does. It is not a constructor promoted to a static (which would skip the fn's side effects and its result)
#![allow(non_snake_case)]
#[derive(Debug)]
struct AddFlags {
    bits: u64,
}

fn AddFlags(bits: u64) -> AddFlags {
    println!("called {bits}");
    AddFlags { bits: bits + 1 }
}

struct W(u8);

fn main() {
    let z = AddFlags(3);
    println!("{:?}", z);
    let x = &AddFlags(1);
    println!("{:?}", x);
    let n = 5;
    let w = &AddFlags(n);
    println!("{:?}", w);
    // (a tuple struct's constructor of constants is still promoted)
    let c: &'static W = &W(7);
    println!("{}", c.0);
}
