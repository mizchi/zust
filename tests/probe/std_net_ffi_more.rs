// std items serde_core names: std::net's addresses (Display and parse), CString / CStr, LinkedList, RwLock,
// SystemTime / UNIX_EPOCH, Bound, fmt::Arguments as a type; a program's own FromStr; `Display::fmt(self, f)` in a
// Debug impl
use std::collections::LinkedList;
use std::ffi::{CStr, CString};
use std::fmt::{self, Debug, Display, Formatter};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::ops::Bound;
use std::str::FromStr;
use std::sync::RwLock;
use std::time::{Duration, UNIX_EPOCH};

#[derive(Debug)]
struct Celsius(i32);
impl FromStr for Celsius {
    type Err = String;
    fn from_str(s: &str) -> Result<Celsius, String> {
        match s.strip_suffix("C") {
            Some(n) => n.parse::<i32>().map(Celsius).map_err(|e| e.to_string()),
            None => Err(format!("no unit in {s:?}")),
        }
    }
}

struct Pt(u8, u8);
impl Display for Pt {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.0, self.1)
    }
}
impl Debug for Pt {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        Display::fmt(self, f)
    }
}

fn kind(b: Bound<&u8>) -> String {
    match b {
        Bound::Included(x) => format!("[{x}"),
        Bound::Excluded(x) => format!("({x}"),
        Bound::Unbounded => "..".to_string(),
    }
}

fn main() {
    // addresses
    let v4 = Ipv4Addr::new(192, 168, 0, 1);
    let v6 = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
    println!("{} {} {:?} {}", v4, v6, v4.octets(), Ipv6Addr::new(1, 0, 0, 2, 0, 0, 0, 3));
    for s in ["10.0.0.1", "::1", "fe80::1:2", "::ffff:1.2.3.4", "1.2.3", "01.2.3.4", "1::2::3", "gg::1"] {
        println!("{s}: {:?}", s.parse::<IpAddr>().ok());
    }
    let sa: SocketAddr = "127.0.0.1:8080".parse().unwrap();
    let sb: SocketAddr = "[2001:db8::7]:443".parse().unwrap();
    println!("{} {} {} {}", sa, sb, sa.port(), sb.is_ipv6());
    println!("{} {}", SocketAddrV4::new(v4, 1), SocketAddrV6::new(v6, 2, 0, 0));
    println!("{:?} {:?}", "1.2.3.4:99999".parse::<SocketAddr>().is_err(), IpAddr::from(v4).is_ipv4());
    // C strings
    let c = CString::new("hi").unwrap();
    let bad = CString::new(vec![b'a', 0, b'b']);
    println!("{:?} {:?} {}", c.as_bytes(), bad.as_ref().map_err(|e| e.nul_position()), bad.is_err());
    let r: &CStr = c.as_c_str();
    println!("{:?} {:?}", r.to_bytes(), r.to_str());
    // LinkedList
    let mut l = LinkedList::new();
    l.push_back(2);
    l.push_back(3);
    l.push_front(1);
    println!("{} {:?} {:?} {:?}", l.len(), l.front(), l.back(), l.iter().collect::<Vec<_>>());
    println!("{:?} {:?}", l.pop_front(), l);
    // (a clone has a buffer of its own: both are dropped)
    let mut names = LinkedList::new();
    names.push_back(String::from("a"));
    let mut copy = names.clone();
    copy.push_back(String::from("b"));
    names.pop_front();
    let dq: std::collections::VecDeque<String> = copy.iter().cloned().collect();
    let dq2 = dq.clone();
    drop(dq);
    println!("{:?} {:?} {:?}", names, copy, dq2);
    // RwLock
    let lock = RwLock::new(vec![1, 2]);
    {
        let a = lock.read().unwrap();
        let b = lock.read().unwrap();
        println!("{} {}", a.len(), b[1]);
    }
    lock.write().unwrap().push(3);
    println!("{:?}", lock.into_inner().unwrap());
    // SystemTime
    let t = UNIX_EPOCH.checked_add(Duration::new(3, 500)).unwrap();
    println!("{:?} {}", t.duration_since(UNIX_EPOCH).unwrap(), UNIX_EPOCH.duration_since(t).is_err());
    // Bound
    let n = 4u8;
    println!("{} {} {}", kind(Bound::Included(&n)), kind(Bound::Excluded(&n)), kind(Bound::Unbounded));
    // FromStr and Display::fmt in Debug
    println!("{:?} {:?}", "21C".parse::<Celsius>(), Celsius::from_str("21"));
    println!("{} {:?}", Pt(1, 2), Pt(3, 4));
}
