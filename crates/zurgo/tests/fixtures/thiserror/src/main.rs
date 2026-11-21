// thiserror 2: its build script (OUT_DIR/private.rs from CARGO_PKG_VERSION_PATCH, and probes of $RUSTC) and its
// library (`include!(concat!(env!("OUT_DIR"), "/private.rs"))`, `dyn Error + Send + Sync + UnwindSafe`). The derive is
// a proc macro (mizchi/zust#29), so the error type here implements the traits by hand
use std::error::Error;
use std::fmt;

#[derive(Debug)]
enum ParseError {
    Empty,
    Bad(String, usize),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty input"),
            ParseError::Bad(s, at) => write!(f, "bad input {s:?} at {at}"),
        }
    }
}

impl Error for ParseError {}

fn parse(s: &str) -> Result<u32, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }
    let mut n = 0u32;
    for (i, c) in s.chars().enumerate() {
        match c.to_digit(10) {
            Some(d) => n = n * 10 + d,
            None => return Err(ParseError::Bad(s.to_string(), i)),
        }
    }
    Ok(n)
}

fn main() {
    for s in ["42", "", "4x2"] {
        match parse(s) {
            Ok(n) => println!("{s:?} -> {n}"),
            Err(e) => {
                let b: Box<dyn Error + Send + Sync> = Box::new(e);
                println!("{s:?} -> error: {b}");
            }
        }
    }
}
