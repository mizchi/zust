use bitflags::bitflags;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct Perm: u32 {
        const READ = 0b001;
        const WRITE = 0b010;
        const EXEC = 0b100;
        const RW = Self::READ.bits() | Self::WRITE.bits();
    }
}

fn main() {
    let p = Perm::READ | Perm::EXEC;
    println!("{:?}", p);
    println!("bits {:#05b} contains READ {} contains RW {}", p.bits(), p.contains(Perm::READ), p.contains(Perm::RW));
    let mut q = Perm::RW;
    q.remove(Perm::WRITE);
    q.toggle(Perm::EXEC);
    println!("{:?} == {:?}: {}", q, p, q == p);
    println!("from_bits(8) = {:?}, truncate = {:?}", Perm::from_bits(8), Perm::from_bits_truncate(0b1111));
    println!("all = {:?}, empty = {:?}", Perm::all(), Perm::empty());
    let names: Vec<_> = Perm::all().iter_names().map(|(n, _)| n).collect();
    println!("names {:?}", names);
    println!("parse {:?}", bitflags::parser::from_str::<Perm>("READ | WRITE"));
    println!("done");
}
