//! CVE-2021-29922 (std, fixed in 1.53.0): `Ipv4Addr::from_str` stripped leading zeros, so "010.8.8.8" (octal 8 to
//! other parsers) was read as 10.8.8.8: an address check passed for a different host. Leading zeros are rejected
//! (the prelude's `Ipv4Addr` parse): each of these prints as rustc's build does.
//@ cve: CVE-2021-29922
//@ expect: run
use std::net::Ipv4Addr;
fn main() {
    for s in ["010.8.8.8", "1.2.3.04", "127.0.0.1", "0.0.0.0", "00.1.1.1"] {
        println!("{s} {:?}", s.parse::<Ipv4Addr>().ok());
    }
}
