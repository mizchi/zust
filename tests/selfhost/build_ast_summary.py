"""Combines zustc's own crates/syntax/src/ast_summary.rs (item summaries; a port of front.rs's summarize) together with lexer / types / parse /
ast_types / ast_closure into a one-file program.
usage: python3 tests/selfhost/build_ast_summary.py > out/selfhost/ast_summary_main.rs
For each argument file, main flattens the modules, moves nested items out, then lexes it and prints the summaries of all items."""

def body(p):
    return open(p).read()

cache = open("crates/base/src/key.rs").read()
a = cache.index("pub type Key = u128;")
b = len(cache)
hasher = cache[a:b]
parts = [("types", "crates/syntax/src/types.rs"), ("parse", "crates/syntax/src/parse.rs"), ("ast_types", "crates/syntax/src/ast_types.rs"),
         ("ast_closure", "crates/syntax/src/ast_closure.rs"), ("ast_summary", "crates/syntax/src/ast_summary.rs"), ("lexer", "crates/syntax/src/lexer.rs"), ("fxhash", "crates/base/src/fxhash.rs")]
out = ["// 自動生成: tests/selfhost/build_ast_summary.py (zustc の src/*.rs をそのまま含む)",
       "mod cache {\nuse std::hash::{Hash, Hasher};\n" + hasher + "}\n"]
for name, path in parts:
    out.append(f"mod {name} {{\n" + body(path) + "}\n")
out.append("""
use lexer::ItemKind;
use types::Summary;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let src0 = std::fs::read_to_string(path).unwrap();
        let s1 = lexer::flatten_modules(&src0).unwrap_or(src0.clone());
        let src = lexer::hoist_nested_items(&s1).unwrap_or(s1);
        let items = match lexer::split(&src) {
            Ok(v) => v,
            Err(e) => {
                println!("{path}: lex error {e}");
                continue;
            }
        };
        let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
        let mut lines: Vec<String> = vec![String::new(); items.len()];
        for pass in 0..2 {
            for (i, it) in items.iter().enumerate() {
                let header = matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader);
                if header != (pass == 0) {
                    continue;
                }
                let ow = if pass == 1 { ast_summary::owner_of(it, &sums) } else { None };
                match ast_summary::parse_and_summarize(&src, it, ow.as_ref()) {
                    Ok(s) => {
                        lines[i] = format!("{s:?}");
                        sums[i] = s;
                    }
                    Err(e) => lines[i] = format!("error {e}"),
                }
            }
        }
        println!("{path}: {} items", items.len());
        for l in lines {
            println!("  {l}");
        }
    }
}
""")
print("\n".join(out))
# (lexer uses crate::key: the same as cache)
print("mod key {\n    pub use crate::cache::*;\n}")
