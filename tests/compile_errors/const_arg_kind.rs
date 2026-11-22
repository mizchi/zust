fn get_flag<const FLAG: bool, const NAME: char>() -> Option<char> {
    if FLAG { Some(NAME) } else { None }
}
fn main() {
    let _ = get_flag::<true, 'c'>();
    let _ = get_flag::<false, 0xFF>();
}
