// A statement that starts with a block-like expression and goes on with a postfix part keeps going through binary
// operators (`unsafe { .. }.wrapping_shr(n) & 1 != 0`); `get_unchecked` gives the element's reference; `impl Trait`
// nested inside an argument's type is a type parameter (unicode-ident, wasm-bindgen-shared)
static LEAF: [u8; 4] = [1, 2, 4, 0x81];

fn bit(i: usize, n: u32) -> bool {
    unsafe { LEAF.get_unchecked(i) }.wrapping_shr(n) & 1 != 0
}

fn qualified(ns: Option<&[impl std::fmt::Display]>, name: &str) -> String {
    let mut out = String::new();
    if let Some(ns) = ns {
        for s in ns {
            out.push_str(&format!("{s}__"));
        }
    }
    out.push_str(name);
    out
}

fn pairs(v: &[(impl std::fmt::Display, u32)]) -> String {
    let mut s = String::new();
    for (a, b) in v {
        s.push_str(&format!("{a}={b};"));
    }
    s
}

fn main() {
    println!("{} {} {} {}", bit(2, 2), bit(1, 0), bit(3, 7), bit(3, 1));
    let mut x = 5;
    let p = &mut x;
    { 1 };
    *p = 2;
    println!("{} {}", x, { 3u8 }.wrapping_shl(1) as u32 + 1);
    println!("{}", qualified(Some(&["a", "b"]), "f"));
    println!("{}", qualified(None::<&[&str]>, "g"));
    println!("{}", qualified(Some(&[String::from("x")]), "h"));
    println!("{}", pairs(&[("k", 1), ("j", 2)]));
}
