# formal/lean: the trait solver, in Lean 4

A model of how zustc decides whether a trait goal `T: Tr<A..>` holds by the program's impls, with proofs of what
its answers mean. The implementation that follows it is [`crates/sema/src/traits.rs`](../../crates/sema/src/traits.rs).
Report: [docs/reports/2026-11-23-04-trait-solver.md](../../docs/reports/2026-11-23-04-trait-solver.md).

Core Lean only (no Mathlib), Lean 4.15 (`lean-toolchain`).

```bash
formal/run.sh lean
```

(it runs `lake build` here, then `lake env lean Axioms.lean`, and fails if a theorem depends on `sorryAx`)

| file | what |
|---|---|
| `TraitSolver/Ty.lean` | types as trees (`var` / `con` / `app`), substitutions, one-way matching `mtch`: sound (`mtch_sound`), complete (`mtch_complete`) |
| `TraitSolver/Solver.lean` | impls as Horn clauses, the specification `Holds` (inductive), the algorithm `solve` (depth first, fuel, the stack of goals being proved), and its theorems |
| `TraitSolver/Weaken.lean` | why zustc may leave out what it cannot read: a weaker program's `no` is the real program's `no` |
| `TraitSolver/Coherence.lean` | overlap of impl heads (E0119): `mayUnify`, exact for linear heads; unique selection; when a conflict is a real one (`conflict_sound`) |
| `TraitSolver/Obligations.lean` | an impl's supertrait obligations, under its own bounds |
| `TraitSolver/Coinductive.lean` | auto traits (`Send` / `Sync`): `csolve`, where a goal that comes back round holds, against the greatest fixpoint |
| `TraitSolver/Examples.lean` | tests/ui cases as goals the model answers (`by decide`) |
| `Axioms.lean` | `#print axioms` of the main theorems |

## The theorems

| theorem | statement |
|---|---|
| `solve_yes_sound` | `solve P env fuel S g = yes → Holds P env g` |
| `solve_no_sound` | `solve P env fuel [] g = no → ¬ Holds P env g` |
| `solve_complete` | for impls whose where clauses name only parameters their heads bind, a ground goal that holds gets `yes` with enough fuel |
| `solve_no_sound_weaken` | if every impl of `P` is covered by one of `P'` (`Prog.weakens`), `solve P' .. = no → ¬ Holds P ..` |
| `mayUnify_sound` / `mayUnify_complete` | heads that `mayUnify` rejects have no common instance; linear heads it accepts have one |
| `select_unique` | impls whose heads pairwise do not unify never both apply to a goal |
| `conflict_sound` / `no_conflict_of_not_mayUnify` | heads made equal by some substitutions, under which the solver proves both impls' where clauses, give a goal both impls apply to (zustc's E0119); heads that do not unify never conflict |
| `supertrait_obligation_fails` | an impl's parameters made rigid, its where clauses the environment: a `no` for a supertrait is a failed obligation |
| `csolve_yes_sound` / `csolve_no_sound` | the coinductive solver's `yes` only for goals in a consistent set, its `no` only for goals in none |

`solve_no_sound` is the property zustc relies on: an E0277 is reported only on `no`, so it is never a false error with
respect to the impls zustc read. The stack is what makes `no` from a cycle sound: `HA_cut` shows that a proof can
always be cut so that no goal is proved twice on one path, so a candidate that leads back to a goal being proved
loses nothing.

## What the model does not cover

- associated types and their normalization (`<T as Tr>::X`), and higher-ranked bounds (`for<'a>`);
- lifetimes (zustc's `Ty` erases them; region checking is separate).
