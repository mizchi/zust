const N: usize = {
    let x = 1 / 0;
    if false { x } else { 1 }
};
fn main() {
    let a = [0u8; N];
    println!("{}", a.len());
}
