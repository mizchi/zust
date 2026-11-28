// std::net's addresses as integers, as ipnet uses them: `From<u32> for Ipv4Addr` and back, `From<u128> for Ipv6Addr`
// and back, `from_bits` / `to_bits`, `From<[u16; 8]>`, the arrays into `IpAddr`, and `Ipv4Addr::from(0)` (an
// integer literal is the impl's that takes an integer)
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

fn netmask_u32(len: u8) -> u32 {
    u32::MAX.checked_shl(32 - len as u32).unwrap_or(0)
}

fn netmask_u128(len: u8) -> u128 {
    u128::MAX.checked_shl(128 - len as u32).unwrap_or(0)
}

fn main() {
    let a = Ipv4Addr::from(netmask_u32(24));
    let b: Ipv4Addr = 0x0a000001u32.into();
    let c = Ipv4Addr::from(0);
    let n: u32 = a.into();
    let m = u32::from(b);
    println!("{} {} {} {} {}", a, b, c, n, m);
    let v6 = Ipv6Addr::from(1u128);
    let w: Ipv6Addr = netmask_u128(64).into();
    let z = Ipv6Addr::from(0);
    let k: u128 = v6.into();
    println!("{} {} {} {} {}", v6, w, z, k, u128::from(w));
    println!("{} {} {}", Ipv4Addr::from_bits(0x7f000001), Ipv6Addr::from_bits(1).to_bits(), Ipv4Addr::LOCALHOST.to_bits());
    let s = Ipv6Addr::from([0u16, 0, 0, 0, 0, 0xffff, 0xc0a8, 1]);
    println!("{} {:?} {}", s, Ipv4Addr::from([10u8, 0, 0, 1]), Ipv6Addr::from(u128::MAX).to_bits() == u128::MAX);
    let ips = [IpAddr::from([192u8, 168, 0, 1]), IpAddr::from([0u8; 16]), IpAddr::from([0x2001u16, 0xdb8, 0, 0, 0, 0, 0, 1])];
    println!("{} {} {}", ips[0], ips[1], ips[2]);
}
