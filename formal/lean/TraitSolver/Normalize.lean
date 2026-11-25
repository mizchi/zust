import TraitSolver.Coherence

/-!
# Associated types: normalization

An impl may define associated types (`impl Tr for W<i32> { type Assoc = u32; }`). `<T as Tr>::Assoc` normalizes
to `u` when an impl of `Tr` applies to `T` (its head matches and its where clauses hold) and defines `Assoc` as
something whose instance is `u`: `NormTo`.

`normalize` is the algorithm: the first impl whose head matches the goal and whose where clauses the solver proves,
and its definition under what matching found.

* `normalize_sound`: what it returns is a normal form;
* `normTo_functional`: with coherent impls (heads that unify are one impl) whose definitions name only parameters
  the head binds, a goal has one normal form;
* `binding_no_sound`: so when `normalize` gives `u` and a bound asks `Assoc = x` with `x ≠ u`, the bound does not
  hold (E0271, `type mismatch resolving <W<i32> as Tr>::Assoc == ()`).
-/

namespace TraitSolver

open Ty

/-- an impl with its associated types' definitions -/
structure AImpl where
  cl : Clause
  assoc : List (String × Ty)

abbrev AProg := List AImpl

def AProg.clauses (Ps : AProg) : Prog := List.map AImpl.cl Ps

/-- The specification: `g`'s associated type `n` is `u` -/
def NormTo (Ps : AProg) (env : List Ty) (g : Ty) (n : String) (u : Ty) : Prop :=
  ∃ a ∈ Ps, ∃ σ : Nat → Ty, Ty.inst σ a.cl.head = g ∧ (∀ w ∈ a.cl.body, Holds Ps.clauses env (Ty.inst σ w)) ∧
    ∃ v, a.assoc.lookup n = some v ∧ Ty.inst σ v = u

/-- one impl as a candidate: its definition of `n` under what matching found, when the where clauses hold -/
def normOne (Ps : AProg) (env : List Ty) (fuel : Nat) (g : Ty) (n : String) (a : AImpl) : Option Ty :=
  match mtch a.cl.head g Subst.empty with
  | none => none
  | some σ =>
    let ws := a.cl.body.map (apply σ)
    if ws.all Ty.ground && ws.all (fun w => solve Ps.clauses env fuel [] w == .yes) then
      match a.assoc.lookup n with
      | some v => if (apply σ v).ground then some (apply σ v) else none
      | none => none
    else none

def normalize (Ps : AProg) (env : List Ty) (fuel : Nat) (g : Ty) (n : String) : Option Ty :=
  (Ps.filterMap (normOne Ps env fuel g n)).head?

theorem normOne_sound (Ps : AProg) (env : List Ty) (fuel : Nat) (g : Ty) (n : String) (a : AImpl) (u : Ty)
    (hg : g.ground = true) (ha : a ∈ Ps) (h : normOne Ps env fuel g n a = some u) : NormTo Ps env g n u := by
  simp only [normOne] at h
  split at h
  · cases h
  · next σ hm =>
    split at h
    · next hc =>
      obtain ⟨hgr, hyes⟩ := Bool.and_eq_true_iff.mp hc
      split at h
      · next v hv =>
        split at h
        · next hvg =>
          cases h
          refine ⟨a, ha, σ.fill, ?_, ?_, v, hv, ?_⟩
          · have hs := mtch_sound _ _ _ _ hm
            rw [← apply_eq_inst _ σ σ.fill σ.agrees_fill (by rw [hs]; exact hg), hs]
          · intro w hw
            have hwg : (apply σ w).ground = true := List.all_eq_true.mp hgr _ (List.mem_map_of_mem _ hw)
            have hy := List.all_eq_true.mp hyes _ (List.mem_map_of_mem _ hw)
            rw [← apply_eq_inst w σ σ.fill σ.agrees_fill hwg]
            exact solve_yes_sound _ env fuel [] _ (by simpa using hy)
          · exact (apply_eq_inst v σ σ.fill σ.agrees_fill hvg).symm
        · cases h
      · cases h
    · cases h

theorem normalize_sound (Ps : AProg) (env : List Ty) (fuel : Nat) (g : Ty) (n : String) (u : Ty)
    (hg : g.ground = true) (h : normalize Ps env fuel g n = some u) : NormTo Ps env g n u := by
  simp only [normalize] at h
  have hm : u ∈ Ps.filterMap (normOne Ps env fuel g n) := by
    cases hl : Ps.filterMap (normOne Ps env fuel g n) with
    | nil => rw [hl] at h; cases h
    | cons x xs => rw [hl] at h; cases h; exact List.mem_cons_self _ _
  obtain ⟨a, ha, hau⟩ := List.mem_filterMap.mp hm
  exact normOne_sound Ps env fuel g n a u hg ha hau

/-- two substitutions that give the same instance of `t` agree on its variables -/
theorem inst_agree : ∀ (t : Ty) (σ τ : Nat → Ty), Ty.inst σ t = Ty.inst τ t → ∀ x, t.Occ x → σ x = τ x
  | var n, σ, τ, h, x, hx => by simp only [Ty.Occ] at hx; subst hx; exact h
  | con _, _, _, _, x, hx => by simp [Ty.Occ] at hx
  | app f a, σ, τ, h, x, hx => by
    simp only [Ty.inst, Ty.app.injEq] at h
    rcases hx with hx | hx
    · exact inst_agree f σ τ h.1 x hx
    · exact inst_agree a σ τ h.2 x hx

/-- impls whose heads unify are one impl -/
def AProg.Coherent (Ps : AProg) : Prop := ∀ a ∈ Ps, ∀ b ∈ Ps, mayUnify a.cl.head b.cl.head = true → a = b

/-- an impl's definitions name only parameters its head binds (rustc: E0207) -/
def AProg.WF (Ps : AProg) : Prop := ∀ a ∈ Ps, ∀ n v, a.assoc.lookup n = some v → ∀ x, v.Occ x → a.cl.head.Occ x

theorem normTo_functional (Ps : AProg) (env : List Ty) (hc : Ps.Coherent) (hw : Ps.WF) (g : Ty) (n : String)
    (u u' : Ty) : NormTo Ps env g n u → NormTo Ps env g n u' → u = u' := by
  rintro ⟨a, ha, σ, hσ, _, v, hv, rfl⟩ ⟨b, hb, τ, hτ, _, v', hv', rfl⟩
  -- the two impls' heads have a common instance, so they unify, so they are one impl
  have hu : mayUnify a.cl.head b.cl.head = true := by
    cases hm : mayUnify a.cl.head b.cl.head
    · exact absurd (hσ.trans hτ.symm) (mayUnify_sound _ _ hm σ τ)
    · rfl
  have hab := hc a ha b hb hu
  subst hab
  rw [hv] at hv'
  cases hv'
  -- the two substitutions agree on the head's variables, which are all the definition names
  apply inst_congr
  intro x hx
  exact inst_agree _ σ τ (hσ.trans hτ.symm) x (hw a ha n v hv x hx)

/-- A bound `T: Tr<n = x>` whose `n` normalizes to another type does not hold -/
theorem binding_no_sound (Ps : AProg) (env : List Ty) (hc : Ps.Coherent) (hw : Ps.WF) (fuel : Nat) (g : Ty)
    (n : String) (u x : Ty) (hg : g.ground = true) (h : normalize Ps env fuel g n = some u) (hne : u ≠ x) :
    ¬ NormTo Ps env g n x :=
  fun hx => hne (normTo_functional Ps env hc hw g n u x (normalize_sound Ps env fuel g n u hg h) hx)

/-- traits/associated-types/mismatch-two-relevant-impls.rs: `W<i32>`'s `Assoc` is `u32`, so `Tr<Assoc = ()>`
does not hold -/
def twoImpls : AProg :=
  [⟨⟨app (con "Tr") (app (con "W") (con "i32")), []⟩, [("Assoc", con "u32")]⟩,
   ⟨⟨app (con "Tr") (app (con "W") (con "u32")), []⟩, [("Assoc", con "i32")]⟩]
example : normalize twoImpls [] 4 (app (con "Tr") (app (con "W") (con "i32"))) "Assoc" = some (con "u32") := by decide

end TraitSolver
