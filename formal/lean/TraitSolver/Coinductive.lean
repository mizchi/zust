import TraitSolver.Solver

/-!
# Auto traits: coinductive goals

`Send` / `Sync` hold of a type when they hold of its fields, and a type may contain itself
(`struct List { next: Option<Box<List>> }`): a cycle proves them. Their meaning is the greatest fixpoint: a goal
holds when it is in some *consistent* set of goals, each of which is assumed or is the head of an impl (the
structural rule, as a clause per type) whose where clauses are in the set.

`csolve` is `solve` with one change: a goal that comes back round is `yes` (it is assumed by the set being built).

* `csolve_yes_sound`: `yes` only for goals in a consistent set;
* `csolve_no_sound`: `no` only for goals in no consistent set.
-/

namespace TraitSolver

open Ty

def csolve (P : Prog) (env : List Ty) : Nat → List Ty → Ty → Res
  | 0, _, _ => .overflow
  | fuel + 1, S, g =>
    if g.ground = false then .overflow
    else if g ∈ env then .yes
    else if g ∈ S then .yes
    else disj (P.map (candidate (csolve P env fuel (g :: S)) g))

/-- every goal of `T` is assumed, or the head of an impl whose where clauses are in `T` or among `A` -/
def Closed (P : Prog) (env : List Ty) (A T : List Ty) : Prop :=
  ∀ t ∈ T, t ∈ env ∨ ∃ c ∈ P, ∃ τ : Nat → Ty, Ty.inst τ c.head = t ∧ ∀ w ∈ c.body, Ty.inst τ w ∈ T ∨ Ty.inst τ w ∈ A

/-- The specification: `g` is in a consistent set of goals -/
def CoHolds (P : Prog) (env : List Ty) (g : Ty) : Prop := ∃ T, Closed P env [] T ∧ g ∈ T

theorem Closed.mono_A {P env A A' T} (h : Closed P env A T) (hA : ∀ x ∈ A, x ∈ A') : Closed P env A' T := by
  intro t ht
  rcases h t ht with he | ⟨c, hc, τ, hh, hb⟩
  · exact Or.inl he
  · exact Or.inr ⟨c, hc, τ, hh, fun w hw => (hb w hw).imp id (hA _)⟩

/-- the union of sets closed above `g :: A`, with `g`, is closed above `A`, when `g` is the head of an impl whose
where clauses are in them -/
theorem closed_cons {P env A} (g : Ty) (Ts : List (List Ty)) (hT : ∀ T ∈ Ts, Closed P env (g :: A) T)
    (hg : g ∈ env ∨ ∃ c ∈ P, ∃ τ : Nat → Ty, Ty.inst τ c.head = g ∧
      ∀ w ∈ c.body, (∃ T ∈ Ts, Ty.inst τ w ∈ T) ∨ Ty.inst τ w ∈ g :: A) :
    Closed P env A (g :: Ts.flatten) := by
  intro t ht
  -- membership in the union, and the step from `g :: A` to `A`: `g` is in the set itself
  have up : ∀ x, x ∈ g :: A → x ∈ g :: Ts.flatten ∨ x ∈ A := by
    intro x hx
    rcases List.mem_cons.mp hx with he | he
    · exact Or.inl (he ▸ List.mem_cons_self _ _)
    · exact Or.inr he
  have inU : ∀ x T, T ∈ Ts → x ∈ T → x ∈ g :: Ts.flatten :=
    fun x T hT hx => List.mem_cons_of_mem _ (List.mem_flatten.mpr ⟨T, hT, hx⟩)
  rcases List.mem_cons.mp ht with he | he
  · subst he
    rcases hg with he | ⟨c, hc, τ, hh, hb⟩
    · exact Or.inl he
    · refine Or.inr ⟨c, hc, τ, hh, fun w hw => ?_⟩
      rcases hb w hw with ⟨T, hTm, hx⟩ | hx
      · exact Or.inl (inU _ T hTm hx)
      · exact up _ hx
  · obtain ⟨T, hTm, htT⟩ := List.mem_flatten.mp he
    rcases hT T hTm t htT with he | ⟨c, hc, τ, hh, hb⟩
    · exact Or.inl he
    · refine Or.inr ⟨c, hc, τ, hh, fun w hw => ?_⟩
      rcases hb w hw with hx | hx
      · exact Or.inl (inU _ T hTm hx)
      · exact up _ hx

theorem csolve_yes_closed (P : Prog) (env : List Ty) :
    ∀ fuel A g, csolve P env fuel A g = .yes → ∃ T, Closed P env A T ∧ (g ∈ T ∨ g ∈ A)
  | 0, _, _, h => by cases h
  | fuel + 1, A, g, h => by
    simp only [csolve] at h
    split at h
    · cases h
    · split at h
      · next hg => exact ⟨[g], fun t ht => by simp at ht; subst ht; exact Or.inl hg, Or.inl (List.mem_cons_self _ _)⟩
      · split at h
        · next hA => exact ⟨[], ⟨fun _ h => by simp at h, Or.inr hA⟩⟩
        · obtain ⟨r, hr, hry⟩ := disj_yes h
          obtain ⟨c, hc, rfl⟩ := List.mem_map.mp hr
          simp only [candidate] at hry
          split at hry
          · cases hry
          · next σ hm =>
            split at hry
            · next hgr =>
              have hall := conj_yes hry
              have hgnd : g.ground = true := by
                cases hgg : g.ground
                · simp_all
                · rfl
              -- the sets the where clauses' answers give
              have ih : ∀ w ∈ c.body, ∃ T, Closed P env (g :: A) T ∧ (apply σ w ∈ T ∨ apply σ w ∈ g :: A) :=
                fun w hw => csolve_yes_closed P env fuel (g :: A) (apply σ w)
                  (hall _ (List.mem_map_of_mem _ (List.mem_map_of_mem _ hw)))
              have pick : ∀ l : List Ty, (∀ w ∈ l, w ∈ c.body) → ∃ Ts : List (List Ty),
                  (∀ T ∈ Ts, Closed P env (g :: A) T) ∧
                  ∀ w ∈ l, (∃ T ∈ Ts, apply σ w ∈ T) ∨ apply σ w ∈ g :: A := by
                intro l
                induction l with
                | nil => intro _; exact ⟨[], ⟨fun _ h => by simp at h, fun _ h => by simp at h⟩⟩
                | cons x xs ihl =>
                  intro hl
                  obtain ⟨Ts, hTs, hw⟩ := ihl (fun w hw => hl w (List.mem_cons_of_mem _ hw))
                  obtain ⟨T, hT, hx⟩ := ih x (hl x (List.mem_cons_self _ _))
                  refine ⟨T :: Ts, fun T' hT' => ?_, fun w hw' => ?_⟩
                  · rcases List.mem_cons.mp hT' with he | he
                    · exact he ▸ hT
                    · exact hTs T' he
                  · rcases List.mem_cons.mp hw' with he | he
                    · subst he
                      rcases hx with hx | hx
                      · exact Or.inl ⟨T, List.mem_cons_self _ _, hx⟩
                      · exact Or.inr hx
                    · rcases hw w he with ⟨T', hT', hx'⟩ | hx'
                      · exact Or.inl ⟨T', List.mem_cons_of_mem _ hT', hx'⟩
                      · exact Or.inr hx'
              obtain ⟨Ts, hTs, hw⟩ := pick c.body (fun w hw => hw)
              refine ⟨g :: Ts.flatten, closed_cons g Ts hTs (Or.inr ⟨c, hc, σ.fill, ?_, fun w hw' => ?_⟩),
                Or.inl (List.mem_cons_self _ _)⟩
              · have hs := mtch_sound _ _ _ _ hm
                rw [← apply_eq_inst c.head σ σ.fill σ.agrees_fill (by rw [hs]; exact hgnd), hs]
              · have hwg : (apply σ w).ground = true :=
                  List.all_eq_true.mp hgr _ (List.mem_map_of_mem _ hw')
                rw [← apply_eq_inst w σ σ.fill σ.agrees_fill hwg]
                exact hw w hw'
            · cases hry

/-- `yes` is sound for the coinductive meaning -/
theorem csolve_yes_sound (P : Prog) (env : List Ty) (fuel : Nat) (g : Ty) :
    csolve P env fuel [] g = .yes → CoHolds P env g := by
  intro h
  obtain ⟨T, hT, hg⟩ := csolve_yes_closed P env fuel [] g h
  rcases hg with hg | hg
  · exact ⟨T, hT, hg⟩
  · cases hg

/-- `no` is sound too: a `no` never comes from the stack, so it holds of every consistent set -/
theorem csolve_no_sound (P : Prog) (env : List Ty) :
    ∀ fuel A g, csolve P env fuel A g = .no → ∀ T, Closed P env [] T → g ∉ T
  | 0, _, _, h => by cases h
  | fuel + 1, A, g, h => by
    intro T hT hgT
    simp only [csolve] at h
    split at h
    · cases h
    · next hgnd =>
      split at h
      · cases h
      · next hne =>
        split at h
        · cases h
        · rcases hT g hgT with he | ⟨c, hc, τ, hh, hb⟩
          · exact hne he
          · have hcn := disj_no h _ (List.mem_map_of_mem _ hc)
            obtain ⟨σ, hm, hag⟩ := mtch_complete c.head τ Subst.empty (fun _ _ h => by cases h)
            rw [hh] at hm
            simp only [candidate, hm] at hcn
            split at hcn
            · next hgr =>
              obtain ⟨r, hr, hrn⟩ := conj_no hcn
              obtain ⟨w', hw', rfl⟩ := List.mem_map.mp hr
              obtain ⟨w, hw, rfl⟩ := List.mem_map.mp hw'
              have hwg : (apply σ w).ground = true := List.all_eq_true.mp hgr _ hw'
              have hin : Ty.inst τ w ∈ T := by
                rcases hb w hw with h' | h'
                · exact h'
                · cases h'
              rw [← apply_eq_inst w σ τ hag hwg] at hin
              exact csolve_no_sound P env fuel (g :: A) (apply σ w) hrn T hT hin
            · cases hcn

/-- `struct List { next: Option<Box<List>> }`: `List: Send` needs `Option<Box<List>>: Send`, which needs
`Box<List>: Send`, which needs `List: Send` again. Coinductively it holds; inductively (an ordinary trait) not -/
def sendList : Prog :=
  [⟨app (con "Send") (con "List"), [app (con "Send") (app (con "Option") (app (con "Box") (con "List")))]⟩,
   ⟨app (con "Send") (app (con "Option") (var 0)), [app (con "Send") (var 0)]⟩,
   ⟨app (con "Send") (app (con "Box") (var 0)), [app (con "Send") (var 0)]⟩]
example : csolve sendList [] 8 [] (app (con "Send") (con "List")) = .yes := by decide
example : solve sendList [] 8 [] (app (con "Send") (con "List")) = .no := by decide

end TraitSolver
