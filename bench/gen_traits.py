#!/usr/bin/env python3
"""A generated program that leans on traits and type inference, for the KPIs (gen.py's functions are simple and
reach little of either).

usage: python3 bench/gen_traits.py N [variant] > p.rs     (N groups; variant changes one function's constant)

Each group has: a generic trait with an associated type and a default method, impls for a struct, a generic struct
and a primitive, generic functions with where-clauses calling through the traits, iterator chains with closures
(map / filter / fold / collect into Vec and HashMap), Option / Result with `?`, a trait object, and operator traits.
main calls every group and prints a checksum, so the output can be compared with rustc's.
"""
import sys


def group(i, variant):
    k = variant if i == 0 else 0
    return f"""
pub trait Shape{i} {{
    type Out: Copy + std::fmt::Debug;
    fn area(&self) -> Self::Out;
    fn scaled(&self, k: u64) -> u64 where Self::Out: Into<u64> {{
        let a: u64 = self.area().into();
        a * k + {k}
    }}
}}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect{i} {{ w: u32, h: u32 }}

#[derive(Clone, Debug)]
pub struct Wrap{i}<T> {{ items: Vec<T> }}

impl Shape{i} for Rect{i} {{
    type Out = u32;
    fn area(&self) -> u32 {{ self.w * self.h }}
}}

impl<T: Shape{i}<Out = u32>> Shape{i} for Wrap{i}<T> {{
    type Out = u32;
    fn area(&self) -> u32 {{ self.items.iter().map(|s| s.area()).sum() }}
}}

impl Shape{i} for u16 {{
    type Out = u16;
    fn area(&self) -> u16 {{ self.wrapping_mul(2) }}
}}

impl std::ops::Add for Rect{i} {{
    type Output = Rect{i};
    fn add(self, o: Rect{i}) -> Rect{i} {{ Rect{i} {{ w: self.w + o.w, h: self.h.max(o.h) }} }}
}}

pub fn total{i}<S>(xs: &[S]) -> u64 where S: Shape{i}, S::Out: Into<u64> {{
    xs.iter().map(|s| s.scaled(3)).fold(0u64, |a, b| a.wrapping_add(b))
}}

pub fn parse{i}(s: &str) -> Result<u32, std::num::ParseIntError> {{
    let n: u32 = s.trim().parse()?;
    Ok(n + {i})
}}

pub fn pick{i}(v: &[Rect{i}]) -> Option<u32> {{
    let first = v.iter().find(|r| r.w > 1)?;
    Some(first.area())
}}

pub fn dyn_sum{i}(xs: &[Box<dyn Shape{i}<Out = u32>>]) -> u32 {{
    xs.iter().map(|s| s.area()).sum()
}}

pub fn run{i}() -> u64 {{
    let rs: Vec<Rect{i}> = (1..6u32).map(|n| Rect{i} {{ w: n, h: n + 1 }}).collect();
    let big = rs.iter().copied().fold(Rect{i} {{ w: 0, h: 0 }}, |a, b| a + b);
    let w = Wrap{i} {{ items: rs.clone() }};
    let mut by_area: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for r in rs.iter().filter(|r| r.area() % 2 == 0) {{
        *by_area.entry(r.area()).or_insert(0) += 1;
    }}
    let boxed: Vec<Box<dyn Shape{i}<Out = u32>>> = vec![Box::new(big), Box::new(w.clone())];
    let ints: Vec<u16> = vec![3, 4, 5];
    total{i}(&rs) + total{i}(&ints) + w.scaled(2) + by_area.len() as u64
        + dyn_sum{i}(&boxed) as u64 + parse{i}(" 7 ").unwrap_or(0) as u64 + pick{i}(&rs).unwrap_or(0) as u64
}}
"""


def main():
    n = int(sys.argv[1])
    variant = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    out = [group(i, variant) for i in range(n)]
    calls = " + ".join(f"run{i}()" for i in range(n))
    out.append(f"\nfn main() {{\n    let s: u64 = {calls};\n    println!(\"{{}}\", s);\n}}\n")
    sys.stdout.write("".join(out))


if __name__ == "__main__":
    main()
