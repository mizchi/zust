// std's str / char breadth: split_ascii_whitespace (not U+000B, which split_whitespace takes), char's escape_unicode /
// escape_default, and a String helper on a `&'static str` static
static NAME: &'static str = "Hello World";
fn main() {
    let s = " a\x0Bb  c\td ";
    let v: Vec<&str> = s.split_ascii_whitespace().collect();
    let w: Vec<&str> = s.split_whitespace().collect();
    let r: Vec<&str> = s.split_ascii_whitespace().rev().collect();
    println!("{} {} {} {}", v.len(), w.len(), r.len(), v[0].len());
    println!("{:?} {:?}", w, &r[..2]);
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
