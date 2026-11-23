fn main() {
    let o = Some(1u8);
    match o {
        Option::<u8, u16>::None => {}
        _ => {}
    }
}
