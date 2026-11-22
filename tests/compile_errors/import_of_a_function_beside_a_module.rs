// (a function and a module of one name: values and types, no clash)
mod tools {
    pub fn tools() -> i32 {
        1
    }
}
use tools::tools;
fn main() {
    let a: i32 = tools();
    let b: u8 = a;
}
