# Borrow checking without MIR

Date: 2026-11-18

zustc had no borrow checker: it accepted every program that type checked. This report covers what was chosen,
how it fits a compiler that lowers the AST straight to machine code, and what it does on rustc's own UI tests
([report 07](2026-11-18-07-rustc-ui-tests.md)).

## State of the art

- rustc's default checker is NLL. It infers regions as sets of points from liveness and outlives constraints, and
  reports an access that conflicts with a loan that is still in scope.
- Its successor is Polonius. The 2026 project goal ([Rust project goals: borrow checker
  within](https://goals.rust-lang.org/2026/roadmap-borrow-checker-within.html)) is to stabilize "Polonius alpha" in
  2026. Polonius alpha handles conditional borrows (NLL problem case 3) and lending iterators. Full flow-sensitive
  Polonius is listed as future work.
- [The Polonius book](https://rust-lang.github.io/polonius/rules/loans.html) states the analysis as rules over
  loans and origins:
  - A loan flows into the origin of the reference it creates, and moves along subset relations and CFG edges while
    that origin is live.
  - A loan is live at a point if a live origin contains it.
  - An error is an invalidation of a live loan.
  - The book also describes a location-insensitive variant used as a fast pre-pass. If it finds no possible error,
    the full analysis is skipped.
- [Wu, Wang, Chen (arXiv 2611.06319)](https://arxiv.org/html/2611.06319) show that, when checked lazily, Polonius
  is a forward abstract interpretation, with no separate liveness pass.

## Design

zustc has no MIR. acg (type checking and code generation together) lowers each function body to minclif. The
borrow checker uses the CFG that acg already builds:

1. **Recording.** While lowering, acg records events per minclif block:
   - a local is bound (initialized or not), read, moved, written, or borrowed (`&`, `&mut`, autoref receivers,
     closure captures, `ref` bindings);
   - a local goes out of scope.

   `jump` and `brif` also record the CFG edges. A brif on a constant records only the edge it takes.
2. **Places.** A place is a local plus projections: fields, `*` through a Box, `*` through `&mut`, `*` through `&`,
   and index. Field and deref projections come from the static types, as auto-deref does.
3. **Location-insensitive pre-pass.** The pass ignores control flow. If there is no move, no uninitialized local,
   and no access that overlaps a conflicting loan, the dataflow does not run. Mutability errors that do not depend
   on flow are reported here.
4. **Dataflow.**
   - A backward pass computes the liveness of locals.
   - A forward pass, iterated to a fixpoint, tracks three things: which paths may be moved or uninitialized, which
     immutable locals may already be assigned, and which loans each local may hold.
   - A loan is live where a live local may hold it. This is the Polonius view, with one origin per local.
5. **What flows into a local.** Without lifetimes in the types, this is approximated:
   - A local gets the loans created while evaluating its initializer, plus the loans held by the locals read there.
   - A call passes on the borrow of an argument only if the signature says the result may borrow from it. This is
     decided by the elision rules and named lifetimes in the declaration (`SigSyn::ret_from`).
   - Unknown signatures pass nothing on. These are closures, fn pointers and `impl Trait` returns.
   - Iterator and view types in the prelude (`Iter`, `Chars`, `ChunksExact`, …) hold the loan of what they iterate.

Loans that no local holds die with their expression. This gives two-phase borrows (`v.push(v.len())`) and the
conditional-return case that NLL rejects. The price is that conflicts inside one expression are found only between
the arguments of a call. A hidden local keeps the argument borrows alive until the call is made, so
`f(&mut x, &x)` and `x += x` (with an overloaded `+=`) conflict.

Other rules:

- A loan is killed when the local it goes through is overwritten (Polonius' `loan_killed_at`).
- Rust's evaluation order is followed: for `a = b`, `b` is evaluated first; for `S { f, ..base }`, `base` is
  evaluated last.
- Closures capture places, not variables (RFC 2229: `p.x`, `*r`, the parts a pattern binds). Each place is captured
  by value, `&mut` or `&`, depending on how it is used.
- Calls that acg itself synthesizes are not treated as the user's borrows: the `index_mut` used for reads, and the
  method call an operator is rewritten to.

## Errors found

| code | what |
|---|---|
| E0382 / E0381 | use of a moved / uninitialized value, including moves by patterns, `for` loops and `move` closures |
| E0499 / E0502 / E0503 / E0505 / E0506 | an access that conflicts with a live loan |
| E0597 / E0515 | a borrow that outlives what it borrows: at the end of a scope, carried out of a block, stored behind a parameter, or returned |
| E0507 / E0508 | a move out through a reference or out of an index |
| E0596 / E0594 / E0384 | mutation of a local not declared `mut`, or of a place behind a `&` reference; a second assignment to an immutable local |

Only the first error of each function is reported, since acg stops a function at its first error.
`ZUSTC_BORROWCK=0` turns the checker off, `=1` checks moves only, and `ZUSTC_BK_DEBUG=1` prints the recorded
events.

## Results

The baseline is the commit before (ac2361e). The tests/ui suite is the same 1.100.0 tests/ui as in report 07.

| kind | pass before | pass after | pass rate after |
|---|---:|---:|---:|
| run-pass | 1120 | 1120 | 46.42% |
| check-pass | 834 | 834 | 46.15% |
| build-pass | 65 | 65 | 48.15% |
| compile-fail | 2298 | 2578 | 35.04% (from 31.23%) |

No test in the baseline list regressed. That list has 4597 tests now, from 4317.

A false positive in a test that already failed for another reason would not show up as a regression. A separate
scan of every failing run-pass / check-pass / build-pass test finds no borrow-checker error among them.

Compile-fail tests that pass, by directory:

| directory | before | after | applicable |
|---|---:|---:|---:|
| borrowck | 30 | 159 | 354 |
| nll | 12 | 41 | 187 |
| closures | 21 | 36 | 115 |
| moves | 2 | 21 | 49 |
| liveness | 4 | 9 | 16 |

Cost, measured as instructions for `zustc check` (valgrind) on the generated benchmark programs:

| program | off | on | change |
|---|---:|---:|---:|
| gen1000 | 782.7 M | 800.0 M | +2.2% |
| gen5000 | 3883.9 M | 3971.1 M | +2.2% |

Without the location-insensitive pre-pass the change was +6.3%. Most of what is left is the recording during
lowering, not the analysis.

The crates in cratebench (fnv, adler2, hex, itoa, rustc-hash, cfg-if) still build and give the same output. Two of
them first hit false positives:

- prelude view methods (`ChunksExact::remainder`) were taken to borrow the view;
- a closure that assigned one field of `self` was taken to borrow all of `self` mutably.

## What is left

- Region errors against signatures, such as E0621, E0521, and "lifetime may not live long enough". These need
  universal regions and outlives checking of the body against the declaration, not only loans.
- Precise flow through types. A local holds the loans of everything its initializer read, so a value taken out
  through a call (`v.get(0).cloned()`) still counts as borrowing `v`. To avoid false positives because of this,
  E0515 and the escaping-block case of E0597 look only at what the expression borrows directly.
- E0716 (temporaries dropped while borrowed), E0510, uses by `Drop` at the end of a scope, partial moves out of
  arrays and struct patterns, and more than one error per function.
- `tests/selfhost_compiler.sh` was not run. It does not get past a parse error in the generated single-file
  compiler, and it does not at the commit before either.
