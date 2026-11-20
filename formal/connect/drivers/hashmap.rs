// A driver for formal/connect: zustc's prelude HashMap, run by a program zustc compiles. It reads one action per line
// on stdin and prints its observation on stdout (0 is "no value"), as formal/tla/HashMapModel.tla describes:
//   reset          -> ok           (a new, empty map)
//   insert K V     -> prev P       (the value K had)
//   remove K       -> prev P
//   get K          -> val V
//   len            -> len N
// Keys are spread out (K * 1000003) so that they do not sit in neighbouring buckets.
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let mut out = io::stdout();
    let mut m: HashMap<u64, u64> = HashMap::new();
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let w: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> u64 { w[i].parse::<u64>().unwrap() };
        let reply = match w[0] {
            "reset" => {
                m = HashMap::new();
                String::from("ok")
            }
            "insert" => format!("prev {}", m.insert(num(1) * 1000003, num(2)).unwrap_or(0)),
            "remove" => format!("prev {}", m.remove(&(num(1) * 1000003)).unwrap_or(0)),
            "get" => format!("val {}", m.get(&(num(1) * 1000003)).copied().unwrap_or(0)),
            "len" => format!("len {}", m.len()),
            _ => String::from("error"),
        };
        writeln!(out, "{}", reply).unwrap();
        out.flush().unwrap();
    }
}
