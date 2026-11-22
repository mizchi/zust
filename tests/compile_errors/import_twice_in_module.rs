mod m {
    pub use self::shapes::Square;
    use self::shapes::Square;
    mod shapes {
        pub struct Square;
    }
}
fn main() {
    let _ = m::Square;
}
