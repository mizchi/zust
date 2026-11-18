//! Macros (fixture).
#[macro_export]
macro_rules! assert {
    ($cond:expr $(,)?) => {{ /* compiler built-in */ }};
}
#[macro_export]
macro_rules! assert_eq {
    ($left:expr, $right:expr $(,)?) => {{ /* compiler built-in */ }};
}
/// Attribute macro applied to a function to turn it into a unit test.
pub macro test($item:item) {
    /* compiler built-in */
}
/// Attribute macro used to apply derive macros.
pub macro derive($item:item) {
    /* compiler built-in */
}
