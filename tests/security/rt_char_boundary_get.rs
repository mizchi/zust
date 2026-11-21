//! The checked forms answer None at a byte inside a char (and do not panic). zustc has no `str::get` yet: when it
//! gets one, this case fails and must become `expect: run`.
//@ pattern: char boundary
//@ expect: unsupported
fn main() {
    let s = String::from("añb€");
    let n = s.len();
    for a in 0..=n {
        for b in a..=n {
            if let Some(t) = s.get(a..b) {
                print!("[{a},{b})={t} ");
            }
        }
    }
    println!();
    println!("{:?} {:?} {:?}", s.get(2..), s.get(..2), s.get(1..3));
    println!("{} {} {}", s.is_char_boundary(2), s.is_char_boundary(3), s.is_char_boundary(100));
}
