# Region (lifetime) checking: approach and steps (issue #1)

Date: 2026-11-20

## 1. Measurements before the change

- All counts below are instructions, counted by callgrind.
- **gen5000 `check`: 3793M.** The breakdown (inclusive):
  - per-function acg (`compile_id_req`): 60%
  - `analyze` (parsing, summaries, prep): 35%
  - `Bk::check`: under 0.1%. Its location-insensitive pre-pass stops it early.
- **patterns `check`: 685M.** The breakdown:
  - `compile_id_req`: 56%
  - `Bk::check`: 2.7%
  - `bk_scrutinee`: 1.1%
  - The whole borrow checker costs about what the README says ("+2.2%").
- **How check works:**
  - It processes functions one at a time, wave by wave.
  - Each function's result stays in the check memo (`driver2.rs`) and the cache, so editing one function re-checks only that function.
  - build runs in parallel, in forked child processes (`compile_many`).
- **Types and the lifetimes of signatures:**
  - `Ty` carries no regions.
  - Of a signature's lifetimes, only `SigSyn::ret_from` is kept: a bit mask of which arguments the return value may borrow from.
  - So **the interface fingerprint (`iface_fp`) does not include lifetimes**. For example, changing `-> &'a T` into `-> &'static T` does not get the callers checked again.
  - That is no problem today (nothing checks them), but with a region check it becomes a correctness problem.
- **tests/ui compile-fail tests with region-related annotations that do not pass today:**

| category | not passing (fail + wrong line) | without ADT lifetimes, HRTB, closures, dyn/impl |
|---|---:|---:|
| lifetime may not live long enough | 189 | 44 |
| does not live long enough (E0597) | 89 | 30 |
| temporaries (E0716) | 55 | 37 |
| `T: 'a` (E0310/E0309) | 50 | 10 |
| E0621/E0623/E0700 and the like | 24 | 3 |
| E0515 / E0373 / E0478 / E0521 / E0713 | 25 / 12 / 11 / 1 / 3 | - |
| syntax only (E0261 undeclared, E0106 missing, E0195/E0207/E0263/E0496 and the like) | 18 / 21 / 31 | - |

- Two things follow from the table:
  - Without the lifetime parameters of ADTs (`struct S<'a>`), only about a quarter of them can be reached.
  - About 70 need only the syntax, at almost no cost.

## 2. What recent research and implementations suggest

- **NLL's (rustc) check of universal regions:**
  - A region's value is "a set of CFG points + end('a)".
  - It propagates `'a: 'b` constraints, collapsed into SCCs, and `check_universal_regions` looks for a required `'a: 'b` that is not declared.
  - A universal region contains every point of the function body anyway. So the checks against a signature ("lifetime may not live long enough" and the like) **are decided by location-insensitive reachability alone**.
  - [rustc-dev-guide: region inference](https://rustc-dev-guide.rust-lang.org/borrow-check/region-inference.html)
- **Polonius alpha (the 2025H2–2026 project goals):**
  - It finds whether a loan is live by reachability, in a constraint graph whose nodes are (region, point).
  - The graph is not materialized (**lazy**). Constraints that hold everywhere are kept as "logical edges" and expanded while traversing. A region's liveness, too, is computed the first time a traversal touches it.
  - It builds no graph for a function with no loans. The number of loans is a single digit in 90% of the benchmarks.
  - The goal is "up to +10–20% over NLL".
  - [goals 2026/polonius](https://goals.rust-lang.org/2026/polonius.html)
  - [rustc_borrowck::polonius](https://doc.rust-lang.org/nightly/nightly-rustc/src/rustc_borrowck/polonius/mod.rs.html)
- **How this relates to zust's borrow checker:**
  - It already works the Polonius way: one origin per local, and a loan is live by the liveness of locals.
  - What is missing is only **the universal regions (the signature's side) and the lifetime parameters of ADTs**.

## 3. Hypotheses on where the cost would be

- **H1 Most of the cost is in *collecting* constraints, not in solving them.**
  - acg walks every expression, so the cost of hooks that collect constraints is proportional to the number of expressions.
  - Solving is a small graph per function (SCCs and reachability, O(edges)).
  - → Stop collecting at a gate (H2). Solve only once, at the end of the function.
- **H2 A function without types that hold regions can cost nothing.**
  - If neither the signature nor the types of locals have references or ADTs with lifetime parameters, no check of universal regions is needed. This is decided once, at summary time.
  - Almost all of gen5000 and crates stop at the gate; patterns and traits pay the cost.
- **H3 Adding regions to `Ty` would make every KPI worse.**
  - `Ty` is used in instance keys, in the encoding of the cache, and in hashes.
  - So, as rustc erases regions for codegen, `Ty` stays erased, and only the check keeps **a "shadow" of regions (region slots) beside it**.
  - Region slots are the positions of the references and of ADT lifetime parameters in a type, in order.
  - → The no-op build, the cache size and the edit KPIs cannot move, by construction.
- **H4 Errors against universal regions need no location-sensitive check.**
  - That covers most of may-not-live, E0310, E0621, E0478, and E0597 where `'static` is required.
  - Only local loans (the existing borrow checker) and temporaries (E0716, a hidden local that dies at the end of the statement) need locations. Both fit the existing dataflow.
- **H5 The variance of ADTs is computed once, at summary time, and cached.**
  - It costs only on a cold build, in proportion to the number of ADTs.
- **H6 The regions of a signature go into `SigSyn`.**
  - Without them, incremental builds are wrong (§1).
  - They are empty for fns without lifetimes, so summaries and the cache do not grow.
- **H7 LSP rides on the per-function memo.**
  - What one edit adds is the region check of that function only.

### On a separate multi-threaded path

- **Option A: solve the region check on another thread.**
  - Solving is a few percent of the total (H1), so handing work between threads would cost more. Not taken.
- **Option B: run check's per-function acg in parallel with `std::thread::scope`.**
  - The functions of a wave are independent. `Insts` is already a `Mutex`. The only other state is thread_local.
  - It helps a cold check and the first LSP check, that is, wall-clock time (vs_blaze).
  - cachegrind counts the instructions of all threads, so the instruction-count KPIs do not improve.
  - So it goes last, as a path independent of the region check, and is done only if the margin of acceptance criterion 1 (faster than blaze) shrinks.

## 4. Approach

Each step is measured before it is kept. Each reports an error only when it is certain; where precision falls short, the program is accepted silently. Regressions on run-pass and check-pass stay at 0.

- **R0 Groundwork**
  - The regions of signatures (into `SigSyn`, for the interface fingerprint and correct incremental builds).
  - ADT lifetime parameters, each field's region slots and variance in the summaries.
  - A mark for the gate.
- **R1 Checks on the syntax alone** (at summary time, almost no cost): E0261, E0106, E0263/E0496/E0195 and the like.
- **R2 Check of universal regions** (location-insensitive, per function):
  - One origin per local, today's granularity, is not enough.
    - `fn foo(x: &mut Vec<&u8>, y: &u8) { x.push(y) }` needs `'y: 'x's element` through the `T` of `Vec<T>::push` (invariant, under `&mut`).
    - The typical tests/ui test has this shape.
  - So, only in the functions that pass the gate, **each value of an expression gets a region tree**.
    - A region tree puts region variables on the sequence of a type's region slots.
    - A call instantiates the callee's generic signature with fresh variables, and puts outlives constraints between them and the arguments' trees, following variance.
    - A construct that is not understood is Unknown, and constraints touching Unknown are dropped. That way there are no false errors.
  - It finds SCCs and reachability; when an `'a: 'b` that is neither declared nor implied is required, it reports "lifetime may not live long enough".
  - When a local's loan flows into `'static` or a universal region, it reports E0597.
- **R3 Temporaries (E0716):** a hidden local that dies at the end of the statement, in the existing dataflow.
- **R4 `T: 'a` of generics (E0310/E0309), the lifetimes of `impl`, some closures and HRTB.**
- **R5 (only if needed)** check's per-function work in parallel (option B).

## 5. Steps for each stage

1. Collect the tests/ui tests concerned with `--filter`, and put small reproductions in tests/: both ones that must be errors and ones that must not.
2. Implement.
3. Check:
   - `cargo test --release`
   - `python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check`, with 0 regressions.
   - The gain in compile-fail: the tests newly passing, and the entries `bench/rust_ui_accidental.txt` can lose.
4. Run `python3 bench/kpi_track.py "label" --dry-run`, with `--lsp` at milestones. Compare with the row before, and write down why what moved moved.
5. When a cost misses its hypothesis, find the cause with callgrind.
6. Commit and push.

## 6. Results (first version)

### What was built

- **R0**
  - The region slots of signatures, impl/trait headers and struct/enum fields are in the summaries (`crates/syntax/src/regions.rs`).
  - They are empty for items that write no lifetime.
  - The per-function cache key (`hash_sig`) has the regions and `ret_from`. The callers of a callee whose lifetimes alone changed are now checked again.
- **R1**
  - E0261 is wider: methods, impl headers and `T: 'x`; the `for<'a>` of where clauses are kept apart.
  - E0496, E0207 (impl lifetimes only associated types use) and E0106 (fn pointer types and `Fn() -> &T`) are added.
- **R2** (`crates/typeck/src/acg/regionck.rs`)
  - NLL's check of universal regions, with no control flow.
  - The values followed: parameters, let, assignments, calls, struct literals, variant values, pattern bindings, returned values. It applies to generic functions' instances too.
  - When a borrow of a local the function owns, or of a temporary, reaches a universal region, it is reported as E0597.
- **R3**
  - A temporary passed by `&` straight to the outermost call of a let initializer is a hidden local that dies at the end of the statement (E0716).

### tests/ui (0 regressions)

- compile-fail went from 3587 to 3674 (+87; the counts after the rebase onto main's 98d5167).
- Nearly all of the gain is "lifetime may not live long enough" and E0261/E0106/E0207/E0496/E0716.

### KPIs (instruction counts)

Measured with `bench/kpi_track.py --dry-run --lsp` on the same machine. The baseline is cd5dccd, before the rebase.

| KPI | cd5dccd | this change | change |
|---|---:|---:|---:|
| no-op build (M) | 0.706 | 0.702 | ±0 |
| edit one fn (M) | 539.6 | 541.1 | +0.28% |
| check (M) | 3808.3 | 3813.6 | +0.14% |
| cache (KiB) | 1902.8 | 1902.8 | ±0 |
| LSP session (G) | 10.906 | 10.912 | +0.06% (run-to-run noise ~0.1%) |
| traits check (M) | 1161.7 | 1165.9 | +0.36% |
| traits edit (M) | 61.00 | 61.05 | +0.08% |
| crates no-op (M) | 9.88 | 9.87 | ±0 |
| crates edit (M) | 312.3 | 312.9 | +0.19% |
| patterns check (M) | 688.0 | 694.2 | +0.91% |
| patterns edit (M) | 255.8 | 257.3 | +0.59% |

- The largest increase is on patterns, which writes the most lifetimes. About 0.5% of it is the region check itself; the rest is summary-time work.
- blaze is not installed in this environment, so `bench/vs_blaze.py` was not run again. The margins in the README are 6–61x, and the instruction counts changed by less than 1%.

### How the hypotheses held

| hypothesis | result |
|---|---|
| H1 | Held. Of the increase on patterns, collecting is about 0.5%, and solving a part of that. |
| H2 | Held. gen5000 pays +0.1%, for the gate's decision alone. |
| H3 | Held. `Ty` was not changed, so the no-op build and the cache size did not move. |
| H6 | Held. Without the regions in `hash_sig`, the incremental check was wrong (`tests/regionck.rs`). |
| H5 | Changed. Not at summary time: computed lazily, only for the ADTs the check reaches. |

- What was not expected:
  - A `getenv` per function cost +5.7M instructions.
  - The larger summary structs made more copies (now boxed).
- **Multi-threading (R5):** not done, since it does not improve the instruction-count KPIs, and given the margin over blaze (6–61x).

### Not covered yet

- closures
- the lifetimes of `impl Trait` and `dyn`
- `T: 'a` (E0310)
- higher-ranked bounds
- temporaries other than those passed by `&` straight to the outermost call of a let initializer
- errors in functions with fewer than two regions in their signature (for example a `main` that makes a call requiring `'static`). 158 of the failing lifetime tests are of this kind
