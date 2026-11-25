//! Does the item splitting of the prelude (src/prelude_lib.rs.in) at build time.
//! Because splitting the whole prelude (thousands of lines) with the lexer on every run takes half the build time of a small program.
//! The splitting uses src/lexer.rs as it is (the hash is 0, since it is not used for prelude items).

use std::fmt::Write;

#[path = "src/lexer.rs"]
#[allow(dead_code)]
mod lexer;

#[allow(dead_code)]
mod key {
    pub type Key = u128;
    pub struct KeyHasher;
    impl KeyHasher {
        pub fn new() -> Self {
            KeyHasher
        }
        pub fn add<T: ?Sized>(&mut self, _v: &T) {}
        pub fn finish(&self) -> Key {
            0
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src/prelude_lib.rs.in");
    println!("cargo:rerun-if-changed=src/lexer.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let src = std::fs::read_to_string("src/prelude_lib.rs.in").unwrap();
    let items = lexer::split(&src).expect("prelude must lex");
    let mut out = String::new();
    out.push_str("// build.rs が生成する: prelude のアイテム (start, end, line, kind, kind の引数, 識別子)\n");
    out.push_str("pub static PRELUDE_ITEMS: &[(u32, u32, u32, u8, u32, &[(u32, u32)])] = &[\n");
    for it in &items {
        let (k, a) = match it.kind {
            lexer::ItemKind::Normal => (0, 0),
            lexer::ItemKind::ImplHeader => (1, 0),
            lexer::ItemKind::ImplMember(h) => (2, h),
            lexer::ItemKind::TraitHeader => (3, 0),
            lexer::ItemKind::TraitMember(h) => (4, h),
            lexer::ItemKind::InnerAttr => (5, 0),
        };
        write!(out, "({}, {}, {}, {k}, {a}, &[", it.start, it.end, it.line).unwrap();
        for (x, y) in &it.idents {
            write!(out, "({x}, {y}),").unwrap();
        }
        out.push_str("]),\n");
    }
    out.push_str("];\n");
    // (the names the prelude gives a struct / enum / trait / union: prelude::rename_user_collisions)
    out.push_str("pub static PRELUDE_TYPE_NAMES: &[&str] = &[\n");
    for it in &items {
        if !matches!(it.kind, lexer::ItemKind::Normal | lexer::ItemKind::TraitHeader) {
            continue;
        }
        let mut ids = it.idents.iter().take(6).map(|&(a, b)| &src[a as usize..b as usize]);
        if ids.any(|w| matches!(w, "struct" | "enum" | "trait" | "union")) {
            if let Some(n) = ids.next() {
                write!(out, "{n:?},").unwrap();
            }
        }
    }
    out.push_str("];\n");
    let dst = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("prelude_items.rs");
    std::fs::write(dst, out).unwrap();
}
