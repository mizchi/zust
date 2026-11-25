"""Bundles zustc's own crates/syntax/src/lexer.rs (and the KeyHasher of key.rs) into a single-file program.
usage: python3 tests/selfhost/build_lexer.py > out/selfhost/lexer_main.rs
main runs the files given as arguments through split / hoist_nested_items / flatten_modules and prints the items' kinds, positions and hashes."""
import re

lexer = open("crates/syntax/src/lexer.rs").read()
cache = open("crates/base/src/key.rs").read()
a = cache.index("pub type Key = u128;")
b = len(cache)
hasher = cache[a:b]
print("""// 自動生成: tests/selfhost/build_lexer.py (zustc の crates/syntax/src/lexer.rs をそのまま含む)
mod cache {
use std::hash::{Hash, Hasher};
""" + hasher + """}

mod lexer {
""" + lexer + """}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        match lexer::split(&src) {
            Ok(items) => {
                println!("{}: {} items", path, items.len());
                for it in &items {
                    println!("{:?} {}..{} line {} idents {} hash {:032x}", it.kind, it.start, it.end, it.line, it.idents.len(), it.hash);
                }
            }
            Err(e) => println!("{}: error {}", path, e),
        }
        println!("hoist: {:?}", lexer::hoist_nested_items(&src).map(|s| s.len()));
        println!("flatten: {:?}", lexer::flatten_modules(&src).map(|s| s.len()));
    }
}
""")
# (lexer uses crate::key: the same thing as cache)
print("mod key {\n    pub use crate::cache::*;\n}")
