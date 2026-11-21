//! Calling a private function from outside its module (E0603): it may rely on invariants only its
//! module upholds.
//@ rule: privacy (a module's invariants are its own to keep)
//@ expect: reject
mod m {
    fn unchecked_get(v: &[u8], i: usize) -> u8 {
        unsafe { *v.as_ptr().add(i) }
    }
    pub fn get(v: &[u8], i: usize) -> u8 {
        if i < v.len() { unchecked_get(v, i) } else { 0 }
    }
}
fn main() {
    let v = vec![1u8];
    println!("{}", m::get(&v, 0));
    println!("{}", m::unchecked_get(&v, 100)); //~ ERROR E0603
}
