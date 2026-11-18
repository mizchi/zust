# LSP hover and completion, and memory for large workspaces (2026-11-17)

Date: 2026-11-17

Added support for `textDocument/hover` and `textDocument/completion` (triggered by `.` and `:`). Both look things up in the same index as
definition and references (`navigate` in zust-syntax). Type checking is not used.

- hover: the definition's signature (attributes removed, up to the body's `{`), the enclosing thing (impl / module), and the `///` doc comment before it.
  For locals, the inferred type (`q: Point`); for built-in types, their name.
- Completion:
  - `x.|`: fields and methods of the receiver's type.
  - `Q::|`: members of a type and items of a module. Re-exported names are included too (`std::sync::Mu|`).
  - Otherwise: visible locals, workspace items, names brought in by use, the prelude, common macros, keywords.
  - Filtered by what has been typed so far: prefix, word start. Locals are also matched by characters in order.
- Inferring the receiver's type (held as a string):
  - What can be followed: local type annotations, `let` initializer expressions, parameters, `self`, field types, and return values of methods and functions.
  - Type parameters of the impl are replaced with the receiver's arguments. Example: `unwrap` on `Option<Sig>` → `Sig`, `get` on `HashMap<K, V>` → `Option<&V>`.
  - Expression forms: indexing (`Vec<T>[i]` → T, map → V), `?`, `for x in v`, `if let Some(x) = ..`, `vec![]`, struct expressions, `as T`.
  - Type aliases are expanded together with their type arguments (`LockResult<G>` → `Result<G, ..>`). Deref looks at what is inside the wrapper (`MutexGuard<T>` → T).
  - Methods of implemented / derived traits (`clone`, `map` of `Iterator`, etc.) are also considered.
  - `..Default::default()` and `field: Default::default()` resolve to the `default` of the struct expression's type / the field's type.
- Fixes in the std index:
  - `pub const unsafe fn` and `const trait` could not be read (most methods of core's `impl str` were missing).
  - Files for `sys`, tests and other OSes are not included.
  - Positions are stored on definitions, and module paths / enclosing things are shared: about 21 MB → 10 MB.
- Large workspaces (`ZUST_ANALYZER_MEM_MB`, default 4):
  - Workspace text and token streams are kept only for files up to this total size.
  - Files beyond that are made slim and keep only definitions, `pub use`, and the set of hashes of the names that appear.
  - When references or hover need them, only the files selected by name hash are re-read. Re-read files are kept in an LRU up to the same size.
  - Even with `ZUST_ANALYZER_MEM_MB=0` (everything slim), the answers do not change (parity is the same). references get slower: 9 → 31 ms per call on the zustc target.

`python3 bench/lsp_parity.py`:

## synth (40 positions, 20 positions after `.`. Compared against rust-analyzer)

| Feature | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| Features that return results | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (name match) | names 100% (kinds too 97%) | names 100% (kinds too 100%) | names 100% (kinds too 95%); extra #0 |
| workspace/symbol (top 20 match) | 87% | 100% | 76% |
| definition (target match) | same 100% (answered 100%) | same 100% (answered 100%) | same 100% (answered 100%) |
| hover (contains the name) | 100% | 100% | 100% |
| references (count match) | 100% | 100% | 100% |
| completion `x.\|` (contains the written name) | 100% (4 candidates) | 100% (3 candidates) | 100% (5 candidates) |
| completion `na\|me` (same) | 97% (25 candidates) | 91% (145 candidates) | 56% (5 candidates) |
| Median time (ms) | defi 0.9, hove 0.8, comp 1.0, refe 3.5 | defi 0.4, hove 0.6, comp 3.1, refe 1.0 | defi 6.7, hove 6.7, comp 224.6, refe 18.9 |

## zustc (40 positions, 20 positions after `.`. Compared against rust-analyzer)

| Feature | zustc | rust-analyzer | rust-glancer |
|---|---|---|---|
| Features that return results | docsym, wsym, definition, hover, completion, references | docsym, wsym, definition, hover, completion, references, rename, signature, semantic, inlay, codeAction, format | docsym, wsym, definition, hover, completion, references, rename, semantic, inlay, codeAction, format |
| documentSymbol (name match) | names 99% (kinds too 97%); missing extern "C" | names 100% (kinds too 100%) | names 90% (kinds too 82%); missing c_close, c_exit, c_fork, c_mkdir |
| workspace/symbol (top 20 match) | 73% | 100% | 35% |
| definition (target match) | same 100% (answered 80%) | same 100% (answered 78%) | same 87% (answered 68%) |
| hover (contains the name) | 85% | 88% | 72% |
| references (count match) | 67% | 100% | 39% |
| completion `x.\|` (contains the written name) | 100% (40 candidates) | 100% (26 candidates) | 90% (37 candidates) |
| completion `na\|me` (same) | 80% (7 candidates) | 68% (85 candidates) | 60% (3 candidates) |
| Median time (ms) | defi 1.0, hove 1.0, comp 1.3, refe 8.8 | defi 1.0, hove 1.1, comp 7.0, refe 30.9 | defi 16.0, hove 13.9, comp 265.5, refe 388.4 |

- On the zustc target: hover 85% (rust-analyzer 88%) and `x.|` completion 100% (median candidate count 40, rust-analyzer 26).
  rust-glancer gets 72% and 90% (37 candidates) respectively.
- The only hover mismatches are 2 places with a name inside a string (`"ZUSTC_INLINE_LOG"`).
- For `na|me`, rust-analyzer returns many other candidates (median 85), so the name written there is sometimes not included (68%).
  zustc filters, so it gets 80% (7 candidates). Scoring higher on this metric does not mean the completion quality is better.
- definition is now 100% (`FxMap::default()` now resolves to the HashMap impl behind the alias). references is 67%. Most of the rest are
  counting differences off by 1–3, and references to built-in types (`u8`).
- Time (per call, zustc target): definition / hover 1.0 ms, completion 1.3 ms, references 9 ms. rust-analyzer is 1.0 / 1.1 / 7.0 / 31 ms.

`python3 bench/lsp_compare.py --cold` (a references column was added. big is a generated crate with 1000 modules, about 110,000 lines, 4.5 MB):

| Target | Server | init | ready | docsym (count) | wsym (found) | edit→docsym | edit→diag | references 1st / 2nd (count) | peak RSS (with children) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| synth | zustc | 4.9 ms | 14.6 ms | 0.8 ms (37) | 0.9 ms (5/5) | 2.1 ms | 1.5 ms | 207 ms / 3.5 ms (29) | 16.9 MB (15.8 MB) |
| synth | rust-analyzer | 23.9 ms | 3398 ms | 1.4 ms (37) | 1.0 ms (5/5) | 19.7 ms | 299 ms | 84.0 ms / 3.2 ms (29) | 596 MB (865 MB) |
| synth | rust-glancer | 26.0 ms | 5812 ms | 1.9 ms (38) | 31.2 ms (5/5) | 4.7 ms | - | 380 ms / 14.6 ms (29) | 15.7 MB (888 MB) |
| zustc | zustc | 2.5 ms | 86.7 ms | 1.8 ms (123) | 1.2 ms (5/5) | 13.6 ms | 10.9 ms | 71.9 ms / 10.0 ms (36) | 30.1 MB (30.1 MB) |
| zustc | rust-analyzer | 25.7 ms | 6653 ms | 4.4 ms (124) | 1.1 ms (5/5) | 12.3 ms | - | 1698 ms / 15.1 ms (36) | 1571 MB (1633 MB) |
| zustc | rust-glancer | 106 ms | 11194 ms | 7.7 ms (113) | 265 ms (5/5) | 48.0 ms | - | 1702 ms / 122 ms (36) | 29.8 MB (1799 MB) |
| big | zustc | 2.4 ms | 152 ms | 0.8 ms (37) | 14.4 ms (5/5) | 3.7 ms | 3.1 ms | 47.6 ms / 16.6 ms (29) | 55.1 MB (55.1 MB) |
| big | rust-analyzer | 20.1 ms | 4293 ms | 5.7 ms (37) | 14.7 ms (5/5) | 5.2 ms | 4875 ms | 487 ms / 7.1 ms (29) | 825 MB (2038 MB) |
| big | rust-glancer | 35.5 ms | 6206 ms | 1.9 ms (38) | 108 ms (5/5) | 5.0 ms | - | 926 ms / 29.4 ms (29) | 22.0 MB (1033 MB) |

- The first references call includes the time spent waiting for the std index (built in a separate thread after initialize): 207 ms on synth.
  This is shorter than the first call of rust-analyzer and rust-glancer (0.5–1.7 s).
- peak RSS:
  - zustc target: from 29.9 MB right after adding definition / references to 30.1 MB (adding type information for hover / completion and
    std impls is offset by the smaller std index).
  - big: 55 MB (28 MB with `ZUST_ANALYZER_MEM_MB=0`).
  - rust-analyzer is 0.6–1.6 GB. rust-glancer's main process is 16–30 MB, but 0.9–1.8 GB including child processes.
