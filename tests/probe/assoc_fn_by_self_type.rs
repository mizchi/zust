// An associated function (no `self`) of one trait in impls for several self types: the call's self type picks the
// impl, not its first argument (the pattern of hex's `impl FromHex for [u8; N]`). Next to it, overloads that differ
// by their argument (`From<&str>` / `From<String>` for one type) still pick by the argument.

trait FromBytes: Sized {
    fn from_bytes<T: AsRef<[u8]>>(x: T) -> Result<Self, usize>;
    fn len_of() -> usize;
}

macro_rules! arrays {
    ($($n:expr)+) => {$(
        impl FromBytes for [u8; $n] {
            fn from_bytes<T: AsRef<[u8]>>(x: T) -> Result<Self, usize> {
                let s = x.as_ref();
                let mut out = [0u8; $n];
                if s.len() != out.len() {
                    return Err(s.len());
                }
                out.copy_from_slice(s);
                Ok(out)
            }
            fn len_of() -> usize {
                $n
            }
        }
    )+};
}

arrays! { 4 2 8 }

struct P(String);

impl From<&str> for P {
    fn from(s: &str) -> P {
        P(format!("str {s}"))
    }
}

impl From<String> for P {
    fn from(s: String) -> P {
        P(format!("string {s}"))
    }
}

fn main() {
    println!("{:?}", <[u8; 4]>::from_bytes([1u8, 2, 3, 4]));
    println!("{:?}", <[u8; 2]>::from_bytes([9u8, 8]));
    println!("{:?}", <[u8; 8]>::from_bytes("abcdefgh"));
    println!("{:?}", <[u8; 2]>::from_bytes("abc"));
    println!("{} {} {}", <[u8; 4]>::len_of(), <[u8; 2]>::len_of(), <[u8; 8]>::len_of());
    println!("{} / {}", P::from("a").0, P::from(String::from("b")).0);
}
