import TraitSolver.Solver

/-!
# The tests/ui cases the solver was written for, as goals the model answers

`T: Tr` is `app (con "Tr") T`.
-/

namespace TraitSolver.Examples

open Ty

def tr (n : String) (t : Ty) : Ty := app (con n) t
def unit : Ty := con "()"
def i32 : Ty := con "i32"

/-- traits/next-solver/diagnostics/where-clause-doesnt-apply.rs: `impl<T> Foo for T where T: Bar`, no `Bar` impl:
`(): Foo` is `no` -/
def whereClause : Prog := [⟨tr "Foo" (var 0), [tr "Bar" (var 0)]⟩]
example : solve whereClause [] 8 [] (tr "Foo" unit) = .no := by decide

/-- traits/inductive-overflow/simultaneous.rs: `impl<T: Tweedledum> Tweedledee for T` and the reverse, and
`impl<T: Tweedledee + Tweedledum> Combo for T`: `i32: Combo` is a cycle, `no` (rustc: E0275) -/
def simultaneous : Prog :=
  [⟨tr "Dee" (var 0), [tr "Dum" (var 0)]⟩, ⟨tr "Dum" (var 0), [tr "Dee" (var 0)]⟩,
   ⟨tr "Combo" (var 0), [tr "Dee" (var 0), tr "Dum" (var 0)]⟩]
example : solve simultaneous [] 8 [] (tr "Combo" i32) = .no := by decide

/-- with an impl to end the chain, the same goal holds -/
example : solve (⟨tr "Dum" i32, []⟩ :: simultaneous) [] 8 [] (tr "Combo" i32) = .yes := by decide

/-- a placeholder's bound is the environment: `fn f<P: Bar>` proves `P: Foo` -/
example : solve whereClause [tr "Bar" (con "P#")] 8 [] (tr "Foo" (con "P#")) = .yes := by decide

/-- out of fuel is `overflow`, not `no`: `impl<T: Tr> Tr for Box<T>` on `Box<Box<..>>` deeper than the fuel -/
def boxes : Prog := [⟨tr "Tr" (app (con "Box") (var 0)), [tr "Tr" (var 0)]⟩, ⟨tr "Tr" i32, []⟩]
example : solve boxes [] 2 [] (tr "Tr" (app (con "Box") (app (con "Box") (app (con "Box") i32)))) = .overflow := by decide
example : solve boxes [] 5 [] (tr "Tr" (app (con "Box") (app (con "Box") (app (con "Box") i32)))) = .yes := by decide

/-- a where clause naming a parameter the head does not bind: `overflow` -/
example : solve [⟨tr "Foo" (var 0), [tr "Bar" (var 1)]⟩] [] 8 [] (tr "Foo" unit) = .overflow := by decide

end TraitSolver.Examples
