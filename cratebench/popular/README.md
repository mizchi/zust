# Popular crates

The goal: programs that use the most depended-on crates.io crates build with zustc and print what rustc's build
prints. Each `apps/<crate>/` uses the crate's main API and prints deterministic text; random-number crates print
properties (lengths, "two draws differ") or use a seeded generator.

| crate | what the app exercises |
|---|---|
| hashbrown | `HashMap` / `HashSet` with the default hasher: entry API, retain, set algebra, growth |
| syn | `parse_file` and `parse_str` with `full`: items, expressions, a rejected input |
| getrandom | `fill` and `u64` from the OS |
| bitflags | the `bitflags!` macro, set operations, `iter_names`, parsing |
| rand_core | a generator written against `TryRng` / `SeedableRng` |
| rand | `StdRng` from a seed (ChaCha), ranges, floats, shuffle, choose; the thread RNG |
| base64 | standard, URL-safe and unpadded engines, round trips, a rejected input |
| libc | `getpid`, `strlen`, `malloc` / `memset` / `free`, `clock_gettime` |
| quote | `quote!` with repetition and `format_ident!` over proc-macro2 tokens |
| proc-macro2 | lexing a `TokenStream` from a string, walking groups, a rejected input |

```bash
cargo build --release -p zustc -p zurgo
python3 cratebench/popular/run.py                 # all, or name some: run.py syn rand
python3 cratebench/popular/run.py --rustc-only    # only the expected outputs (any platform)
```

Per crate the result is `pass` (same stdout), `mismatch`, `run-fail`, `build-fail` (with the package the first
error points into) or `timeout`; files are in `out/popular/<crate>/`. CI runs it on every push that touches the
compiler (`.github/workflows/popular-crates.yml`) and shows the table in the run's summary.

To add a crate: make `apps/<crate>/` with `[workspace]` in its Cargo.toml (it is not a member of zust's workspace),
pin the crate with `=`, commit its Cargo.lock, and check the output with `--rustc-only`.
