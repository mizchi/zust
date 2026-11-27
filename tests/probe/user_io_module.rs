// A program's own `io::Error` (#256, #280's review): `io::Error::last_os_error()` / `from_raw_os_error(..)` are its
// own fns, not std's io::Error's
mod io {
    pub struct Error(pub i32);
    impl Error {
        pub fn last_os_error() -> Error {
            Error(77)
        }
        pub fn from_raw_os_error(c: i32) -> Error {
            Error(c + 1)
        }
    }
}

fn main() {
    let e = io::Error::last_os_error();
    let g = io::Error::from_raw_os_error(5);
    println!("{} {}", e.0, g.0);
}
