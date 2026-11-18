# Multiple files and multiple crates (`zustc build <workspace>`)

Date: 2026-11-16

- `mod x;` reads `x.rs` / `x/mod.rs` (`src/srcmap.rs`). To keep line numbers unchanged, `mod x;` is blanked out and its contents are appended at the end of the file
  as `pub mod x { .. }`, with a table from lines of the combined source → (file, line). Errors become `file:line:col:`
- The workspace file has one crate per line (`lib <name> <root> [deps..]` / `bin <name> <root> [deps..]`). A .o is built per crate and they are linked at the end
- Crate C is analyzed as "C's source + the sources of the crates it depends on, appended as `pub mod name { .. }`".
  Non-generic functions and statics of dependency crates are in that crate's .o, so they are not compiled and are called by symbol. Instances of generic functions and
  the prelude are placed locally in each .o. Names inside a crate's modules are always `name__Item` (the same symbol no matter which crate it is analyzed with).
  `crate::` inside a crate means that crate, and `pub use` re-exports are followed too
- The panic table is placed per crate, and the executable's table points to the list of dependency crates' tables (`_xT_<name>`). The table holds the file mapping and
  a table of places where renaming shifted columns (`geom::f` → `geom__f` has the same length, but `f` → `geom__f` is longer), so panic locations are
  reported with the original file, line and column (`geom/src/lib.rs:13:37`, the same as rustc). The unwrap caller location (`__EXR_CALLER`) of the executable is shared by all
- Fingerprints: per crate, a hash of "compiler, settings, own source, interfaces of dependency crates" is written to `<name>.fp`, and if it is unchanged
  nothing is done. The interface hash includes only the signature for non-generic functions; for everything else (generic functions, traits, generic impls, `const fn`,
  `-> impl Trait`) it also includes the body and lines. So changing the body of a function in a depended-on crate does not recompile its users (only relinks)
- Out-of-date crates only use each other's sources (they do not wait for .o files), so they are compiled concurrently with fork. The cache is per crate (`<name>.crcache2`)
- `tests/workspace.rs`: compares tests/workspace (3 crates, 5 files: trait objects, statics, re-exports, Display) with rustc (rlib + link)
  (output, exit code, panic locations for division by zero / indexing / unwrap across crates and files). It also checks that a change to a function body rebuilds only that crate,
  and a change to a signature rebuilds its users too

`python3 bench/ws_bench.py --rustc` (6 lib × 8 files × 40 functions = 1920 functions + bin, 4 cores. rustc uses `-C incremental` rlib + link):

| Scenario | zustc build | rustc |
|---|---:|---:|
| cold | 489 ms | 3786 ms |
| no changes | 7 ms | 1043 ms |
| change the body of a function in the bottom crate | 66 ms | 1062 ms |
| change the signature of a function in the bottom crate | 189 ms | 1136 ms |
| change main in bin | 83 ms | 1029 ms |

(All are the median of 3 runs. The signature scenario varies between 150 and 190 ms)

What we did to make it faster (measured in this order):

| Change | cold | no changes | body | signature | bin |
|---|---:|---:|---:|---:|---:|
| initial (analyze every crate every time) | 1229 | 725 | 936 | 733 | 805 |
| fingerprint + interface hash + per-crate cache | 1296 | 15 | 798 → 83 | 743 | 237 |
| remove the quadratic cost of module renaming (per-token linear search replaced by a table) | 880 | 24 | 83 | 422 | 108 |
| compile out-of-date crates concurrently with fork | 486 | 20 | 81 | 229 | 113 |
| save the interface hash, use FxHash for the name table, and filter out non-path names first | 464 | 7 | 60 | 154 | 82 |

(The 798 → 83 for "body" comes from fixing a bug in the interface hash, which searched for `{` in the flattened text.)
What remains is linking (about 22 ms, all .o files every time) and the time to re-analyze dependency crates' sources for each user (renaming, splitting, prelude selection).

Slowness found during self-hosting: when the zustc-built compiler compiled itself, the stage that builds the .o took more than 13 minutes.
`parse::tokenize` counted characters from the start of the line for every token (quadratic on long lines; in the rustc version it was not noticeable thanks to SIMD `chars().count()`), so
it now counts on from the previous token. It became 2.4 seconds, and the whole second stage is 36 seconds (the rustc-built compiler takes 5.6 seconds).
