# Region (lifetime) checking: approach and first version (issue #1)

Date: 2026-11-20 (the plan, in Japanese: docs/ja/reports/2026-11-20-01-region-check-plan.md)

## Where the cost would be

These are callgrind instruction counts, before the change.

- **gen5000 `check`:**
  - per-function acg (lowering and checking): 60%
  - `analyze` (parsing, summaries, prep): 35%
  - `Bk::check`: under 0.1%, because its location-insensitive pre-pass stops early
- **patterns `check`:** `Bk::check` is 2.7%.

Function results are cached, so an edit re-checks only the edited function.

So the hypothesis was that the cost of region checking is in **collecting** constraints over every expression, not in
solving them, and that the design should follow from that:

- **Do not change `Ty`.** It is interned and hashed into instance keys and the cache, so adding regions to it would
  move every KPI. Region information sits beside it as *region slots*: one per reference and per lifetime parameter
  of an ADT, in preorder.
- **Gate first.** A function with fewer than two regions in its signature, and no named lifetime, is not checked.
- **No control flow for errors against signatures.** NLL's constraints between regions hold at every point, and a
  universal region holds the whole body. So "lifetime may not live long enough" is reachability in the constraint
  graph from each universal region (rustc's `check_universal_regions`). Polonius alpha builds its graph lazily, and
  that is the same direction.
- **Make the incremental check correct.** The regions of a signature must be in the cache key.

## What was built

- **Summaries (`crates/syntax/src/regions.rs`):** region slots of signatures, impl/trait headers and struct/enum
  fields, written as codes: a lifetime index, `'static`, elided, an ADT's lifetimes left out, or a type that is not
  followed. Nothing is kept when no lifetime is named. `Program::hash_sig` hashes them, together with `ret_from`.
- **Syntax-only checks:**
  - E0261 in methods, impl headers and `T: 'x` (the `for<'a>` binders of where clauses are kept apart)
  - E0496
  - E0207 for impl lifetimes that only associated types use
  - E0106 in fn pointer types and `Fn() -> &T` bounds
- **`crates/typeck/src/acg/regionck.rs`:**
  - **Region trees:** values get trees: parameters, `let`, assignments, calls (the callee's generic signature with
    fresh regions, related by variance; an ADT's variance is computed when first needed), struct literals, variant
    values, pattern bindings, and returned values.
  - **The check:** universal regions, their implied bounds (`&'a T` and fields' inferred outlives) and declared
    bounds; reachability from each universal region.
  - **Borrows that must outlive the function:** a borrow of a local the function owns, or of a temporary, that must
    outlive a universal region is reported too (E0597).
  - **Not followed:** anything else has no tree, and adds no constraint.
- **Temporaries (E0716):** a temporary passed by `&` straight to the outermost call of a `let` initializer is a hidden
  local that goes out of scope at the end of the statement, in the existing borrow checker's dataflow.

## Results

tests/ui: 0 regressions on run-pass, check-pass and build-pass. compile-fail went from 3578 to 3665.

KPIs, instruction counts (`bench/kpi_track.py --dry-run --lsp`, same machine), cd5dccd and then this change:

| KPI | cd5dccd | with the region check | change |
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

The largest increase is on patterns, which writes the most lifetimes; about 0.5% of it is the region check itself.
The rest is summary-time work. On the way, two costs had to be removed: a `getenv` per function (+5.7M instructions
on gen5000) and copies of the larger summary structs (now boxed). blaze is not installed here, so `bench/vs_blaze.py`
was not re-run. The margins in the README are 6–61x, against instruction changes below 1%.

Not yet:
- closures
- the lifetimes of `impl Trait` and `dyn`
- `T: 'a` (E0310)
- higher-ranked bounds
- temporaries other than those passed by `&` straight to a `let` initializer's outermost call
- the 158 failing lifetime tests whose functions have fewer than two regions in their signatures (`main` with a
  `'static` requirement, for example)
