fn main() {
    match Some("foo") {
        None::<isize> => {}
        Some(_) => {}
    }
    match Some(3u8) {
        Option::<u8>::None => {}
        Some::<u8>(_) => {}
    }
}
