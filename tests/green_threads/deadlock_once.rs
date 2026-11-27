// A call_once inside its own Once's closure: std's deadlocks; zustc's panics
use std::sync::Once;

static O: Once = Once::new();

fn main() {
    O.call_once(|| {
        println!("outer");
        O.call_once(|| println!("inner"));
    });
    println!("done");
}
