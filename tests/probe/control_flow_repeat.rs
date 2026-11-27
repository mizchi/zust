// `core::ops::ControlFlow` as rustls-pki-types' PEM reader returns it, and `iter::Repeat` / `iter::Empty` as types
// (bit-set's `Skip<Take<Enumerate<Repeat<B>>>>`)
use std::iter::{self, Empty, Enumerate, Repeat, Skip, Take};
use std::ops::ControlFlow;

fn step(line: &str) -> Result<ControlFlow<Option<u32>, ()>, String> {
    if line.is_empty() {
        return Ok(ControlFlow::Break(None));
    }
    if line.starts_with('#') {
        return Ok(ControlFlow::Continue(()));
    }
    match line.parse::<u32>() {
        Ok(n) => Ok(ControlFlow::Break(Some(n))),
        Err(_) => Err(format!("bad line {line:?}")),
    }
}

fn first(lines: &[&str]) -> Result<Option<u32>, String> {
    for l in lines {
        match step(l)? {
            ControlFlow::Continue(()) => continue,
            ControlFlow::Break(item) => return Ok(item),
        }
    }
    Ok(None)
}

type Blanks = Skip<Take<Enumerate<Repeat<u8>>>>;

fn blanks(n: usize) -> Blanks {
    iter::repeat(0u8).enumerate().take(n).skip(1)
}

fn main() {
    println!("{:?}", first(&["# a", "#b", "42", "7"]));
    println!("{:?}", first(&["#", ""]));
    println!("{:?}", first(&["x"]));
    let c: ControlFlow<i32> = ControlFlow::Continue(());
    let b: ControlFlow<i32, &str> = ControlFlow::Break(3);
    println!("{:?} {:?} {} {}", c, b, c.is_continue(), b.is_break());
    println!("{:?} {:?}", b.break_value(), b.continue_value());
    println!("{:?}", b.map_break(|x| x * 10));
    let k: ControlFlow<u8, i32> = ControlFlow::Continue(5);
    println!("{:?} {}", k.map_continue(|x| x + 1), k == ControlFlow::Continue(5));
    let r: Repeat<char> = iter::repeat('z');
    let s: String = r.take(3).collect();
    println!("{s}");
    println!("{:?}", blanks(4).collect::<Vec<_>>());
    let e: Empty<u8> = iter::empty();
    println!("{}", e.count());
}
