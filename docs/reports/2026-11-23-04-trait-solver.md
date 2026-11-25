# A trait solver for the program's traits, modeled and proved in Lean first

Date: 2026-11-23

zustc decided trait goals with string lookups: `Program::implements` answered "yes" for any type once a trait had a
blanket impl (`impl<T> Foo for T where T: Bar` made every type `Foo`), the trait arguments of a bound were not read,
and only the well-formedness check of struct fields followed impls' bounds (`bound_holds`, with "deeper than 16
counts as holding"). This work states what a trait goal holding means, writes the algorithm against it in Lean 4
with proofs, and then implements that algorithm in zustc.

- Model and proofs: [formal/lean/](../../formal/lean/README.md) (`formal/run.sh lean`)
- Implementation: [crates/sema/src/traits.rs](../../crates/sema/src/traits.rs)

## The model

Types are trees (`var` for an impl's parameter, `con`, `app`), and a goal `T: Tr<A>` is the tree `Tr T A`, so an
impl is a Horn clause: its head (self type and trait arguments) and its body (the bounds of its parameters, where
clauses included). The specification `Holds` is the least set of goals closed under the impls and the item's own
assumptions (a placeholder's bounds); it is inductive, so `impl<T: B> A for T` with `impl<T: A> B for T` proves
neither (rustc: E0275).

`solve` is the algorithm: try each impl whose head matches the goal (one-way matching), prove its where clauses
depth first with less fuel, and keep the goals being proved on a stack; a goal that comes back round fails that
candidate. It answers `yes`, `no` or `overflow` (out of fuel; or a where clause names a parameter the head does not
bind, so the subgoal is not ground).

| theorem | meaning for zustc |
|---|---|
| `solve_yes_sound` | `yes` only for goals that hold |
| `solve_no_sound` | `no` only for goals that do not hold: an E0277 reported on `no` is never a false error |
| `solve_complete` | with enough fuel a goal that holds gets `yes` (impls whose where clauses only name parameters their heads bind) |
| `solve_no_sound_weaken` | leaving out of an impl what zustc cannot read keeps `no` sound (see below) |
| `mayUnify_sound` / `mayUnify_complete`, `select_unique` | overlap of impl heads (E0119): exact for linear heads; heads that do not unify never compete |

The step that needs care is the stack. `no` from a cycle is sound because a proof can always be cut so that no goal
is proved twice on one path (`HA_cut`, by strong induction on the height of the proof): a candidate that only leads
back to a goal being proved loses no proof. Everything is in core Lean (no Mathlib); `Axioms.lean` checks that the
theorems use only Lean's standard axioms (no `sorry`). `Examples.lean` runs the model on the tests/ui cases below
(`by decide`).

## Weakening: what zustc cannot read

zustc's summaries do not hold everything an impl says. The implementation never guesses: a where clause it cannot
parse (an associated type's bound, a bound on a concrete type) is left out of the body; a self type or trait argument
it cannot resolve matches anything; an associated type in a head matches anything; and a goal's `i32` / `f64`, which
may be a literal zustc defaulted before an impl could choose its type (`f(0, 2)` with `impl A<isize> for isize`),
matches any integer / float. Each makes zustc's clause *cover* the real impl, and `solve_no_sound_weaken` proves
that a `no` of the weakened program is a `no` of the real one. A placeholder or trait object whose `no` might be
explained by bounds zustc does not read gives `overflow` instead.

`overflow` is "cannot tell", and zustc accepts.

## The implementation

`TraitDb::build` turns the program's impls of its own traits into clauses (once, in `Program::build_with`).
`Cx::solve` is `solve` of the model: the same `and` / `or` tables (a unit test checks them case by case), exact
matching (`&mut T` is not `&T`, array lengths agree; `match_ty` is lenient for inference and is not used), fuel 48
and a budget of 20,000 goals per question. Traits that are not the program's go to `Program::implements`, whose "no"
is believed only for scalars and plain structs and the prelude's traits, as before.

It is used in:

- `Insts::request` (every generic call's bounds): a `no` is E0277. When the goal names no placeholder, it holds
  whatever the caller's type parameters are, so it is also reported from a generic fn nothing calls (marked
  `traits::SOLVED_NO`, which the placeholder round lets through);
- `bound_holds` (statics' and fields' bounds), which is now `holds != No`;
- `wf_error`, extended to impl headers' trait arguments (the types in them, and the bounds the trait puts on its
  parameters: `trait Getter<T: Clone2>` with `impl Getter<isize> for isize`) and to free fns' signatures
  (`fn explode(x: Foo<u32>)` with `struct Foo<T: Trait>`).

## Results

tests/ui (rust-lang/rust 1.100.0), against main:

| kind | before | after |
|---|---:|---:|
| run-pass | 1832 | 1832 |
| check-pass | 1512 | 1512 |
| build-pass | 102 | 102 |
| compile-fail | 4497 | 4509 |

No test that passed fails. The 12 compile-fail tests that pass now, each at the line and for the reason it tests:
`traits/inductive-overflow/{simultaneous,supertrait,two-traits}.rs` (cycles), `traits/issue-18400.rs`,
`traits/next-solver/diagnostics/where-clause-doesnt-apply.rs`, `errors/trait-bound-error-spans/blame-trait-error.rs`,
`suggestions/issue-84973-2.rs`, `typeck/issue-90804-incorrect-reference-suggestion.rs` (where clauses of blanket
impls), `traits/impl-bounds-checking.rs`, `trait-bounds/super-assoc-mismatch.rs`,
`traits/bound/on-structs-and-enums-in-impls.rs` (impl headers' trait arguments),
`traits/bound/on-structs-and-enums-in-fns.rs` (fn signatures).

Two regressions found on the way and fixed before the numbers above: a bound's arguments name the callee's own
parameters (`U: Eq<T>` resolved `T` as a struct of that name), and a generic struct's base without its arguments is
not a ground type.

New tests: `tests/compile_errors/trait_solver_*.rs`, `examples/trait_solver.rs` (expected output from rustc),
`tests/check.rs::trait_solver_no_in_uncalled_generic_fn`, `traits::tests::and_or_tables`.

## What is left

- Coherence is proved in the model (`mayUnify`) but zustc's E0119 checks are still the old ones.
- Auto traits (coinductive) are not in the model; zustc keeps its structural `auto_trait`.
- Associated types: a bound on one (`where <T as Mirror>::Mirror: Foo`) is left out (weakened), and
  normalization is not modeled.
- Supertraits as obligations of an impl (`impl<T> Complete for T` needs `T: Partial`, traits/issue-43784-supertrait.rs)
  and E0277 at dyn coercions (`traits/coercion-generic-bad.rs`) are not checked yet.
- Method selection between impls of one trait with different arguments (`impl Conv<u64> for u8` and
  `impl Conv<bool> for u8`) picks the wrong one; this is method lookup, not the solver.
- `tests/selfhost.sh` already fails on main (the build scripts do not include `regions`); `traits.rs` is added to
  the program / compiler scripts so it is in place when they are repaired.
