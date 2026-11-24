# Survey: the Rust compiler's frontier (November 2026), and what zustc takes from it

Date: 2026-11-23

This report records what is new upstream in the Rust compiler, language and tooling, and where zustc stands against each item, so that the roadmap follows the frontier instead of rustc as it was. The sources are the 2026 project goals, the release notes of 1.95–1.100, the Rust blog, and tracking issues (links at the end). Where zustc's state is given, it was measured on 2026-11-23 on 4 cores with:

- rustc 1.97.0 stable (installed here);
- rustc 1.102.0-nightly (2026-11-22), which has Polonius Alpha on by default;
- zustc main d2146f7 (`cargo build --release`);
- rustc's tests/ui at 1.100.0 (`python3 bench/rust_ui.py --fetch`), the version this repository is developed against.

Three items were written up as issues: Polonius Alpha ([#1](https://github.com/mizchi/zust/issues/1#issuecomment-7110120344)), the next-generation trait solver ([#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011)) and a parallel front end ([#101](https://github.com/mizchi/zust/issues/101)).

## 1. Summary

| area | upstream (2026-11) | zustc | what to do |
|---|---|---|---|
| Borrow check | Polonius Alpha on by default on nightly since 2026-08-04, stable targeted before the end of 2026 | Already Polonius-like (one origin per local). Disagrees with Alpha on 5 of 34 `nll/polonius` tests (2 more are unsupported) | #1: location-sensitive liveness in place of `Bk::forever` |
| Trait solving | New solver in coherence (1.84) and orphan checking (1.100). Stabilizing `-Znext-solver=globally` is a 2026 goal | `implements(t, tr) -> bool`, by name. 26 false and 23 missed errors in 193 `traits/next-solver` tests | #2: goals with a certainty, nested where-clauses, normalization as goals |
| Parallel front end | Being promoted; reported enabled by default on nightly from 2026-11-16 (unconfirmed) | `check` runs in one process: as slow as a cold build at `-j 4` | #101: fork per wave of the check |
| Rebuilds | Relink Don't Rebuild: an initial implementation in an open PR | Done at function level (callers keep their code when a callee's body changes) | Do the same across crates (reuse dependency analysis) |
| Debug codegen | TPDE (single-pass back end, 10–20x faster than LLVM -O0) as an experimental option | Cranelift with the single-pass register allocator, and minclif | Compare minclif against TPDE's design |
| Generics | Polymorphic code generation experiment (hidden per-parameter dictionaries, layout-keyed sharing) | One instance per type arguments | The roadmap's "share instances keyed by layout" is the same idea |
| Language | `!` stable, custom allocators, `cfg_select!`, `if let` guards, new range types, many APIs | 4 of 12 probes pass | Close the 8 gaps (section 4) |

## 2. Build speed

The 2026 roadmap ["Fast Builds"](https://goals.rust-lang.org/2026/roadmap-fast-builds.html) says batch compilation is "past the point of easy wins". It has 12 workstreams. The ones that bear on zustc:

- **Relink Don't Rebuild (RDR)**, tracking issue [rust-lang/rust#158844](https://github.com/rust-lang/rust/issues/158844). A crate's metadata gets a public API hash, and dependents rebuild only when it changes; otherwise only the link reruns. The goal is 5–10x faster rebuilds for common changes. An initial implementation is in rust-lang/rust#156940 (open). Open problems:
  - spans are part of the hash, so editing a body or a doc comment still rebuilds dependents;
  - v0 mangling (the default since 1.97) is incompatible with it;
  - it costs about 3% on non-incremental builds and 25% on no-op incremental ones.

  zustc already does this within a crate, and without the span problem: a function's key holds only the signatures of its callees, and hashes are of normalized tokens (`docs/architecture.md`, "Key points of incrementality"). Across crates it does not yet: a dependent crate analyzes its dependencies' sources again. This is the roadmap's "reuse the analysis results of dependency crates", and RDR is the upstream evidence that it is the next big win for multi-crate builds (the "crates edit" KPI).
- **TPDE** ([goal](https://goals.rust-lang.org/2026/tpde.html), [paper](https://arxiv.org/abs/2505.22610)). A back end from TU Munich that runs one analysis pass, then instruction selection, register allocation and encoding in a single pass. On LLVM IR it is 8–24x faster than `-O0` with similar code quality (SPECint 2017). rustc would offer it as `-Z tpde` inside the LLVM back end, falling back to `-O0` for what it does not support. It targets ELF x86-64 and AArch64 only, and vector types it cannot legalize are its known blocker on Rust code. For zustc this confirms minclif's direction (a small x86-64 back end with its own register allocation). Measure where minclif and Cranelift's `single_pass` allocator spend their time against TPDE's single-pass design.
- **Improve `rustc_codegen_cranelift`.** The goal targets 2x inside the back end. In February 2026, Cranelift was about 5% faster than LLVM on Zed's incremental builds without debug info, and 10–15% on average for one crate with line tables. zustc already uses Cranelift (0.137.1) with `regalloc_algorithm=single_pass` outside `-O` (`crates/clif/src/lib.rs`), so Cranelift's own improvements reach it by updating the crate.
- **Polymorphic code generation experiment** ([goal](https://goals.rust-lang.org/2026/polymorphization.html)). Generic code with hidden arguments per type parameter (`TypeId`, drop glue, trait methods), and instances still split by data layout (`foo::<&u32>` and `foo::<&String>` share one). The estimate is 30% to 2x on post-monomorphization work, with smaller binaries. A proof of concept works partly on `std`. This is the roadmap's "share monomorphized instances keyed by layout" (blaze's polymorph); upstream adding hidden arguments is a step further than zustc's plan.
- **Incremental linking with Wild.** Still a goal ("2026 and beyond"); Wild itself does not link incrementally yet. zustc's roadmap items (embed libwild, replace only changed `.o` files, padding and jump tables) are not behind upstream.
- **Target directory size reduction.** zustc's cache is 1/14 of rustc's incremental directory on gen1000 (PERFORMANCE.md); nothing to take here.
- **Crate slicing** (future: compile only the parts of dependencies that are used; 1.4–30x in experiments). zustc already compiles only what is reachable from `main`, within the program and the prelude.
- **Parallel front end**: [#101](https://github.com/mizchi/zust/issues/101).

## 3. Type system and borrow check

### Polonius Alpha

On nightly by default since 2026-08-04, with `-Zpolonius=off` giving NLL back; stable is planned before the end of 2026 ([blog](https://blog.rust-lang.org/2026/08/04/enabling-polonius-alpha-on-nightly)). Outlives constraints become location-sensitive: a borrow that flows into a returned region is live only on the paths where it flows there. Full Polonius (the datalog version) is not planned, and Alpha is neither a subset nor a superset of it.

The 34 tests of `tests/ui/nll/polonius` without auxiliary crates, checked by stable (NLL), nightly (Alpha) and `zustc check`, accepted or rejected only:

| NLL / Alpha / zustc | tests |
|---|---:|
| reject / accept / accept | 13 |
| reject / accept / reject | 2 |
| accept / accept / reject | 1 |
| reject / reject / accept | 2 |
| same verdict | 14 |
| unsupported in zustc | 2 |

zustc already accepts NLL problem case 3 (nine tests), because a loan is live only while a live local holds it. It rejects the blog's smallest example (`let b = &mut *a; if true { b } else { a }`), because `Bk::forever` keeps a loan that flows into a universal region live to the end of the body: NLL's rule. It accepts walking a list to its end with a reassigned `&mut`, which Alpha rejects and datalog Polonius accepts. All these tests have `//@ revisions`, so `bench/rust_ui.py` skips them: none of this shows in the tests/ui numbers. Details and the plan: [#1](https://github.com/mizchi/zust/issues/1#issuecomment-7110120344).

### The next-generation trait solver

Used in coherence since 1.84 and in orphan checking by default since 1.100. The 2026 goal is to stabilize it everywhere and remove the old solver ([goal](https://goals.rust-lang.org/2026/next-solver.html)). rust-analyzer replaced chalk with it, blaze vendors rustc's solver, and relax ported it.

193 tests of `tests/ui/traits/next-solver/**`, checked by stable (old solver), nightly with `-Znext-solver=globally` and `zustc check`: 113 same verdict, 26 false errors, 23 missed errors, 14 accepted only by the new solver and rejected by zustc, 9 accepted only by the new solver and by zustc, 7 unsupported or timing out. The false errors are mostly inference that a trait goal should drive ("cannot infer type parameter `T`"), projections on objects and in the param env, and candidate preference. One process was killed for lack of memory during this run, so the counts may be off by a few. Plan: [#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011).

### Also in the 2026 goals

Not taken up yet, but each bears on what zustc checks: "Next-Gen Region Constraints", "Expanding a-mir-formality to work better as a Rust type system spec" (a possible oracle for the region check), "End-to-End Executable Rust Specification", "Stabilize FLS Release Cadence", "Reintroduce a FCW system to borrowck", and "Normative Documentation for Sound `unsafe` Rust".

## 4. Language and library: 1.95–1.100

Stable since the last survey: 1.95 (2026-04-16), 1.96 (2026-05-28), 1.97 (2026-07-09), 1.98 (2026-08-20), 1.99 (2026-10-01) and 1.100 (2026-11-12). 1.101 is in beta for 2026-12-24.

A program was written for one feature of each release, built with rustc 1.97, rustc 1.102-nightly and zustc (`--edition=2024`), and run. Features stabilized after 1.97 were checked against the nightly's output:

| release | feature | zustc |
|---|---|---|
| 1.95 | `cfg_select!` | ok |
| 1.95 | `if let` guards | ok |
| 1.95 | `AtomicUsize::update` | ok |
| 1.96 | `core::range::Range` (Copy, IntoIterator) | "cannot find value `from`" |
| 1.97 | `u32::bit_width` | no method |
| 1.98 | `#[core::derive(Debug, Clone)]` | **the attribute is dropped silently**, then "no method named `clone`" |
| 1.98 | `str::substr_range`, `bool::ok_or`, `Path::is_empty` | no method `substr_range` |
| 1.99 | `Vec::into_parts` / `from_parts`, `String::from_utf8_lossy_owned` | no method `into_parts` |
| 1.100 | `!` as a type (`Vec<!>`, `Result<u8, !>`, irrefutable `let Ok(x)`) | ok |
| 1.100 | `Infallible` is an alias of `!` | "expected `Result<i32, !>`, found `Result<i32, Infallible>`" |
| 1.100 | `Allocator` / `Global`, `Vec::new_in`, `Box::new_in` | "cannot find type `Global`" |
| 1.100 | `bool::toggle`, `Vec::from_fn`, `str::trim_prefix` | no method `toggle` |

C-variadic definitions (1.99) were also tried, but the probe used a `VaList` method the nightly does not have, so it is left out.

Other changes of these releases that zustc should follow:

- 1.100: never-type fallback is `!` on all editions; `box` patterns are no longer allowed; the next solver in orphan checking; Unicode 18.0.0 in std.
- 1.99: `mod foo;` inside attribute and derive macros (with #29); `no_mangle_generic_items` is a hard error.
- 1.98: stricter ambiguous-import errors (#79); `repr(transparent)` changes.
- 1.97: v0 symbol mangling by default (relevant only where zustc's objects meet rustc's).
- 1.95: custom JSON target specs no longer accepted on stable.

The `#[core::derive]` row is the same class of problem as TODO.md's "zustc alone still drops an unknown attribute silently": an attribute path that zustc does not know should be an error, not nothing.

Coming next (2026 goals, nightly): reflection (`TypeId::info`, a const-eval-only MVP has landed), return type notation and TAIT, async fn in `dyn` traits, field projections, in-place initialization, explicit tail calls and `loop_match`, open enums, generic integers, the `Try` trait, specialization (a minimal one), and ergonomic ref-counting.

## 5. Where the community disagrees

- **Ergonomic ref-counting.** RFC 3680's `.use` and `Use` trait got mixed feedback. The 2026 goal compares it with a `Share` trait and `move($expr)` closures, aiming at something "low-level enough for a kernel, usable enough for a GUI". The disagreement is whether more required syntax helps at all. No design is accepted yet ([goal](https://goals.rust-lang.org/2026/ergonomic-rc.html)).
- **Polonius Alpha against datalog Polonius.** Alpha gives up programs that datalog Polonius accepted (loops over a reassigned `&mut`), and upstream plans no work after Alpha. zustc today sits on the datalog side for those programs. Following rustc means rejecting them.
- **RDR's priority.** RDR is listed as unfunded, and it was "will not complete" for 2025H2. Commenters doubted progress in early 2026 before the October goal and the open PR.
- **Reflection.** The scope (only const-eval values first, types back into the type system later) and the API name (`TypeId::info` in the goal against `type_info` reported elsewhere) are not settled.
- **Supply chain.**
  - In September 2026 the Rust blog warned of targeted attacks on rust-lang members and owners of popular crates, and `arrayref` was briefly compromised.
  - 1.100 added a minimum publish age setting to Cargo (dependency cooldowns), and "crates.io hardening against supply chain attacks" is a 2026 goal.
  - zurgo reads crates.io sources, so it should follow Cargo's cooldown setting.
- **Outside the compiler:** Ubuntu's move to the Rust coreutils (uutils): licensing (MIT against GPL), compatibility bugs, pace.

## 6. What to do, in order

1. [#101](https://github.com/mizchi/zust/issues/101): parallel `check`. It needs no new analysis, the gain is about the number of cores on cold checks, and the risks (determinism, KPI counting) are known.
2. [#1](https://github.com/mizchi/zust/issues/1): location-sensitive liveness for loans that flow into universal regions, and the Polonius tests in the comparison. Stable rustc is to follow Alpha before the end of 2026.
3. The 1.96–1.100 gaps of section 4. They are small prelude and attribute work: an error for unknown attribute paths, `core::derive`, `Infallible = !`, `Allocator` / `Global`, the new APIs, `core::range`.
4. [#2](https://github.com/mizchi/zust/issues/2): the trait solver, in the six steps of the issue. It is the largest area of tests/ui, and it is where rust-analyzer, blaze and relax already went.
5. Cross-crate reuse of analysis results (zustc's RDR).
6. Layout-keyed sharing of instances, with the upstream polymorphization experiment as the reference.

## 7. Keeping up

- At each rustc release (every six weeks), run the feature probes of section 4 for the new release's stabilizations, and move the tests/ui pin when this repository moves its rustc.
- `bench/rust_ui.py` skips every test with `//@ revisions`, and that is where upstream's new behavior is tested (`nll/polonius`, `traits/next-solver`). Compare those directories with the three compilers of this report until rust_ui.py can pick a revision.
- Watch the 2026 goals' monthly updates on the Rust blog for: RDR, TPDE, the parallel front end, Polonius Alpha, the next solver, polymorphization.

## Sources

- [Rust Project Goals 2026](https://goals.rust-lang.org/2026/goals.html), [Fast Builds roadmap](https://goals.rust-lang.org/2026/roadmap-fast-builds.html)
- [Relink Don't Rebuild tracking issue](https://github.com/rust-lang/rust/issues/158844), [goals #722](https://github.com/rust-lang/goals/pull/722)
- [TPDE goal](https://goals.rust-lang.org/2026/tpde.html), [TPDE paper](https://arxiv.org/abs/2505.22610), [TPDE repository](https://github.com/tpde2/tpde)
- [Polymorphic code generation experiment](https://goals.rust-lang.org/2026/polymorphization.html)
- [Improve cg_clif performance](https://goals.rust-lang.org/2026/improve-cg_clif-performance.html)
- [Parallel front end goal](https://goals.rust-lang.org/2026/parallel-front-end.html), [Phoronix: parallel frontend by default on nightly](https://www.phoronix.com/news/Rust-Nightly-Parallel-Frontend)
- [Enabling Polonius Alpha on nightly](https://blog.rust-lang.org/2026/08/04/enabling-polonius-alpha-on-nightly), [Polonius goal](https://goals.rust-lang.org/2026/polonius.html)
- [Next-generation trait solver goal](https://goals.rust-lang.org/2026/next-solver.html)
- [Reflection and comptime goal](https://goals.rust-lang.org/2026/reflection-and-comptime.html), [Ergonomic ref-counting goal](https://goals.rust-lang.org/2026/ergonomic-rc.html), [RTN goal](https://goals.rust-lang.org/2026/rtn.html)
- Release notes: [1.95](https://blog.rust-lang.org/2026/04/16/Rust-1.95.0/), [1.96](https://blog.rust-lang.org/2026/05/28/Rust-1.96.0/), [1.97](https://blog.rust-lang.org/2026/07/09/Rust-1.97.0/), [1.98](https://releases.rs/docs/1.98.0/), [1.99](https://releases.rs/docs/1.99.0/), [1.100](https://releases.rs/docs/1.100.0/)
- [Be alert: targeted attacks on prominent Rustaceans](https://blog.rust-lang.org/2026/09/17/targeted-attacks/)
- [The acyclic e-graph: Cranelift's mid-end optimizer](https://cfallin.org/blog/2026/04/09/aegraph/)
