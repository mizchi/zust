// A bare variant name is the one the imports bring into scope: std's ControlFlow's, glob-imported, beside a program
// enum with variants of the same names that it does not import (its own are then named by their paths)
#![allow(dead_code)]
use std::ops::ControlFlow::{self, *};

#[derive(Debug, PartialEq)]
enum Local {
    Break(u8),
    Continue,
}

fn scan(v: &[u8]) -> ControlFlow<u8> {
    for &x in v {
        if x > 2 {
            return Break(x);
        }
    }
    Continue(())
}

fn local(n: u8) -> Local {
    if n > 0 { Local::Break(n) } else { Local::Continue }
}

fn main() {
    println!("{:?} {:?}", scan(&[1, 5]), scan(&[1]));
    match scan(&[9]) {
        Break(x) => println!("broke at {x}"),
        Continue(()) => println!("went on"),
    }
    let c: ControlFlow<u8> = Break(7u8);
    println!("{:?} {}", c, c.is_break());
    println!("{:?} {:?} {}", local(2), local(0), local(0) == Local::Continue);
}
