mod outer {
    pub mod inner {
        pub fn f() {}
    }
}
use outer::inner;
mod inner {}
fn main() {
    inner::f();
}
