# Performance

The KPIs were measured <!-- measured -->2026-11-28, 0f96483<!-- /measured --> by CI on each push to main (bench.yml runs
`bench/ci_bench.py`, then `scripts/update_readme.py` writes them here and their summary in the
[README](README.md#speed)). Times come from shared GitHub Actions runners and vary by about ±20%; the instruction
counts (valgrind) do not. How to measure a change yourself: [CONTRIBUTING.md](CONTRIBUTING.md).

## KPIs

Build speed, build cache size and LSP latency on generated programs of 1000 / 5000 functions (`bench/ci_bench.py`).
In parentheses: the instruction count (valgrind), which does not vary with the runner:

<!-- auto:kpi -->
| KPI | gen1000 | gen5000 |
|---|---:|---:|
| cold build | 127 ms | 578 ms |
| no-op build | 0.7 ms (0.5 M) | 0.7 ms (0.5 M) |
| build after editing one function | 16.8 ms (128.4 M) | 67.7 ms (594.1 M) |
| `check` | 41 ms (798.9 M) | 188 ms (3923.7 M) |
| build cache on disk | 387.6 KiB | 1903.2 KiB |
| binary | 324.1 KiB | 1558.4 KiB |
| cold build peak RSS | 23.1 MiB | 55.7 MiB |

| zust-analyzer (zustc's own sources) | |
|---|---:|
| p50: definition, hover, completion, references, rename | 1.02 ms / 0.86 ms / 1.86 ms / 14.88 ms / 0.86 ms |
| startup until indexed | 335 ms |
| peak RSS | 141.1 MiB |
<!-- /auto:kpi -->

Every change is also measured on its own with `python3 bench/kpi_track.py`, so that small increases that add up stay
visible: [bench/kpi_log.md](bench/kpi_log.md) has one row per change with its effect (Δ) and the total since the
reference (Σ). The [benchmark dashboard](https://mizchi.github.io/zust/dev/bench/) has the CI numbers of every commit.

## Against rustc, relax and rust-analyzer (snapshots)

Measured with `python3 bench/vs_rustc.py` and `python3 cratebench/run.py` (2026-11-18, 4 cores, rustc 1.100.0; times are
medians of 3 runs). These comparisons are run by hand (they need rustc, relax and rust-analyzer side by side) and are dated. gen1000 is a generated program with 1000 functions:

| gen1000 | cold | no-op | edit one function | on disk | binary (stripped) | run |
|---|---:|---:|---:|---:|---:|---:|
| zustc | 123 ms | 18 ms | 27 ms | 766 KiB | 324 KiB | 11 ms |
| zustc `-O` | 374 ms | 68 ms | 67 ms | 1199 KiB | 341 KiB | 3 ms |
| rustc `-C incremental` | 756 ms | 197 ms | 406 ms | 10.8 MiB | 1038 KiB | 7 ms |
| rustc `-O` | 18345 ms | - | - | - | 377 KiB | 2 ms |
| **zustc vs rustc `-C incremental`** | **6.1x faster** | **11x faster** | **15x faster** | **14x smaller** | **3.2x smaller** | |

Small programs (`bench/rt`: fib, hashmap, matmul, nbody, sieve, sort, strings) and real crates (cratebench: fnv,
adler2, hex, itoa, rustc-hash, cfg-if; all six give the same output as rustc):

| | zustc vs rustc `-C incremental` (bench/rt) | zustc vs rustc (cratebench) |
|---|---:|---:|
| cold build | 2.5–4.0x faster (21–39 ms vs 71–131 ms) | 2.9–4.2x faster (22–61 ms vs 90–234 ms) |
| edit and rebuild | 12–17x faster (4–6 ms vs 63–75 ms) | 4.8–7.0x faster (11–22 ms vs 68–114 ms) |
| binary | 13–24x smaller | 93–550x smaller (8–48 KiB vs 4.4 MiB) |
| run time vs rustc `-O` | zustc 1.2–3.6x slower, zustc `-O` 1.0–2.9x slower | zustc 3.1–22x slower, zustc `-O` 0.9–9.1x |

zustc's code is close to rustc's debug build at run time, and faster than it in all seven small programs (rustc's
debug build is 3–26x slower than rustc `-O` there).

Against relax ([makepad/relax](https://github.com/makepad/relax) 3489f2d, a Rust JIT that compiles each function
when it is first called; `python3 bench/vs_relax.py`, 2026-11-18). relax runs the program while it compiles it, so the
three are compared on the time from source to the program's last output (zustc and rustc: build, then run). Every
output matches rustc `-O`:

| source to finished output | cold | no-op | edit one function | cache on disk |
|---|---:|---:|---:|---:|
| gen1000: zustc | 103 ms | 23 ms | 31 ms | 766 KiB |
| gen1000: rustc `-C incremental` | 585 ms | 173 ms | 384 ms | 10.8 MiB |
| gen1000: relax run | 1193 ms | 74 ms | 569 ms | 145 MiB |
| bench/rt (7 programs): zustc | 87–278 ms | 56–224 ms | 58–266 ms | 37–90 KiB |
| bench/rt: rustc `-C incremental` | 164–1718 ms | 141–1654 ms | 145–1615 ms | 0.1–0.9 MiB |
| bench/rt: relax run | 831–1460 ms | 106–356 ms | 168–435 ms | 136–137 MiB |
| **zustc vs relax** | **5.3–12x faster** | **1.3–3.4x faster** | **1.6–18x faster** | **1/190 (gen1000)–1/3800** |

relax's numbers include starting its runtime and loading its own std; its strength (patching edits into a running
program, keeping its state) is not measured here. On the small programs, the run itself is a large part of every
number: rustc's debug build runs slowly, so relax beats it on five of the seven after an edit.

Against blaze ([indexable-inc/blaze](https://github.com/indexable-inc/blaze) 0.20261027.0, a replacement for cargo
whose daemon keeps the workspace compiled and builds only what an edit changed), on this machine and the same
programs: each program as a cargo package for `blaze build` (dev profile, the daemon left running between builds as it
is meant to be used; cold also stops it), `python3 bench/vs_blaze.py` (2026-11-19, 4 cores, with other jobs running;
medians of 3; every output matches rustc `-O`). Memory is the peak RSS of one edit build for zustc and rustc, and what
blaze's daemon holds:

| | cold | no-op | edit one function | on disk | memory |
|---|---:|---:|---:|---:|---:|
| gen5000: zustc | 719 ms | 2 ms | 100 ms | 3.6 MiB | 37 MiB |
| gen5000: rustc `-C incremental` | 3656 ms | 829 ms | 2245 ms | 55.8 MiB | 482 MiB |
| gen5000: blaze | 9400 ms | 32 ms | 773 ms | 304 MiB | 661 MiB |
| gen1000: zustc | 214 ms | 2 ms | 34 ms | 766 KiB | 17 MiB |
| gen1000: rustc `-C incremental` | 828 ms | 202 ms | 468 ms | 10.8 MiB | 157 MiB |
| gen1000: blaze | 2834 ms | 29 ms | 218 ms | 259 MiB | 442 MiB |
| bench/rt (6 programs): zustc | 34–54 ms | 2 ms | 5–12 ms | 36–90 KiB | 13 MiB |
| bench/rt: rustc `-C incremental` | 81–203 ms | 65–126 ms | 75–168 ms | 0.1–0.9 MiB | 82–94 MiB |
| bench/rt: blaze | 1705–2311 ms | 32–60 ms | 41–74 ms | 244 MiB | 342–389 MiB |
| **zustc vs blaze** | **13–61x faster** | **15–30x faster** | **6–11x faster** | **1/83 (gen5000)–1/6900** | **1/18–1/30** |

A no-op build reads no source: like blaze's file watcher (`crates/blaze-vfs/src/watch.rs`: a file whose stat stamp
did not move is never read or hashed), zustc keeps `<output>.stamp` with the stat stamps of every file the last
successful build read, of the places `mod x;` looked, of the zustc binary and of the output, and stops when the
command line, the `ZUSTC_*` environment and every stamp are as kept (a stamp taken within 20 ms of its file's change,
or 2 s on a file system with whole-second times, is not trusted: git's racy rule, as blaze-vfs's `window_ns`).
Before it, a no-op build did the front end over the whole source with every function a cache hit (84 ms on gen5000,
against blaze's 29 ms). `ZUSTC_NO_STAMP=1` turns it off.

hashmap is left out of bench/rt's rows: blaze does not build it (`HashMap::new` reaches a thread-local static it cannot
compile yet). blaze builds real cargo projects that zustc cannot build yet (ripgrep, nushell, bun: its own
[benchmarks](https://github.com/indexable-inc/blaze/blob/main/docs/benchmarks.md) show 56 ms to rebuild ripgrep after an
edit, against cargo's 508 ms, on 16 cores of an AMD EPYC 9R45), and compiles the standard library itself, which is
most of its cold build and of its target directory here. Its way of tracking its numbers (each merge's instruction
counts against the median of the last runs, with a noise band) is what `bench/kpi_track.py` does too.

Compatibility against blaze on the same tests: rustc's tests/ui, 300 of the tests `bench/rust_ui.py` counts as
applicable of each kind (all 135 build-pass), picked with a fixed seed, each built by `blaze build` as the `src/main.rs`
of a cargo package (its `//@ edition:`, else 2015; the C helpers compiletest links linked through a build script), and
run for run-pass (`python3 bench/blaze_ui.py --per-kind 300`, 2026-11-20, blaze 0.20261027.0, zustc d9294dc; the table
is also in `bench/blaze_ui_result.md`). compile-fail is left out: blaze reports rust-analyzer's diagnostics, whose lines
this harness does not compare. blaze passes more of each kind; its front end is rust-analyzer's with rustc's trait
solver, where zustc's is its own. Most of blaze's misses here are functions its code generator does not compile yet
(`thread::spawn` reaches a writable static it has no symbol for), and a few are its own type errors:

| tests/ui kind | sample | blaze | zustc |
|---|---:|---:|---:|
| run-pass | 300 | 254 (84.7%) | 224 (74.7%) |
| check-pass | 300 | 274 (91.3%) | 245 (81.7%) |
| build-pass | 135 | 123 (91.1%) | 97 (71.9%) |

zust-analyzer against rust-analyzer and rust-glancer on zustc's own sources (40 query points,
`python3 bench/lsp_parity.py --servers zustc,rust-analyzer,rust-glancer --targets zustc`, 2026-11-18, e45ee10;
rust-glancer is github.com/rust-glancer/rust-glancer 50dd51a):

| | startup until symbols resolve | memory | references p50 | agreement (mean over the queries) |
|---|---:|---:|---:|---:|
| zust-analyzer | 113 ms | 71 MB | 10.9 ms | 99.0% |
| rust-analyzer | 6118 ms | 2060 MB | 214.0 ms | 98.9% |
| rust-glancer | 9965 ms | 1573 MB | 548.8 ms | 58.5% |

Details are in [bench/lsp_parity_result.md](bench/lsp_parity_result.md).
