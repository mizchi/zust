// A program's own types named as std's ControlFlow, Layout, Once, Thread, Repeat and Condvar. The name brings in the
// prelude's ControlFlow too, whose variants have the same names: `use ControlFlow::*;` finds the program's own.
// std's Thread, renamed in the prelude against the program's, and its ThreadId, still print as `Thread` / `ThreadId`
#![allow(dead_code)]
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ControlFlow {
    Continue,
    Break(u8),
}
use ControlFlow::*;

#[derive(Debug)]
struct Layout {
    rows: usize,
}

struct Once(u8);

impl fmt::Display for Once {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "once {}", self.0)
    }
}

#[derive(Debug)]
struct Thread {
    name: &'static str,
}

#[derive(Debug)]
struct ThreadId(u8);

#[derive(Debug, PartialEq)]
enum Repeat {
    Never,
    Times(u32),
}

struct Condvar;

impl Condvar {
    fn new() -> Self {
        Condvar
    }
    fn notify_one(&self) -> &'static str {
        "mine"
    }
}

fn run(v: &[u8]) -> ControlFlow {
    for &x in v {
        if x > 2 {
            return Break(x);
        }
    }
    Continue
}

fn main() {
    println!("{:?} {:?}", run(&[1, 5]), run(&[1]));
    println!("{:?}", Layout { rows: 3 });
    println!("{}", Once(4));
    println!("{:?} {:?}", Thread { name: "t" }, ThreadId(9));
    println!("{:?} {:?}", Repeat::Never, Repeat::Times(2));
    println!("{}", Condvar::new().notify_one());
    // (std's Thread is renamed for the program's own: its Debug still says `Thread`)
    let t = std::thread::current();
    println!("{:?}", t);
    println!("{:?}", t.id());
}
