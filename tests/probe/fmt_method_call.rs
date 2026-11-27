// `x.fmt(f)` in an impl of Display or Debug (#256: zustc found no method `fmt`, which anstyle, getrandom, ipnet,
// iana-time-zone, mime and toml_datetime call): a type's own impl, through a reference and of a temporary, a derive,
// a str (with the formatter's flags), a number, an io::Error, and `fmt::Debug::fmt(s, f)` of a str
use std::fmt;
use std::io;

struct Inner(u8);

impl fmt::Display for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>", self.0)
    }
}

#[derive(Debug)]
struct Point {
    x: i32,
    y: i32,
}

struct Outer {
    i: Inner,
    p: Point,
}

impl Outer {
    fn render(&self) -> Inner {
        Inner(self.i.0 + 1)
    }

    fn fmt_to(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use core::fmt::Display as _;
        self.render().fmt(f)?;
        self.i.fmt(f)
    }
}

impl fmt::Display for Outer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() { "alt".fmt(f) } else { self.fmt_to(f) }
    }
}

impl fmt::Debug for Outer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.p.fmt(f)?;
        " / ".fmt(f)?;
        self.i.0.fmt(f)
    }
}

enum Net {
    V4(Inner),
    Name(String),
}

impl fmt::Display for Net {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Net::V4(ref a) => a.fmt(f),
            Net::Name(ref s) => s.fmt(f),
        }
    }
}

struct Mime(String);

impl fmt::Debug for Mime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self.0.as_str(), f)
    }
}

struct OsErr(io::Error);

impl fmt::Display for OsErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let e = &self.0;
        e.fmt(f)
    }
}

struct Msg;

impl fmt::Display for Msg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "invalid datetime".fmt(f)
    }
}

fn main() {
    let o = Outer { i: Inner(1), p: Point { x: 2, y: 3 } };
    println!("{o} {o:#} {o:?}");
    println!("{} {}", Net::V4(Inner(7)), Net::Name("host".to_string()));
    println!("{:?}", Mime("text/plain".to_string()));
    println!("[{}] [{:>20}] [{:<18}]", Msg, Msg, Msg);
    println!("{}", OsErr(io::Error::from_raw_os_error(2)));
}
