fn main() { let x = 5u16; let y = <u16 as From<u8>>::from(x); println!("{y}"); }
