// Impls of a trait for T, &T and &&T side by side: each is its own method (they used to share one symbol), and a
// call on a reference picks the impl rustc picks (a by-value self equal to the receiver first)
trait Abi {
    fn abi(self) -> u32;
    fn name(&self) -> &'static str {
        "d"
    }
}
impl Abi for &str {
    fn abi(self) -> u32 {
        self.len() as u32
    }
    fn name(&self) -> &'static str {
        "one"
    }
}
impl Abi for &&str {
    fn abi(self) -> u32 {
        (*self).abi() + 100
    }
    fn name(&self) -> &'static str {
        "two"
    }
}
impl Abi for &u8 {
    fn abi(self) -> u32 {
        *self as u32
    }
}
impl Abi for &&u8 {
    fn abi(self) -> u32 {
        (*self).abi() * 2
    }
}
struct S(u32);
impl Abi for &S {
    fn abi(self) -> u32 {
        self.0
    }
}
impl Abi for &&S {
    fn abi(self) -> u32 {
        self.0 + 1000
    }
}

trait Tr {
    fn v(self) -> u32;
}
struct T(u32);
impl Tr for T {
    fn v(self) -> u32 {
        self.0
    }
}
impl Tr for &T {
    fn v(self) -> u32 {
        self.0 + 100
    }
}
impl Tr for u8 {
    fn v(self) -> u32 {
        self as u32
    }
}
impl Tr for &u8 {
    fn v(self) -> u32 {
        *self as u32 + 100
    }
}

fn main() {
    let s = "four";
    let x = 21u8;
    let v = S(7);
    println!("{} {} {} {}", s.abi(), (&s).abi(), s.name(), (&s).name());
    println!("{} {}", (&x).abi(), (&&x).abi());
    println!("{} {}", (&v).abi(), (&&v).abi());
    let a = T(1);
    println!("{}", (&a).v());
    println!("{}", a.v());
    let y = 5u8;
    println!("{} {}", (&y).v(), y.v());
}
