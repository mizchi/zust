// (`self` of a list imports the module alone: a fn of its name beside it is no clash)
mod source {
    pub mod tools {}
    pub fn tools() {}
}
mod m {
    #[allow(unused_imports)]
    use crate::source::tools::{self as local};
    fn local() -> i32 {
        1
    }
    pub fn get() -> u8 {
        local()
    }
}
fn main() {}
