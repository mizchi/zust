import TraitSolver.Solver

/-!
# Overlap of impls (coherence, E0119)

Two impls overlap when some goal is an instance of both heads. zustc decides it without unification by
`mayUnify`, which walks the two heads side by side and fails only at two different constructors. A variable of
either head stands for anything, so a head whose parameters occur once each (linear: `impl<T, U> Tr for (T, U)`, not
`impl<T> Tr for (T, T)`) needs no occurs check or substitution to be decided exactly:

* `mayUnify_sound`: `mayUnify = false` → no goal is an instance of both (the impls never compete);
* `mayUnify_complete`: for linear heads, `mayUnify = true` → some goal is (a real overlap: report E0119).

For heads that are not linear `mayUnify = true` may be no overlap (`(T, T)` against `(u8, u16)`), so zustc
does not report E0119 on it alone.

`select_unique`: with heads that pairwise do not unify, at most one impl applies to a goal.
-/

namespace TraitSolver

open Ty

def mayUnify : Ty → Ty → Bool
  | var _, _ => true
  | _, var _ => true
  | con c, con d => c = d
  | app f a, app g b => mayUnify f g && mayUnify a b
  | _, _ => false

def Linear : Ty → Prop
  | var _ => True
  | con _ => True
  | app f a => Linear f ∧ Linear a ∧ ∀ x, f.Occ x → ¬ a.Occ x

theorem mayUnify_sound : ∀ (p q : Ty), mayUnify p q = false →
    ∀ σ τ : Nat → Ty, Ty.inst σ p ≠ Ty.inst τ q
  | var _, _, h, _, _ => by simp [mayUnify] at h
  | con _, var _, h, _, _ => by simp [mayUnify] at h
  | app _ _, var _, h, _, _ => by simp [mayUnify] at h
  | con c, con d, h, σ, τ => by
    simp [mayUnify] at h; simp [Ty.inst, h]
  | con _, app _ _, _, _, _ => by simp [Ty.inst]
  | app _ _, con _, _, _, _ => by simp [Ty.inst]
  | app f a, app g b, h, σ, τ => by
    simp only [mayUnify, Bool.and_eq_false_iff] at h
    simp only [Ty.inst, ne_eq, Ty.app.injEq, not_and]
    rcases h with h | h
    · intro he; exact absurd he (mayUnify_sound f g h σ τ)
    · intro _ he; exact absurd he (mayUnify_sound a b h σ τ)

/-- substitutions that agree on the variables of `t` give the same instance -/
theorem inst_congr : ∀ (t : Ty) (σ σ' : Nat → Ty), (∀ x, t.Occ x → σ x = σ' x) → Ty.inst σ t = Ty.inst σ' t
  | var n, _, _, h => h n rfl
  | con _, _, _, _ => rfl
  | app f a, σ, σ', h => by
    simp only [Ty.inst]
    rw [inst_congr f σ σ' (fun x hx => h x (Or.inl hx)), inst_congr a σ σ' (fun x hx => h x (Or.inr hx))]

/-- an instance of a linear pattern can be put together from instances of its two sides -/
theorem mayUnify_complete : ∀ (p q : Ty), Linear p → Linear q → mayUnify p q = true →
    ∃ σ τ : Nat → Ty, Ty.inst σ p = Ty.inst τ q
  | var x, q, _, _, _ =>
    ⟨fun _ => Ty.inst (fun _ => con "_") q, fun _ => con "_", by simp [Ty.inst]⟩
  | con c, var y, _, _, _ => ⟨fun _ => con "_", fun _ => con c, by simp [Ty.inst]⟩
  | app f a, var y, _, _, _ =>
    ⟨fun _ => con "_", fun _ => Ty.inst (fun _ => con "_") (app f a), by simp [Ty.inst]⟩
  | con c, con d, _, _, h => by
    simp [mayUnify] at h; subst h; exact ⟨fun _ => con "_", fun _ => con "_", rfl⟩
  | con _, app _ _, _, _, h => by simp [mayUnify] at h
  | app _ _, con _, _, _, h => by simp [mayUnify] at h
  | app f a, app g b, ⟨lf, la, df⟩, ⟨lg, lb, dg⟩, h => by
    simp only [mayUnify, Bool.and_eq_true] at h
    obtain ⟨σ₁, τ₁, h₁⟩ := mayUnify_complete f g lf lg h.1
    obtain ⟨σ₂, τ₂, h₂⟩ := mayUnify_complete a b la lb h.2
    -- each side takes the variables of its own half (the halves share none)
    have : ∀ x, Decidable (f.Occ x) := fun _ => Classical.propDecidable _
    have : ∀ x, Decidable (g.Occ x) := fun _ => Classical.propDecidable _
    let σ : Nat → Ty := fun x => if f.Occ x then σ₁ x else σ₂ x
    let τ : Nat → Ty := fun x => if g.Occ x then τ₁ x else τ₂ x
    have hσf : Ty.inst σ f = Ty.inst σ₁ f := inst_congr f σ σ₁ (fun x hx => if_pos hx)
    have hσa : Ty.inst σ a = Ty.inst σ₂ a := inst_congr a σ σ₂ (fun x hx => if_neg (fun h => df x h hx))
    have hτg : Ty.inst τ g = Ty.inst τ₁ g := inst_congr g τ τ₁ (fun x hx => if_pos hx)
    have hτb : Ty.inst τ b = Ty.inst τ₂ b := inst_congr b τ τ₂ (fun x hx => if_neg (fun h => dg x h hx))
    exact ⟨σ, τ, by simp only [Ty.inst]; rw [hσf, hσa, hτg, hτb, h₁, h₂]⟩

/-- impls whose heads pairwise do not unify never both apply: selection is unique -/
theorem select_unique (P : Prog) (hdis : ∀ c ∈ P, ∀ d ∈ P, c ≠ d → mayUnify c.head d.head = false)
    (g : Ty) (c d : Clause) (hc : c ∈ P) (hd : d ∈ P) (σ τ : Nat → Ty)
    (hcg : Ty.inst σ c.head = g) (hdg : Ty.inst τ d.head = g) : c = d := by
  apply Classical.byContradiction
  intro hne
  exact mayUnify_sound _ _ (hdis c hc d hd hne) σ τ (hcg.trans hdg.symm)

end TraitSolver
