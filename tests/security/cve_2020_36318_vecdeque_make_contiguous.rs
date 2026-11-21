//! CVE-2020-36318 (std, fixed in 1.49.0): `VecDeque::make_contiguous` popped the same element twice for some
//! layouts of a wrapped buffer (a use after free / double free). Every layout of head and length must give each
//! element exactly once.
//@ cve: CVE-2020-36318
//@ expect: run
use std::collections::VecDeque;
fn main() {
    for cap in [4usize, 5, 8] {
        for head in 0..cap {
            for len in 0..=cap {
                let mut d: VecDeque<String> = VecDeque::with_capacity(cap);
                for i in 0..head {
                    d.push_back(format!("pad{i}"));
                }
                for _ in 0..head {
                    d.pop_front();
                }
                for i in 0..len {
                    d.push_back(format!("e{i}"));
                }
                let s = d.make_contiguous();
                let joined: Vec<&str> = s.iter().map(|x| x.as_str()).collect();
                let want: Vec<String> = (0..len).map(|i| format!("e{i}")).collect();
                if joined != want.iter().map(|x| x.as_str()).collect::<Vec<_>>() {
                    println!("cap {cap} head {head} len {len}: {:?}", joined);
                }
            }
        }
    }
    println!("done");
}
