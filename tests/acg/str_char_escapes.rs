// std's str / char breadth: split_ascii_whitespace (not U+000B, which split_whitespace takes), Debug's `\u{..}` for
// control characters, char's escape_unicode / escape_default, and a String helper on a `&'static str` static
static NAME: &'static str = "Hello World";
fn main() {
    let s = " a\x0Bb  c\td ";
    let v: Vec<&str> = s.split_ascii_whitespace().collect();
    let w: Vec<&str> = s.split_whitespace().collect();
    let r: Vec<&str> = s.split_ascii_whitespace().rev().collect();
    println!("{:?} {:?} {:?}", v, w, r);
    println!("{:?} {:?} {:?}", "a\x0Bb\x01\x7f\u{80}", '\x0B', "tab\tnl\n\x1b");
    let e: String = '\u{10401}'.escape_unicode().collect();
    println!("{} {} {}", e, 'a'.escape_unicode(), '\n'.escape_default());
    for c in ['x', '"', '\x07', '\u{e9}'] {
        print!("[{}]", c.escape_default());
    }
    println!(" {}", '\\'.escape_default().count());
    match &*NAME.to_ascii_lowercase() {
        "hello world" => println!("lower"),
        _ => println!("other"),
    }
}
