trait Local {
    type Output;
}
fn main() {
    let x: <i32 as Local>::Output = 3;
    println!("{}", x);
}
