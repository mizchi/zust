//! Neighbors of the String edits fixed with rt_bounds: every API that takes a byte index into a str must panic at
//! a byte inside a char, never cut a char in two. (zustc has no `String::split_off` / `drain` yet.)
//@ pattern: char boundary
//@ expect: panic
//@ args: 0 1 2 3 4 5
fn main() {
    let k: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let s = String::from("añb"); // 'ñ' is bytes 1..3
    let i = 2 + (k & 0);
    match k {
        0 => println!("{}", &s[i..]),
        1 => println!("{}", &s[..i]),
        2 => println!("{}", &s.as_str()[1..i]),
        3 => {
            let mut t = s.clone();
            t.insert_str(i, "x");
            println!("{t}");
        }
        4 => println!("{:?}", s.split_at(i)),
        _ => println!("{}", s[i..].chars().count()),
    }
    println!("not reached");
}
