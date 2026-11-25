import TraitSolver.Solver

/-!
# An impl's own obligations: its trait's supertraits

`impl<T: B> Sub for H` is well formed when `H: Super` holds for every `T` the impl allows, for each supertrait
`Super` of `Sub` (all the way up). zustc asks it of the impl's parameters made rigid: a substitution `ρ` to fresh
constants (placeholders) that no impl names, with the impl's where clauses under `ρ` as the environment (its bounds
are all it may assume). A `no` there means no proof from what the impl assumes: the obligation fails (E0277).
-/

namespace TraitSolver

open Ty

theorem supertrait_obligation_fails (P : Prog) (c : Clause) (ρ : Nat → Ty) (sup : Ty → Ty) (fuel : Nat) :
    solve P (c.body.map (Ty.inst ρ)) fuel [] (sup (Ty.inst ρ c.head)) = .no →
    ¬ Holds P (c.body.map (Ty.inst ρ)) (sup (Ty.inst ρ c.head)) :=
  solve_no_sound P _ fuel _

end TraitSolver
