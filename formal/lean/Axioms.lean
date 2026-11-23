import TraitSolver
-- the theorems rest on Lean's standard axioms only (no `sorry`): `lake env lean Axioms.lean`
#print axioms TraitSolver.solve_yes_sound
#print axioms TraitSolver.solve_no_sound
#print axioms TraitSolver.solve_complete
#print axioms TraitSolver.solve_no_sound_weaken
#print axioms TraitSolver.mayUnify_sound
#print axioms TraitSolver.mayUnify_complete
#print axioms TraitSolver.select_unique
#print axioms TraitSolver.conflict_sound
#print axioms TraitSolver.no_conflict_of_not_mayUnify
#print axioms TraitSolver.supertrait_obligation_fails
#print axioms TraitSolver.csolve_yes_sound
#print axioms TraitSolver.csolve_no_sound
#print axioms TraitSolver.normalize_sound
#print axioms TraitSolver.normTo_functional
#print axioms TraitSolver.binding_no_sound
