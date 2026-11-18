# LSP go to definition and references (2026-11-17)

Date: 2026-11-17

Added support for `textDocument/definition` and `textDocument/references` (`navigate` in zust-syntax). It does no type inference; names are
resolved with the token stream and an item table (symbols flattened, with the owner type and module path attached):

- Locals: function parameters, `let` (visible after the `;`), `for` / `if let` / `while let` (from the next `{`), match arms, closure parameters.
- `x.name`: methods and fields of the type of x (the enclosing impl for `self`; for a local, its type annotation or its `T::..` / `T { .. }` / `T(..)` initializer).
  When the type is unknown, it answers only if there is exactly one member with that name.
- `Q::name`: a member of Q, or an item inside a module named Q. `Self` is the type of the enclosing impl.
- Items: same module → `use` (names, globs, uses of modules brought in by a glob, `as` aliases, the name inside a `use` itself) →
  unique in the workspace → std. std re-exports (`std::sync::Mutex` → sync::poison::mutex) are followed through `pub use`.
  The X in `#[derive(X)]` is the derive macro. Built-in types get no answer.
- Workspace: from the Cargo.toml (members) at the initialize root, it follows each crate root (lib / main / bin / tests / examples /
  benches / build.rs) and `mod x;` (also `#[path]` and inline mods) to decide each file's module path. Items in crates such as tests
  are visible only from the same crate.
- std / core / alloc (rust-src from `rustc --print sysroot`) are indexed in a separate thread after initialize (only definitions and uses are
  kept; the text is discarded and positions are stored as line and column: about 7 MB). `ZUST_ANALYZER_NO_STD=1` turns it off.
- references are the identifiers with the same name (looked up in a per-file name table) whose definition resolves to the same place.
  The inside of `macro_rules!` bodies is not counted (same as rust-analyzer).
- Memory: it keeps the token streams (stored as u32) and name tables of all workspace files, so peak RSS on the zustc target goes 10.9 → 29.9 MB
  (27.5 MB with the std index off). ready also goes 55 → 91 ms (building the workspace index).

`python3 bench/lsp_parity.py` (compares with rust-analyzer's answers; `--detail` prints the mismatches):

## synth (40 positions, 20 positions after `.`. Compared against rust-analyzer)

| Feature | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| Features that return results | docsym, wsym, definition, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (name match) | names 100% (kinds too 97%) | names 100% (kinds too 100%) | names 100% (kinds too 95%); extra #0 |
| workspace/symbol (top 20 match) | 87% | 100% | 76% |
| definition (target match) | same 100% (answered 100%) | same 100% (answered 100%) | same 100% (answered 100%) |
| hover (contains the name) | 0% | 100% | 100% |
| references (count match) | 100% | 100% | 100% |
| completion `x.\|` (contains the written name) | 0% (0 candidates) | 100% (3 candidates) | 100% (5 candidates) |
| completion `na\|me` (same) | 0% (0 candidates) | 91% (145 candidates) | 56% (5 candidates) |
| Median time (ms) | defi 0.8, hove 0.2, comp 0.1, refe 5.8 | defi 0.4, hove 0.5, comp 2.5, refe 1.0 | defi 6.3, hove 6.3, comp 218.4, refe 18.8 |

## zustc (40 positions, 20 positions after `.`. Compared against rust-analyzer)

| Feature | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| Features that return results | docsym, wsym, definition, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (name match) | names 99% (kinds too 97%); missing extern "C" | names 100% (kinds too 100%) | names 90% (kinds too 82%); missing c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (top 20 match) | 73% | 100% | 35% |
| definition (target match) | same 97% (answered 80%) | same 100% (answered 78%) | same 87% (answered 68%) |
| hover (contains the name) | 0% | 88% | 72% |
| references (count match) | 64% | 100% | 42% |
| completion `x.\|` (contains the written name) | 0% (0 candidates) | 100% (26 candidates) | 90% (37 candidates) |
| completion `na\|me` (same) | 0% (0 candidates) | 68% (85 candidates) | 60% (3 candidates) |
| Median time (ms) | defi 1.3, hove 0.3, comp 0.1, refe 13.6 | defi 0.8, hove 1.1, comp 6.6, refe 27.9 | defi 15.4, hove 13.0, comp 243.6, refe 315.0 |

- definition is 100% on synth and 97% on zustc (the mismatches: it answers for the item `Backend` that cfg excludes (rust-analyzer does
  not answer), and it cannot follow `FxMap::default()` through the type alias to its target (the HashMap impl)). rust-glancer is 87%. It takes about 1 ms per call.
- references (exactly the same count) is 100% on synth and 64% on zustc (rust-glancer 42%). The mismatches are mainly cases that need type inference:
  the callee of `Default::default()` (the method of an impl Default), `FxMap::default()` (the std method reached through the type alias),
  and `x.f()` where the receiver's type cannot be found from a local annotation. The rest are off by 1–3 in count (`Ty` with 2290, etc.). For built-in types
  (`u8`), rust-analyzer returns references but zustc does not. Time on the zustc target is 10–14 ms per call (rust-analyzer 28 ms).
- hover and completion are not supported yet.
