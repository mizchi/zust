// std's `impl Extend<&T> for Vec<T>` needs `T: Copy`: a `Vec<String>` is no `Extend<&String>`
fn add<'a, E: Extend<&'a String>>(e: &mut E, s: &'a String) {
    e.extend([s]);
}

fn main() {
    let s = String::from("x");
    let mut v: Vec<String> = Vec::new();
    add(&mut v, &s);
    println!("{}", v.len());
}
