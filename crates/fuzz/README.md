# zust-fuzz

Mutation-based fuzzing of zustc's front end and checker. It uses std only (no libFuzzer, no nightly), so it runs
anywhere cargo does, CI included.

```bash
cargo build --release -p zust-fuzz
target/release/zust-fuzz fuzz --target all --time 300            # 5 minutes over every target
target/release/zust-fuzz fuzz --target parse,expand-diff --time 60
target/release/zust-fuzz fuzz --target check --corpus out/rust-tests/tests/ui --time 600
target/release/zust-fuzz replay --target all out/fuzz/findings/panic-*.min.rs
target/release/zust-fuzz minimize --target parse --edition 2015 some_input.rs
```

## Targets

| target | what runs | a finding |
|---|---|---|
| `lex` | the parser's tokenizer, the item splitter, the lexer's token scan | a panic |
| `parse` | `parse::parse_file` | a panic |
| `expand` | the `macro_rules!` expander | a panic |
| `expand-diff` | the fast expander against the reference one (as `tests/macros_expand.rs` compares them) | a panic, or the two disagree |
| `prep` | the preprocessing: cfg, macros, module flattening, items moved out of bodies | a panic |
| `check` | `zustc check` in process: everything up to the type, borrow and region checks | a panic |

Every target also finds hangs (an input over `--timeout-ms`, 5 s by default) and crashes (a stack overflow or an
abort, which kill the worker process).

## How it works

- The corpus is the `.rs` files under `--corpus` (default: `examples/`, `tests/probe/`, `tests/fuzz/`), each up to
  64 KiB. Running over rustc's tests/ui (`python3 bench/rust_ui.py --fetch` puts it in `out/rust-tests/tests/ui`) gives
  the most varied inputs.
- A mutant applies 1 to 4 mutations on a rough token split: delete, duplicate or swap token ranges, insert or replace
  tokens from a dictionary of Rust's corner cases (delimiters, literal prefixes, comments, attributes, macro matchers,
  lifetimes, keywords), splice in a range of another file, or wrap a range in a delimiter pair (or half of one).
  The edition is picked per input.
- An input whose answer is new (the start of its first error, per target) joins the in-memory corpus, as a cheap
  stand-in for coverage feedback.
- The supervisor (`fuzz`) runs a worker process and restarts it after a crash or a hang. The worker writes each input
  to `<out>/current.rs` before it runs it, so the supervisor can save what killed it.
- A finding is saved under `<out>/findings/` (default `out/fuzz/findings/`) as `<kind>-<hash>.rs`, with
  `.min.rs` (minimized by delta debugging over tokens: the smallest source that fails at the same place) and `.txt`
  (where, the message, the edition). One place is reported once per worker.
- `--seed` makes a run reproducible. `fuzz` exits 1 when it found a panic, a hang or a crash. An `expand-diff`
  mismatch is saved and printed, and fails the run only with `--strict-diff`: random inputs are mostly invalid Rust,
  where the two expanders may tokenize differently without either being wrong.

## After a fix

Put the minimized input in `tests/fuzz/` (with `// edition: N` on its first line if it needs one).
`tests/regressions.rs` runs every file there through every target and fails on a panic or a mismatch, and the
fuzzer starts from them.

CI: `.github/workflows/fuzz.yml` runs every target for a while each day (and on demand), and uploads the findings.
