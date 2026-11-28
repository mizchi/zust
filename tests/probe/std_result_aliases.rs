// std's `Result` aliases by their paths (#260): fmt's (no parameters) through `fmt::`, `core::fmt::` and `std::fmt::`,
// io's `Result<T>`, thread's `Result<T>`, and a module's own alias with no parameters by its path
use std::fmt;
use std::io;
use std::thread;

struct P(i32);

impl fmt::Display for P {
    fn fmt(&self, f: &mut fmt::Formatter) -> core::fmt::Result {
        write!(f, "P{}", self.0)
    }
}

impl fmt::Debug for P {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        write!(f, "P({})", self.0)
    }
}

fn show(p: &P) -> fmt::Result {
    let mut s = String::new();
    use std::fmt::Write;
    write!(s, "{p} {p:?}")?;
    println!("{s}");
    Ok(())
}

fn size(p: &str) -> io::Result<usize> {
    std::fs::read(p).map(|v| v.len())
}

fn run() -> thread::Result<i32> {
    thread::spawn(|| 40 + 2).join()
}

fn unit() -> std::thread::Result<()> {
    Ok(())
}

mod m {
    #[derive(Debug)]
    pub struct E;
    pub type Res = core::result::Result<(), E>;
    pub fn ok(b: bool) -> Res {
        if b { Ok(()) } else { Err(E) }
    }
}

fn check(b: bool) -> m::Res {
    m::ok(b)
}

fn main() {
    show(&P(3)).unwrap();
    println!("{}", size("/nonexistent/zust-probe").is_err());
    println!("{} {}", run().unwrap(), unit().is_ok());
    println!("{:?} {:?}", check(true), check(false));
}
