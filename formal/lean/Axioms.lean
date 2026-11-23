import TraitSolver
-- the theorems rest on Lean's standard axioms only (no `sorry`): `lake env lean Axioms.lean`
#print axioms TraitSolver.solve_yes_sound
#print axioms TraitSolver.solve_no_sound
#print axioms TraitSolver.solve_complete
#print axioms TraitSolver.solve_no_sound_weaken
#print axioms TraitSolver.mayUnify_sound
#print axioms TraitSolver.mayUnify_complete
#print axioms TraitSolver.select_unique
