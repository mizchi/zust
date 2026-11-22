const N: usize = {
    let _x = 1 / 0;
    1
};
fn main() {
    let a = [0u8; N];
    println!("{}", a.len());
}
