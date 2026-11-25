"""Combines zustc's own crates/syntax/src/prelude.rs (picks prelude entries by name) together with lexer.rs, fxhash.rs and key.rs's KeyHasher
into a one-file program.
usage: python3 tests/selfhost/build_prelude.py > out/selfhost/prelude_main.rs
For each argument file, main calls prelude::select with it as the user source and prints the selected prelude's length, line count and hash.
(include_str! is rewritten to point at crates/syntax/src/prelude_lib.rs.in as seen from out/selfhost/, and the table that build.rs generates,
 PRELUDE_ITEMS (the newest target/release/build/zust-core-*/out/prelude_items.rs), is embedded as is)"""
import glob, os

lexer = open("crates/syntax/src/lexer.rs").read()
fx = open("crates/base/src/fxhash.rs").read()
items = max(glob.glob("target/release/build/zust-syntax*/**/prelude_items.rs", recursive=True), key=os.path.getmtime)
prelude = (open("crates/syntax/src/prelude.rs").read()
    .replace('include_str!("prelude_lib.rs.in")', 'include_str!("../../crates/syntax/src/prelude_lib.rs.in")')
    .replace('include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));', open(items).read()))
cache = open("crates/base/src/key.rs").read()
a = cache.index("pub type Key = u128;")
b = len(cache)
hasher = cache[a:b]
print("""// 自動生成: tests/selfhost/build_prelude.py (zustc の crates/syntax/src/prelude.rs をそのまま含む)
mod cache {
use std::hash::{Hash, Hasher};
""" + hasher + """}

mod fxhash {
""" + fx + """}

mod lexer {
""" + lexer + """}

mod prelude {
""" + prelude + """}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut sc = prelude::SelectCache::default();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        let Ok(items) = lexer::split(&src) else {
            println!("{}: lex error", path);
            continue;
        };
        let p = prelude::select(&src, &items);
        let q = prelude::select_with(&src, &items, &mut sc);
        let mut h = cache::KeyHasher::new();
        h.add(&p);
        println!("{}: {} bytes {} lines {:032x} same={}", path, p.len(), p.lines().count(), h.finish(), p == q);
    }
}
""")
# (lexer uses crate::key: the same as cache)
print("mod key {\n    pub use crate::cache::*;\n}")
