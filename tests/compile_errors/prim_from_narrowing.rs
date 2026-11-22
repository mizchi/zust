fn main() {
    let a = u16::from(3u8);
    let b = <u16 as From<u32>>::from(5u32);
    println!("{a} {b}");
}
