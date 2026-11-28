// A call_once inside its own Once's closure: std's deadlocks, and so does zustc's
use std::sync::Once;

static O: Once = Once::new();

fn main() {
    O.call_once(|| {
        println!("outer");
        O.call_once(|| println!("inner"));
    });
    println!("done");
}
