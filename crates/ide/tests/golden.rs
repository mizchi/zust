//! Golden tests of ide: at the positions of all identifiers in the small workspace of tests/fixture (and a small std), produces definition / references /
//! hover / completion and compares them with tests/golden/*.txt. Also compares the symbol lists and the definition tables (Def).
//! The expected values are zustc's output itself (when it changes, look at the diff and fix it, or run `UPDATE_GOLDEN=1 cargo test -p zust-ide --test golden`
//! to rewrite them). The destinations of definitions within the workspace have been compared with rust-analyzer by bench/ide_golden_ra.py.
//!
//! Also checks that the answers are the same when the other files are made slim (when their text is dropped and re-read).

use zust_ide::navigate::{self, FileIndex, Item, Loc};
use zust_ide::symbols::{self, Symbol};
use zust_syntax::lexer::tokenize;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn rs_files(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let mut es: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    es.sort();
    for p in es {
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rs_files(&p, base, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
}

/// The fixture workspace: lib/ (in place of std), ws/ (src is the main crate, tests is another crate), sym/ (files outside
/// the crates: checks how symbols are read)
struct Fx {
    names: Vec<String>,
    texts: Vec<String>,
    ws: Vec<FileIndex>,
}

fn index(name: &str, text: &str, features: &[&str]) -> FileIndex {
    let fs: Vec<String> = features.iter().map(|s| s.to_string()).collect();
    if let Some(rel) = name.strip_prefix("lib/") {
        let mut f = FileIndex::library(text.to_string(), navigate::module_of(rel));
        // (a number per library crate: same as the LSP)
        let k = ["core", "alloc", "std"].iter().position(|c| rel.starts_with(&format!("{c}/"))).unwrap_or(9);
        f.krate = navigate::LIB_KRATE_BASE + k as u32;
        return f;
    }
    let (rel, krate) = if let Some(r) = name.strip_prefix("ws/") {
        (r, if r.starts_with("src/") { 0 } else { 1 })
    } else {
        (name, 2)
    };
    let mut f = FileIndex::new(text.to_string(), navigate::module_of(rel.strip_prefix("sym/").unwrap_or(rel)));
    f.in_src = krate == 0;
    f.krate = krate;
    f.apply_cfg(fs);
    f
}

/// part: "ws" (lib and ws) / "sym" (lib and sym) / "all"
fn fixture_of(features: &[&str], part: &str) -> Fx {
    let base = root().join("fixture");
    let mut names = vec![];
    rs_files(&base, &base, &mut names);
    names.retain(|n| part == "all" || n.starts_with("lib/") || n.starts_with(&format!("{part}/")));
    let texts: Vec<String> = names.iter().map(|n| std::fs::read_to_string(base.join(n)).unwrap()).collect();
    let ws = names.iter().zip(&texts).map(|(n, t)| index(n, t, features)).collect();
    // (crate roots: the same as the LSP sets)
    let mut roots = vec![];
    for (c, f) in [("core", "lib/core/src/lib.rs"), ("alloc", "lib/alloc/src/lib.rs"), ("std", "lib/std/src/lib.rs"), ("fixture", "ws/src/lib.rs")] {
        if let Some(i) = names.iter().position(|n| n == f) {
            roots.push((c.to_string(), i));
        }
    }
    navigate::set_crate_roots(roots);
    Fx { names, texts, ws }
}

fn fixture(features: &[&str]) -> Fx {
    fixture_of(features, "ws")
}

impl Fx {
    fn refs(&self) -> Vec<&FileIndex> {
        self.ws.iter().collect()
    }
    fn load(&self, i: usize) -> Option<Rc<FileIndex>> {
        Some(Rc::new(self.ws[i].reload(self.texts[i].clone())))
    }
    fn lc(&self, file: usize, at: usize) -> String {
        if self.ws[file].lib {
            // (libraries keep no text: the line and column of an end of the name)
            let (l, c) = self.ws[file].lib_pos(at).unwrap_or((u32::MAX, 0));
            return format!("{}:{}:{}", self.names[file], l + 1, c + 1);
        }
        let t = &self.texts[file][..at];
        let l = t.matches('\n').count() + 1;
        let c = at - t.rfind('\n').map(|i| i + 1).unwrap_or(0) + 1;
        format!("{}:{l}:{c}", self.names[file])
    }
    fn loc(&self, l: &Loc) -> String {
        let s = self.lc(l.file, l.start);
        let width = l.end - l.start;
        format!("{s}+{width}")
    }
    fn locs(&self, v: &[Loc]) -> String {
        if v.is_empty() {
            return "-".to_string();
        }
        v.iter().map(|l| self.loc(l)).collect::<Vec<_>>().join(" ")
    }
}

fn esc(s: &str) -> String {
    s.replace('\n', "⏎")
}

fn item(it: &Item) -> String {
    let k = it.kind.map(|k| format!("{k:?}")).unwrap_or_else(|| if it.local { "local".into() } else { "-".into() });
    if it.detail.is_empty() { format!("{}/{k}", it.label) } else { format!("{}/{k}/{}", it.label, it.detail) }
}

fn check(name: &str, got: &str) {
    let path = root().join("golden").join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    if want != got {
        let (mut shown, mut n) = (String::new(), 0);
        for (i, (a, b)) in want.lines().map(Some).chain(std::iter::repeat(None)).zip(got.lines().map(Some).chain(std::iter::repeat(None))).enumerate() {
            if a.is_none() && b.is_none() {
                break;
            }
            if a != b {
                n += 1;
                if n <= 20 {
                    let _ = writeln!(shown, "line {}:\n  want {}\n  got  {}", i + 1, a.unwrap_or("<none>"), b.unwrap_or("<none>"));
                }
            }
        }
        panic!("{name} が期待値と違う ({n} 行)。意図した変更なら UPDATE_GOLDEN=1 で書き直す\n{shown}");
    }
}

/// The (start, end) of identifier tokens (kind 0)
fn idents(text: &str) -> Vec<(usize, usize)> {
    tokenize(text.as_bytes()).into_iter().filter(|t| t.0 == 0).map(|t| (t.1, t.2)).collect()
}

fn nav_files(fx: &Fx) -> Vec<usize> {
    (0..fx.names.len()).filter(|&i| !fx.ws[i].lib).collect()
}

/// At every identifier: definition / references (with and without the definition) / hover
#[test]
fn navigation() {
    check("navigation.txt", &nav_dump(&fixture(&["std"])));
}

/// Produces the same for sym/ (unusual styles / broken code) too (must not crash, and must read as far as it can)
#[test]
fn navigation_odd() {
    check("navigation_odd.txt", &nav_dump(&fixture_of(&["std"], "sym")));
}

fn nav_dump(fx: &Fx) -> String {
    let ws = fx.refs();
    let load = |i: usize| fx.load(i);
    let mut out = String::new();
    let mut listed: HashSet<String> = HashSet::new();
    for i in nav_files(&fx) {
        let _ = writeln!(out, "== {}", fx.names[i]);
        let text = &fx.texts[i];
        for (a, b) in idents(text) {
            let name = &text[a..b];
            let d = navigate::definition(&ws, i, a + (b - a) / 2);
            let r = navigate::references(&ws, i, a, true, &load);
            let r2 = navigate::references(&ws, i, b, false, &load);
            let h = navigate::hover(&ws, i, a, &load).map(|h| esc(&h)).unwrap_or_else(|| "-".into());
            // (the reference list only once per destination; for built-in types with no destination, once per name)
            let key = d.first().map(|l| fx.loc(l)).unwrap_or_else(|| name.to_string());
            let _ = write!(out, "{} {name} -> {} | refs {}/{}", fx.lc(i, a), fx.locs(&d), r.len(), r2.len());
            if listed.insert(key) && !r.is_empty() {
                let _ = write!(out, " [{}]", fx.locs(&r));
            }
            let _ = writeln!(out, " | {h}");
        }
    }
    out
}

/// Completion at `x.|` / `x.na|me` / `Q::|` / `na|me`
#[test]
fn completion() {
    let fx = fixture(&["std"]);
    let ws = fx.refs();
    let mut out = String::new();
    for i in nav_files(&fx) {
        let _ = writeln!(out, "== {}", fx.names[i]);
        let text = &fx.texts[i];
        let bs = text.as_bytes();
        for (a, b) in idents(text) {
            let name = &text[a..b];
            let after_dot = a >= 1 && bs[a - 1] == b'.' && !(a >= 2 && bs[a - 2] == b'.');
            let after_path = a >= 2 && &bs[a - 2..a] == b"::";
            if after_dot || after_path {
                let (v, cut) = navigate::complete(&ws, i, a);
                let top: Vec<String> = v.iter().take(12).map(item).collect();
                let rank = v.iter().position(|x| x.label == name).map(|p| p as i64).unwrap_or(-1);
                let _ = writeln!(out, "{} {}|{name} n={} cut={cut} rank={rank} [{}]", fx.lc(i, a), if after_dot { "." } else { "::" }, v.len(), top.join(" "));
                if b - a >= 3 {
                    let (v, _) = navigate::complete(&ws, i, a + 1);
                    let rank = v.iter().position(|x| x.label == name).map(|p| p as i64).unwrap_or(-1);
                    let _ = writeln!(out, "{} {}{}|{} n={} rank={rank}", fx.lc(i, a), if after_dot { "." } else { "::" }, &name[..1], &name[1..], v.len());
                }
            } else if b - a >= 4 {
                let (v, cut) = navigate::complete(&ws, i, a + 2);
                let rank = v.iter().position(|x| x.label == name).map(|p| p as i64).unwrap_or(-1);
                let top: Vec<String> = v.iter().take(5).map(item).collect();
                let _ = writeln!(out, "{} {}|{} n={} cut={cut} rank={rank} [{}]", fx.lc(i, a), &name[..2], &name[2..], v.len(), top.join(" "));
            }
        }
    }
    // (the end of the file, an empty file, whitespace before an identifier)
    let f = FileIndex::new(String::new(), vec![]);
    let (v, _) = navigate::complete(&[&f], 0, 0);
    let _ = writeln!(out, "== empty n={}", v.len());
    let src = "fn f() { let abc = 1; let x =   ab }";
    let f = FileIndex::new(src.to_string(), vec![]);
    let at = src.find("ab }").unwrap();
    for off in [at, at + 1, at + 2, src.len() + 10] {
        let (v, cut) = navigate::complete(&[&f], 0, off);
        let _ = writeln!(out, "inline@{off} n={} cut={cut} [{}]", v.len(), v.iter().take(4).map(item).collect::<Vec<_>>().join(" "));
    }
    check("completion.txt", &out);
}

fn sym_line(out: &mut String, s: &Symbol, depth: usize) {
    let _ = writeln!(out, "{}{:?} {} [{}..{}] name {}..{} {}", "  ".repeat(depth), s.kind, s.name, s.start, s.end, s.name_start, s.name_end, esc(&s.detail));
    for c in &s.children {
        sym_line(out, c, depth + 1);
    }
}

/// The documentSymbol tree, its flattened form (with the container names), and the LSP kind numbers
#[test]
fn symbol_tree() {
    let fx = fixture_of(&[], "all");
    let mut out = String::new();
    for (n, t) in fx.names.iter().zip(&fx.texts) {
        let _ = writeln!(out, "== {n}");
        let syms = symbols::document_symbols(t);
        for s in &syms {
            sym_line(&mut out, s, 0);
        }
        for (s, c) in symbols::flatten(&syms) {
            let _ = writeln!(out, "flat {} in {c:?} lsp={}", s.name, s.kind.lsp());
        }
    }
    check("symbols.txt", &out);
}

/// The index's definition table (Def) and the impl pairs. With different features (what cfg excludes)
#[test]
fn def_tables() {
    let mut out = String::new();
    for feats in [&["std"][..], &[][..], &["std", "extra"][..]] {
        let fx = fixture_of(feats, "all");
        let _ = writeln!(out, "#### features {feats:?}");
        for (i, f) in fx.ws.iter().enumerate() {
            let _ = writeln!(out, "== {} module={:?} lib={} slim={} src={} krate={}", fx.names[i], f.module, f.lib, f.slim, f.in_src, f.krate);
            for d in &f.defs {
                let _ = writeln!(
                    out,
                    "{:?} {} owner={:?} in={:?} mod={:?} ty={:?} params={:?} at={} item={}..{} pos={:?}",
                    d.kind,
                    d.name,
                    d.owner,
                    &*d.container,
                    &d.module[..],
                    d.ty,
                    d.params,
                    fx.lc(i, d.start),
                    d.item_start,
                    d.item_end,
                    d.pos
                );
            }
            if !f.impls.is_empty() {
                let _ = writeln!(out, "impls {:?}", f.impls);
            }
        }
    }
    check("defs.txt", &out);
}

/// The answers of definition / references / hover are the same when the other files are made slim (when the LSP drops them from memory)
#[test]
fn slim_is_transparent() {
    let fx = fixture(&["std"]);
    let full = fx.refs();
    let load = |i: usize| fx.load(i);
    for i in nav_files(&fx) {
        let mut slim: Vec<FileIndex> = vec![];
        for j in 0..fx.ws.len() {
            let mut f = index(&fx.names[j], &fx.texts[j], &["std"]);
            if j != i {
                f.make_slim();
                f.make_slim();
            }
            slim.push(f);
        }
        let sw: Vec<&FileIndex> = slim.iter().collect();
        let text = &fx.texts[i];
        for (a, _) in idents(text) {
            let what = format!("{} {}", fx.lc(i, a), &text[a..]);
            let what = &what[..what.find('\n').unwrap_or(what.len()).min(60)];
            assert_eq!(navigate::definition(&sw, i, a), navigate::definition(&full, i, a), "definition {what}");
            let mut r1 = navigate::references(&sw, i, a, true, &load);
            let mut r2 = navigate::references(&full, i, a, true, &load);
            r1.sort_by_key(|l| (l.file, l.start));
            r2.sort_by_key(|l| (l.file, l.start));
            assert_eq!(r1, r2, "references {what}");
            assert_eq!(navigate::hover(&sw, i, a, &load), navigate::hover(&full, i, a, &load), "hover {what}");
        }
    }
}

/// A slimmed one keeps no text and is restored by reload. heap_bytes gets smaller
#[test]
fn slim_and_reload() {
    let fx = fixture(&["std"]);
    for (i, f) in fx.ws.iter().enumerate() {
        if f.lib {
            assert_eq!(f.text_len(), 0, "{}", fx.names[i]);
            continue;
        }
        let mut s = index(&fx.names[i], &fx.texts[i], &["std"]);
        let before = s.heap_bytes();
        assert_eq!(s.text_len(), fx.texts[i].len());
        s.make_slim();
        assert!(s.slim && s.text_len() == 0);
        let after = s.heap_bytes();
        assert!(after.1 < before.1, "{}: {before:?} → {after:?}", fx.names[i]);
        assert!(after.0 <= before.0);
        let r = s.reload(fx.texts[i].clone());
        assert!(!r.slim);
        assert_eq!((r.in_src, r.krate, &r.module), (f.in_src, f.krate, &f.module));
        let names = |x: &FileIndex| x.defs.iter().map(|d| (d.name.clone(), d.start)).collect::<Vec<_>>();
        assert_eq!(names(&r), names(f), "{}", fx.names[i]);
        let fresh = index(&fx.names[i], &fx.texts[i], &["std"]).heap_bytes();
        assert_eq!((r.heap_bytes().0, r.heap_bytes().1), (fresh.0, fresh.1));
    }
}
