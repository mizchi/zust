// Impls of one trait with different trait arguments for one type are told apart by the argument's type: ipnet's
// `IpSub<Ipv4Addr>` (a count) and `IpSub<u32>` (an address) for Ipv4Addr, with a local, a field, a cast and an integer
// literal; rustls-pki-types' `TryFrom<&str>` and `TryFrom<&[u8]>`, through `Self::try_from` and `T::try_from`
use std::convert::TryFrom;
use std::net::Ipv4Addr;

trait IpSub<Rhs = Self> {
    type Output;
    fn saturating_sub(self, rhs: Rhs) -> Self::Output;
}

impl IpSub<Ipv4Addr> for Ipv4Addr {
    type Output = u32;
    fn saturating_sub(self, rhs: Ipv4Addr) -> u32 {
        let l: u32 = self.into();
        l.saturating_sub(rhs.into())
    }
}

impl IpSub<u32> for Ipv4Addr {
    type Output = Ipv4Addr;
    fn saturating_sub(self, rhs: u32) -> Ipv4Addr {
        let l: u32 = self.into();
        l.saturating_sub(rhs).into()
    }
}

struct Range {
    start: Ipv4Addr,
    end: Ipv4Addr,
}

impl Range {
    fn back(&self, n: usize) -> Ipv4Addr {
        let n = n as u32;
        let nth = self.end.saturating_sub(n);
        nth
    }
    fn back2(&self, n: usize) -> Ipv4Addr {
        self.end.saturating_sub(n as u32)
    }
    fn count(&self) -> u32 {
        self.end.saturating_sub(self.start) + 1
    }
}

#[derive(Debug)]
struct Name(String);

impl<'a> TryFrom<&'a str> for Name {
    type Error = ();
    fn try_from(s: &'a str) -> Result<Name, ()> {
        if s.is_empty() { Err(()) } else { Ok(Name(s.to_string())) }
    }
}

impl<'a> TryFrom<&'a [u8]> for Name {
    type Error = ();
    fn try_from(b: &'a [u8]) -> Result<Name, ()> {
        match std::str::from_utf8(b) {
            Ok(s) => Self::try_from(s),
            Err(_) => Err(()),
        }
    }
}

fn main() {
    let r = Range { start: Ipv4Addr::new(10, 0, 0, 1), end: Ipv4Addr::new(10, 0, 0, 9) };
    println!("{} {} {}", r.back(3), r.back2(1), r.count());
    let k = r.end.saturating_sub(2);
    println!("{} {}", k, r.end.saturating_sub(r.start));
    println!("{:?} {:?} {:?}", Name::try_from(&b"abc"[..]), Name::try_from("x"), Name::try_from(&[0xffu8][..]));
}
