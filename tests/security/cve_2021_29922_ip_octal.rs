//! CVE-2021-29922 (std, fixed in 1.53.0): `Ipv4Addr::from_str` stripped leading zeros, so "010.8.8.8" (octal 8 to
//! other parsers) was read as 10.8.8.8: an address check passed for a different host. Leading zeros are rejected.
//! Not applicable yet: zustc has no `std::net`, so the build is rejected; when it builds, this case fails
//! and must become a test of the behavior above.
//@ cve: CVE-2021-29922
//@ expect: unsupported
use std::net::Ipv4Addr;
fn main() {
    for s in ["010.8.8.8", "1.2.3.04", "127.0.0.1", "0.0.0.0", "00.1.1.1"] {
        println!("{s} {:?}", s.parse::<Ipv4Addr>().ok());
    }
}
