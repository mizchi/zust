"""Puts zustc's own crates/syntax/src/macros.rs (the expansion of macro_rules! and include_str!) together into a program of 1 file.
The proc_macro2 crate cannot be used, so a replacement for just the parts used (tests/selfhost/mini_pm2.rs) is attached
(the rustc version uses the same replacement).
usage: python3 tests/selfhost/build_macros.py > out/selfhost/macros_main.rs
main calls macros::expand for each file in its arguments and prints the length, line count and hash of the expanded result (and errors)."""

macros = open("crates/syntax/src/macros.rs").read()
pm2 = open("tests/selfhost/mini_pm2.rs").read()
# (expand uses lexer::tokenize; lexer uses the cache's Key / KeyHasher)
lexer = open("crates/syntax/src/lexer.rs").read()
key = open("crates/base/src/key.rs").read()
cfg = open("crates/syntax/src/cfg.rs").read()
# (expand runs proc macros through pm.rs, and keeps its tables in fxhash's FxMap)
pm = open("crates/syntax/src/pm.rs").read()
fx = open("crates/base/src/fxhash.rs").read()
print("""// 自動生成: tests/selfhost/build_macros.py (zustc の crates/syntax/src/macros.rs をそのまま含む)
""" + pm2 + """
mod cache {
""" + key + """
}

mod lexer {
""" + lexer + """
}

mod cfg {
""" + cfg + """
}

mod macros {
""" + macros + """
use crate::proc_macro2;
}

mod pm {
""" + pm + """
use crate::proc_macro2;
}

mod fxhash {
""" + fx + """
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        match macros::expand(&src) {
            Ok(Some(t)) => println!("{path}: expanded {} bytes {} lines {:016x}", t.len(), t.lines().count(), fnv(&t)),
            Ok(None) => println!("{path}: no macros"),
            Err(e) => println!("{path}: error {e}"),
        }
        let dir = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new("."));
        match macros::expand_includes(&src, dir) {
            Ok(Some(t)) => println!("  includes: {} bytes {:016x}", t.len(), fnv(&t)),
            Ok(None) => {}
            Err(e) => println!("  includes error: {e}"),
        }
    }
}
""")
# (lexer uses crate::key: the same as cache)
print("mod key {\n    pub use crate::cache::*;\n}")
