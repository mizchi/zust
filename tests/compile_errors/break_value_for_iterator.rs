struct It;
impl Iterator for It {
    type Item = u8;
    fn next(&mut self) -> Option<u8> { None }
}
fn main() {
    for _ in It {
        break 1;
    }
}
