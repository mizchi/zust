//! Macros (fixture).
#[macro_export]
macro_rules! vec {
    () => { $crate::vec::Vec::new() };
    ($($x:expr),+ $(,)?) => { $crate::vec::Vec::new() };
}
#[macro_export]
macro_rules! format {
    ($($arg:tt)*) => { $crate::string::String::new() };
}
