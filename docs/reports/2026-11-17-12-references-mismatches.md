# Closing the remaining references mismatches (2026-11-17)

Date: 2026-11-17

Looked at the mismatches from `python3 bench/lsp_parity.py --detail` and aligned the counting with rust-analyzer.

- References to a type now include `Self` inside that type's impls (`-> Option<Self>` in `impl Enc for Vec<T>` is a reference to Vec).
- Pattern and parameter bindings (`let (a, concrete) = ..` / `fn f(concrete: T)` / `for (i, x) in` / `|a, b|`) resolve to the binding itself.
  Before, they were counted as references to a function with the same name, `concrete`.
- If Q in `Q::name` is an unknown name (`syn::Pat` from an external crate, `Delimiter::None` from `use proc_macro2::Delimiter`), no answer is given.
  Before, `None` resolved to Option's None and `Pat` to the workspace's Pat.
- cfg:
  - Items excluded by `#[cfg(..)]` are removed from definitions, and the names inside them are not counted. If the cursor is inside one, no answer is given.
  - features are followed per crate, starting from default in `[features]` of Cargo.toml.
  - Other things it understands: test, debug_assertions, unix / windows, target_os, and all / any / not.
- References to built-in types (`u8`, etc.): the positions of types with the same name in the workspace. The `0u8` suffix, the name in `fn u8`, and `x.u8` are excluded.

## synth (40 positions, 20 positions after `.`. Compared against rust-analyzer)

| Feature | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| Features that return results | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (name match) | names 100% (kinds too 97%) | names 100% (kinds too 100%) | names 100% (kinds too 95%); extra #0 |
| workspace/symbol (top 20 match) | 87% | 100% | 76% |
| definition (target match) | same 100% (answered 100%) | same 100% (answered 100%) | same 100% (answered 100%) |
| hover (contains the name) | 100% | 98% | 100% |
| references (count match) | 100% | 100% | 100% |
| completion `x.\|` (contains the written name) | 100% (4 candidates) | 100% (3 candidates) | 100% (5 candidates) |
| completion `na\|me` (same) | 97% (25 candidates) | 91% (145 candidates) | 56% (5 candidates) |
| Median time (ms) | defi 0.8, hove 0.7, comp 1.0, refe 3.7 | defi 0.5, hove 0.7, comp 3.7, refe 1.8 | defi 7.5, hove 7.6, comp 235.4, refe 19.8 |

## zustc (40 positions, 20 positions after `.`. Compared against rust-analyzer)

| Feature | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| Features that return results | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (name match) | names 99% (kinds too 97%); missing extern "C" | names 100% (kinds too 100%) | names 90% (kinds too 82%); missing c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (top 20 match) | 73% | 100% | 35% |
| definition (target match) | same 100% (answered 78%) | same 100% (answered 78%) | same 87% (answered 68%) |
| hover (contains the name) | 82% | 88% | 72% |
| references (count match) | 78% | 100% | 39% |
| completion `x.\|` (contains the written name) | 100% (40 candidates) | 100% (26 candidates) | 90% (37 candidates) |
| completion `na\|me` (same) | 80% (7 candidates) | 68% (85 candidates) | 60% (3 candidates) |
| Median time (ms) | defi 1.1, hove 1.0, comp 1.3, refe 16.6 | defi 0.9, hove 1.2, comp 6.2, refe 28.3 | defi 14.9, hove 12.5, comp 252.8, refe 330.1 |

- references (exactly the same count) on the zustc target: 67% → 78%.
- The remaining 9 positions are all due to how rust-analyzer behaves.
  - Inside `#[cfg(not(feature = "build"))] fn lower_blob`, which cfg excludes:
    - rust-analyzer counts the names inside (1–2 for Vec and Analysis).
    - It does not answer for `type Backend` under the same cfg.
  - `u8`: rust-analyzer also counts the inside of prelude_items.rs (`include!`) generated under target/.
  - `Key`: rust-analyzer counts `mod key { pub type Key = u128; }` in build.rs as a reference to Key in crates/base.
- Time: references take 17 ms per call (9 ms before, because `Self` inside impls is now resolved too). rust-analyzer takes 28 ms.
- Same answers with `ZUST_ANALYZER_MEM_MB=0` (everything slim).
- hover went 85 → 82%: it no longer answers for `Backend`, which cfg excludes (rust-analyzer does not answer either).
