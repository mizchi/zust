// `Trait::eq(a, b)` of traits of the program's named as std's comparison traits (#261): their own `eq`, not the
// `==` that std's `PartialEq::eq(a, b)` is
mod own {
    pub trait Ord {
        fn eq(&self, other: &Self) -> bool;
    }
    impl Ord for u32 {
        fn eq(&self, _other: &Self) -> bool {
            false
        }
    }
}

mod mine {
    pub trait PartialEq {
        fn eq(&self, other: &Self) -> bool;
        fn ne(&self, other: &Self) -> bool;
    }
    impl PartialEq for u8 {
        fn eq(&self, _other: &Self) -> bool {
            false
        }
        fn ne(&self, _other: &Self) -> bool {
            false
        }
    }
}

fn ord() -> bool {
    use own::Ord;
    Ord::eq(&1u32, &1u32)
}

fn partial() -> (bool, bool) {
    use mine::PartialEq;
    (PartialEq::eq(&3u8, &3u8), PartialEq::ne(&3u8, &4u8))
}

fn main() {
    println!("{}", ord());
    println!("{:?}", partial());
}
