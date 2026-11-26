// Literals at the edges of the lexer: unterminated, out of range, odd prefixes and suffixes.
fn main() {
    let a = 1e309;
    let b = 0b2;
    let c = '\u{110000}';
    let d = 255u8 + 1_000u8;
    let e = r#"raw "# ;
    let f = b'
    let g = "unterminated
}
