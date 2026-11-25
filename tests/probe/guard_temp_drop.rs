// A guard's temporaries are dropped before its arm runs (rustc's terminating scope), and a temporary receiver that a
// method takes by value holds nothing of what it borrowed after the call
struct D<'a>(&'a i32);
impl Drop for D<'_> {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
impl D<'_> {
    fn ok(&self) -> bool {
        *self.0 > 0
    }
    fn consume(self) -> i32 {
        *self.0 * 10
    }
}
fn main() {
    let mut x = 1;
    match () {
        _ if D(&x).ok() => {
            x = 2;
        }
        _ => {}
    }
    println!("{}", x);
    let t = (D(&x).consume(), {
        x = 3;
    });
    println!("{} {:?} {}", t.0, t.1, x);
}
