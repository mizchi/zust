// More of what wasm-bindgen 0.2.130's `convert/slices.rs` needs (W1 of #38): a generic method called right in a
// `match` (its type parameter from the arms' type), `impl<T: Tr + ?Sized> Tr for &T` with functions without `self`,
// a blanket impl's `self`-by-value method on a number, on a reference to a number or a struct and through a
// qualified path (with a reference's own impl before it), and `String::from_utf8_unchecked`.

// (a generic method in a `match`)
pub struct W {
    n: usize,
}
impl W {
    pub fn dec<T: Default>(self) -> Result<Vec<T>, &'static str> {
        if self.n > 10 {
            return Err("too long");
        }
        Ok((0..self.n).map(|_| T::default()).collect())
    }
}
fn decode(js: W) -> Vec<bool> {
    match js.dec() {
        Ok(d) => d,
        Err(_) => Vec::new(),
    }
}

// (`impl<T: Tr + ?Sized> Tr for &T`'s functions without `self`)
pub trait Describe {
    fn describe() -> u32;
}
impl Describe for u8 {
    fn describe() -> u32 {
        1
    }
}
impl Describe for str {
    fn describe() -> u32 {
        2
    }
}
impl<T: Describe + ?Sized> Describe for &T {
    fn describe() -> u32 {
        10 + T::describe()
    }
}
impl<T: Describe + ?Sized> Describe for &mut T {
    fn describe() -> u32 {
        20 + T::describe()
    }
}
fn describe_of<T: Describe>() -> u32 {
    T::describe()
}

// (a blanket impl beside the impls for a reference and for a number of their own)
pub trait IntoAbi {
    fn into_abi(self) -> u32;
}
pub trait OptionIntoAbi: IntoAbi {
    fn none() -> u32;
}
pub trait MaybeIntoAbi {
    fn into_maybe_abi(self) -> u32;
    fn none() -> u32;
}
impl<T: OptionIntoAbi> MaybeIntoAbi for T {
    fn into_maybe_abi(self) -> u32 {
        self.into_abi() + 1
    }
    fn none() -> u32 {
        <T as OptionIntoAbi>::none() + 1
    }
}
impl IntoAbi for u8 {
    fn into_abi(self) -> u32 {
        self as u32
    }
}
impl OptionIntoAbi for u8 {
    fn none() -> u32 {
        0xFF
    }
}
impl MaybeIntoAbi for &u8 {
    fn into_maybe_abi(self) -> u32 {
        (*self).into_maybe_abi() + 1000
    }
    fn none() -> u32 {
        <u8 as MaybeIntoAbi>::none() + 1000
    }
}
// (a number with its own impl, no OptionIntoAbi: the blanket impl is not its)
impl MaybeIntoAbi for u32 {
    fn into_maybe_abi(self) -> u32 {
        self * 2
    }
    fn none() -> u32 {
        7
    }
}
impl MaybeIntoAbi for &u32 {
    fn into_maybe_abi(self) -> u32 {
        (*self).into_maybe_abi() + 3
    }
    fn none() -> u32 {
        <u32 as MaybeIntoAbi>::none()
    }
}
fn opt<T: MaybeIntoAbi>(x: Option<T>) -> u32 {
    match x {
        Some(x) => x.into_maybe_abi(),
        None => T::none(),
    }
}
pub struct S(String);
impl IntoAbi for S {
    fn into_abi(self) -> u32 {
        self.0.len() as u32
    }
}
impl OptionIntoAbi for S {
    fn none() -> u32 {
        0
    }
}
impl IntoAbi for &S {
    fn into_abi(self) -> u32 {
        self.0.len() as u32 * 10
    }
}
impl OptionIntoAbi for &S {
    fn none() -> u32 {
        50
    }
}
impl MaybeIntoAbi for &&S {
    fn into_maybe_abi(self) -> u32 {
        (*self).into_maybe_abi() + 100
    }
    fn none() -> u32 {
        <&S as MaybeIntoAbi>::none() + 100
    }
}

// (`&str`'s own impl beside a blanket one)
pub trait JsCast {}
pub trait StrAbi {
    fn str_abi(self) -> u32;
}
impl<T: JsCast> StrAbi for T {
    fn str_abi(self) -> u32 {
        7
    }
}
impl<'a> StrAbi for &'a str {
    fn str_abi(self) -> u32 {
        self.len() as u32
    }
}
impl StrAbi for &&str {
    fn str_abi(self) -> u32 {
        (*self).str_abi() + 100
    }
}

fn main() {
    println!("{:?} {:?}", decode(W { n: 2 }), decode(W { n: 20 }));
    println!("{} {} {}", <&u8 as Describe>::describe(), <&str as Describe>::describe(), <&mut u8 as Describe>::describe());
    println!("{} {} {}", <&&u8 as Describe>::describe(), describe_of::<&u8>(), describe_of::<&mut u8>());
    let a = 7u8;
    let b = 9u32;
    println!("{} {} {} {}", opt(Some(5u8)), opt::<u8>(None), opt(Some(&a)), opt::<&u8>(None));
    println!("{} {} {} {}", opt(Some(b)), opt::<u32>(None), opt(Some(&b)), opt::<&u32>(None));
    let s = S("abc".to_string());
    let r = &s;
    println!("{} {} {} {}", (&r).into_maybe_abi(), r.into_maybe_abi(), <&S as MaybeIntoAbi>::none(), <&&S as MaybeIntoAbi>::none());
    println!("{}", s.into_maybe_abi());
    let t = "abcd";
    println!("{} {}", t.str_abi(), (&t).str_abi());
    let u = unsafe { String::from_utf8_unchecked(vec![104u8, 105]) };
    println!("{} {}", u, u.len());
}
