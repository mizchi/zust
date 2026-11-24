# Trait solving, step 1: a goal's answer is yes, maybe or no (#2)

Date: 2026-11-24

[#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011) plans six steps toward the design of rustc's
next trait solver. Its step 1 is goals as data with a certainty: the answer to `T: Trait` is yes, maybe (ambiguous or
overflow) or no, not `bool`. The previous step ([2026-11-24-05](2026-11-24-05-impl-bounds-nested-goals.md)) made an
impl's bounds nested goals, with a cycle or an overflow taken to hold. This step gives those two their own answer,
and uses it once: a bound whose goal needs itself is E0275, as in rustc.

## What changed

- **`Certainty`** (`crates/sema/src/program.rs`): `Yes`, `Maybe(Cycle | Overflow)`, `No`, with `or` / `and` and
  `any` / `all` over candidates and over bounds. `Yes` wins over `Maybe` over `No` among candidates, and the other way
  round among bounds.
- **`Program::evaluate(t, tr) -> Certainty`** answers a goal. `implements` is `evaluate != No`, so every caller of
  `implements` sees what it saw before: `Maybe` holds.
- The nested goal (`goal`, formerly `goal_holds`) gives `Maybe(Cycle)` for a goal met again while it is being proved
  and `Maybe(Overflow)` for one past 64 deep. Blanket impls (`blanket_certainty`) and struct and tuple impls
  (`impl_certainty`) combine their bounds' answers.
- Only the goals of the program's traits carry `Maybe`. The rest of `implements` is written case by case and gives
  `Yes` or `No`. A struct or tuple impl's answer comes out of `implements_concrete` as `bool`, and is passed to
  `evaluate` beside it (`impl_applies_noted`): that answer is `implements_concrete`'s own return value, so nothing
  runs in between. A `Maybe` lost on other paths (a derived impl's or a `Box`'s arguments) becomes `Yes`, which
  accepts, as before.
- **E0275**: when an instance's bound (`Insts::request`) is `Maybe(Cycle)`, zustc reports "overflow evaluating the
  requirement" (E0275). A program trait's goals are inductive, so a goal that needs itself does not hold. The error
  is at the line rustc reports.
- `Maybe(Overflow)` is still accepted. rustc also gives E0275 there, but at its own recursion limit (128 by default),
  not at 64, so an error here could be a false one.
- An impl whose bound is its own trait (`impl<T: P> P for T`) is a goal too now, and so a cycle; before, that bound
  was skipped.

```rust
trait P {}
trait Q {}
impl<T: Q> P for T {}
impl<T: P> Q for T {}
fn need<X: P>(_: X) {}
fn main() { need(1u8); } // E0275: overflow evaluating the requirement `u8: P`
```

Cycles that are not errors stay accepted, each checked against rustc 1.97 (`tests/check.rs` `goals_have_a_certainty`):

- the same impls with no obligation through them;
- a generic fn's own bound (`fn g<Y: P>(y: Y) { need(y) }`: a where clause, not followed);
- struct impls through each other that end at a concrete impl (`W<u8>: P` by `u8: Q`);
- a recursive type list (`impl<H: P, T: P> P for (H, T)`);
- two blanket impls of one trait where one holds and the other's goals cycle.

## Measured

`bench/rust_ui.py --check`, on main (d6202b6) merged in:

- compile-fail 4628 → 4635 (+7) against main. Three are this step's:
  - `traits/inductive-overflow/simultaneous.rs`
  - `traits/inductive-overflow/supertrait.rs`
  - `traits/inductive-overflow/two-traits.rs`

  Each is rejected with E0275 at the line the test marks. The other four are the previous step's.
- run-pass, check-pass and build-pass unchanged, 0 regressions. The baseline and the ratchet were rewritten.
- `traits/inductive-overflow/lifetime.rs` is still accepted: its cycle goes through `where T::P: NotAuto`, a bound on
  an associated type, which is step 3.

Instructions of `zustc check -j 1` (cachegrind), the previous commit (ef23222d) against this change:

| | before | this | change |
|---|---:|---:|---:|
| gen5000 | 3802.12 M | 3802.10 M | -0.00% |
| patterns (`mix 8`) | 1354.96 M | 1355.02 M | +0.00% |
| traits (`gen_traits.py 300`) | 10096.80 M | 10097.77 M | +0.01% |

`cargo test --release --workspace` passes, apart from the two tests of `crates/analyzer/tests/lsp.rs` that fail on
main too in this environment.

## Left

- Goals are still `(Ty, trait name)`. A goal as data with typed trait arguments is what the two false accepts left
  from the previous step need: a blanket impl's trait arguments (`Tr<u8>` against `Tr<u16>`), and where clauses on
  other than a parameter (`where W<T>: Bar`).
- `Maybe` does not reach inference yet (step 4). The "cannot infer type parameter" false errors of
  `traits/next-solver` need a stalled goal to be retried, not a `bool`.
- Auto traits are not goals, so there is no coinductive cycle yet (step 5). `Send` and `Sync` are structural
  (`auto_trait`), which already accepts a recursive type.
