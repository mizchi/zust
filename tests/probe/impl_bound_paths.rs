// An impl's method bounds are compared with its trait's by the trait each path names, not by the path's text (#264):
// writeable 0.6.4's macro writes `core::fmt::Write + ?Sized` for the trait's `fmt::Write + ?Sized`, and bitflags
// 2.13.2's `impl_bits!` writes `fmt::Write`; here also `std::fmt::Write`, an imported `Write`, `::core::fmt::Write`,
// a `$crate::` path, and bounds in a where clause for the trait's inline ones (and the other way). And a parameter
// that is `?Sized` in the trait and Sized in the impl, where the trait's bound is of a trait that is Sized by its where
// clause's `Self: Sized`: no stricter (rustc 1.97)
use core::fmt;
use std::fmt::Write as FmtWrite;

pub mod sink {
    pub trait Sink {
        fn put(&mut self, s: &str);
    }
    impl Sink for String {
        fn put(&mut self, s: &str) {
            self.push_str(s);
        }
    }
}

pub trait Writeable {
    fn write_to<W: fmt::Write + ?Sized>(&self, sink: &mut W) -> fmt::Result;
    fn write_where<W>(&self, sink: &mut W) -> fmt::Result
    where
        W: fmt::Write + ?Sized;
    fn put_to<S: sink::Sink>(&self, s: &mut S);
}

pub trait WriteHex {
    fn write_hex<W: fmt::Write>(&self, writer: W) -> fmt::Result;
}

// (Sized as a supertrait by a where clause: `T: Plain + ?Sized` is Sized all the same)
pub trait Plain: Send
where
    Self: Sized,
{
    fn weight(&self) -> u8;
}

impl Plain for u8 {
    fn weight(&self) -> u8 {
        *self
    }
}

pub trait Count {
    fn count<T: Plain + ?Sized>(&self, t: &T) -> u8;
}

macro_rules! impl_write_num {
    ($($u:ty),*) => {
        $(
            impl $crate::Writeable for $u {
                fn write_to<W: core::fmt::Write + ?Sized>(&self, sink: &mut W) -> core::fmt::Result {
                    write!(sink, "{}", self)
                }
                fn write_where<W: ?Sized>(&self, sink: &mut W) -> core::fmt::Result
                where
                    W: ::core::fmt::Write,
                {
                    write!(sink, "<{}>", self)
                }
                fn put_to<S: $crate::sink::Sink>(&self, s: &mut S) {
                    s.put("n");
                }
            }
            impl WriteHex for $u {
                fn write_hex<W: fmt::Write>(&self, mut writer: W) -> fmt::Result {
                    write!(writer, "{:x}", self)
                }
            }
            impl $crate::Count for $u {
                fn count<T: $crate::Plain>(&self, t: &T) -> u8 {
                    t.weight() + 2
                }
            }
        )*
    };
}

impl_write_num!(u8, i16, u32);

pub struct Name(&'static str);

impl Writeable for Name {
    fn write_to<W>(&self, sink: &mut W) -> std::fmt::Result
    where
        W: std::fmt::Write + ?Sized,
    {
        sink.write_str(self.0)
    }
    fn write_where<W: ?Sized + FmtWrite>(&self, sink: &mut W) -> fmt::Result {
        sink.write_str("[")?;
        sink.write_str(self.0)?;
        sink.write_str("]")
    }
    fn put_to<S>(&self, s: &mut S)
    where
        S: crate::sink::Sink,
    {
        s.put(self.0);
    }
}

impl WriteHex for Name {
    fn write_hex<W: core::fmt::Write>(&self, mut writer: W) -> fmt::Result {
        for b in self.0.bytes() {
            write!(writer, "{:02x}", b)?;
        }
        Ok(())
    }
}

impl Count for Name {
    fn count<T: Plain>(&self, t: &T) -> u8 {
        t.weight() + self.0.len() as u8
    }
}

// (a writer by value: it prints what it is given)
struct Out;

impl fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        print!("{s}");
        Ok(())
    }
}

fn main() {
    let mut s = String::new();
    7u8.write_to(&mut s).unwrap();
    s.push(' ');
    (-3i16).write_where(&mut s).unwrap();
    s.push(' ');
    Name("ab").write_to(&mut s).unwrap();
    Name("cd").write_where(&mut s).unwrap();
    42u32.put_to(&mut s);
    Name("!").put_to(&mut s);
    println!("{s}");
    255u8.write_hex(Out).unwrap();
    print!(" ");
    4096u32.write_hex(Out).unwrap();
    print!(" ");
    Name("hi").write_hex(Out).unwrap();
    println!();
    println!("{} {}", Name("xyz").count(&3u8), 9u32.count(&4u8));
}
