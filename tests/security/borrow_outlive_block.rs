//! A reference that outlives the block of what it borrows (E0597).
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn main() {
    let r;
    {
        let x = String::from("short");
        r = &x; //~ ERROR E0597
    }
    println!("{r}");
}
