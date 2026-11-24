// A program's own `size_of` is called as itself in an array length, not taken for core::mem::size_of:
// one at the crate's root, one in a module
const fn size_of() -> usize {
    7
}

mod plain {
    pub const fn size_of() -> usize {
        5
    }
    pub fn len() -> usize {
        let a = [0u8; size_of()];
        a.len()
    }
}

fn main() {
    let a = [0u8; size_of()];
    let b = [0u8; core::mem::size_of::<u32>()];
    println!("{} {} {}", a.len(), plain::len(), b.len());
}
