/-!
# Types, substitutions and one-way matching

A type is a tree of constructors applied to arguments, curried: `Vec<u8>` is `app (con "Vec") (con "u8")`, and a
trait goal `T: Tr<A>` is the type `app (app (con "Tr") T) A`. So the head of an impl and the goals it asks for
(its where clauses) are types too, and the impls of a program are Horn clauses over these trees.

`var n` is the impl's n-th generic parameter. A type parameter of the function being checked (a placeholder, rigid)
is a constructor of its own, not a `var`: nothing is ever substituted for it.
-/

namespace TraitSolver

inductive Ty where
  | var : Nat → Ty
  | con : String → Ty
  | app : Ty → Ty → Ty
  deriving DecidableEq, Repr

namespace Ty

/-- `x` occurs in `t` -/
def Occ (x : Nat) : Ty → Prop
  | var y => x = y
  | con _ => False
  | app f a => Occ x f ∨ Occ x a

/-- no variable occurs in `t` -/
def ground : Ty → Bool
  | var _ => false
  | con _ => true
  | app f a => ground f && ground a

/-- a total substitution: the meaning of an impl's parameters in the specification -/
def inst (τ : Nat → Ty) : Ty → Ty
  | var n => τ n
  | con c => con c
  | app f a => app (inst τ f) (inst τ a)

end Ty

open Ty

/-- a partial substitution: what matching has found so far -/
abbrev Subst := Nat → Option Ty

def Subst.empty : Subst := fun _ => none

def Subst.set (σ : Subst) (n : Nat) (t : Ty) : Subst := fun m => if m = n then some t else σ m

/-- `σ` is contained in `σ'` -/
def Subst.le (σ σ' : Subst) : Prop := ∀ n u, σ n = some u → σ' n = some u

/-- `σ` agrees with the total `τ` where it is defined -/
def Subst.agrees (σ : Subst) (τ : Nat → Ty) : Prop := ∀ n u, σ n = some u → τ n = u

def apply (σ : Subst) : Ty → Ty
  | var n => (σ n).getD (var n)
  | con c => con c
  | app f a => app (apply σ f) (apply σ a)

/-- One-way matching of a pattern (an impl's head, with `var`s) against a goal. A variable seen twice must match
equal types (`impl<T> Tr for (T, T)`). -/
def mtch : Ty → Ty → Subst → Option Subst
  | var n, t, σ => match σ n with
    | none => some (σ.set n t)
    | some u => if u = t then some σ else none
  | con c, con d, σ => if c = d then some σ else none
  | app f a, app g b, σ => match mtch f g σ with
    | none => none
    | some σ₁ => mtch a b σ₁
  | _, _, _ => none

theorem Subst.le_refl (σ : Subst) : σ.le σ := fun _ _ h => h

theorem Subst.le_trans {a b c : Subst} (h₁ : a.le b) (h₂ : b.le c) : a.le c :=
  fun n u h => h₂ n u (h₁ n u h)

theorem Subst.agrees_of_le {σ σ' : Subst} {τ} (h : σ.le σ') (h' : σ'.agrees τ) : σ.agrees τ :=
  fun n u hn => h' n u (h n u hn)

/-- matching only adds to the substitution -/
theorem mtch_mono : ∀ (p t : Ty) (σ σ' : Subst), mtch p t σ = some σ' → σ.le σ'
  | var n, t, σ, σ', h => by
    unfold mtch at h
    split at h
    · next hn =>
      cases h
      intro m u hm
      simp only [Subst.set]
      split
      · next he => subst he; rw [hn] at hm; cases hm
      · exact hm
    · split at h
      · cases h; exact Subst.le_refl _
      · cases h
  | con c, con d, σ, σ', h => by
    unfold mtch at h; split at h
    · cases h; exact Subst.le_refl _
    · cases h
  | con _, var _, _, _, h => by simp [mtch] at h
  | con _, app _ _, _, _, h => by simp [mtch] at h
  | app f a, app g b, σ, σ', h => by
    unfold mtch at h
    split at h
    · cases h
    · next σ₁ h₁ => exact Subst.le_trans (mtch_mono f g σ σ₁ h₁) (mtch_mono a b σ₁ σ' h)
  | app _ _, var _, _, _, h => by simp [mtch] at h
  | app _ _, con _, _, _, h => by simp [mtch] at h

/-- after matching, every variable of the pattern is bound -/
theorem mtch_binds : ∀ (p t : Ty) (σ σ' : Subst), mtch p t σ = some σ' → ∀ x, p.Occ x → (σ' x).isSome
  | var n, t, σ, σ', h, x, hx => by
    simp only [Ty.Occ] at hx; subst hx
    unfold mtch at h
    split at h
    · cases h; simp [Subst.set]
    · next u hu =>
      split at h
      · cases h; simp [hu]
      · cases h
  | con c, con d, σ, σ', _, x, hx => by simp [Ty.Occ] at hx
  | con _, var _, _, _, h, _, _ => by simp [mtch] at h
  | con _, app _ _, _, _, h, _, _ => by simp [mtch] at h
  | app f a, app g b, σ, σ', h, x, hx => by
    unfold mtch at h
    split at h
    · cases h
    · next σ₁ h₁ =>
      rcases hx with hx | hx
      · have := mtch_binds f g σ σ₁ h₁ x hx
        have hle := mtch_mono a b σ₁ σ' h
        cases hs : σ₁ x with
        | none => rw [hs] at this; cases this
        | some u => rw [hle x u hs]; rfl
      · exact mtch_binds a b σ₁ σ' h x hx
  | app _ _, var _, _, _, h, _, _ => by simp [mtch] at h
  | app _ _, con _, _, _, h, _, _ => by simp [mtch] at h

/-- applying a larger substitution changes nothing once every variable of `p` is bound -/
theorem apply_ext : ∀ (p : Ty) (σ σ' : Subst), σ.le σ' → (∀ x, p.Occ x → (σ x).isSome) → apply σ p = apply σ' p
  | var n, σ, σ', hle, hb => by
    have := hb n rfl
    cases hs : σ n with
    | none => rw [hs] at this; cases this
    | some u => simp [apply, hs, hle n u hs]
  | con _, _, _, _, _ => rfl
  | app f a, σ, σ', hle, hb => by
    simp only [apply]
    rw [apply_ext f σ σ' hle (fun x hx => hb x (Or.inl hx)), apply_ext a σ σ' hle (fun x hx => hb x (Or.inr hx))]

/-- matching is sound: the pattern under the substitution found is the goal -/
theorem mtch_sound : ∀ (p t : Ty) (σ σ' : Subst), mtch p t σ = some σ' → apply σ' p = t
  | var n, t, σ, σ', h => by
    unfold mtch at h
    split at h
    · cases h; simp [apply, Subst.set]
    · next u hu =>
      split at h
      · next he => cases h; subst he; simp [apply, hu]
      · cases h
  | con c, con d, σ, σ', h => by
    unfold mtch at h; split at h
    · next he => subst he; rfl
    · cases h
  | con _, var _, _, _, h => by simp [mtch] at h
  | con _, app _ _, _, _, h => by simp [mtch] at h
  | app f a, app g b, σ, σ', h => by
    unfold mtch at h
    split at h
    · cases h
    · next σ₁ h₁ =>
      simp only [apply]
      have hf := mtch_sound f g σ σ₁ h₁
      have ha := mtch_sound a b σ₁ σ' h
      rw [← apply_ext f σ₁ σ' (mtch_mono a b σ₁ σ' h) (mtch_binds f g σ σ₁ h₁), hf, ha]
  | app _ _, var _, _, _, h => by simp [mtch] at h
  | app _ _, con _, _, _, h => by simp [mtch] at h

/-- matching is complete: if some instance of the pattern is the goal, matching finds a substitution that agrees
with it -/
theorem mtch_complete : ∀ (p : Ty) (τ : Nat → Ty) (σ : Subst), σ.agrees τ →
    ∃ σ', mtch p (Ty.inst τ p) σ = some σ' ∧ σ'.agrees τ
  | var n, τ, σ, hag => by
    cases hs : σ n with
    | none =>
      refine ⟨σ.set n (τ n), by simp [mtch, Ty.inst, hs], ?_⟩
      intro m u hm
      simp only [Subst.set] at hm
      split at hm
      · next he => subst he; cases hm; rfl
      · exact hag m u hm
    | some u =>
      have hu := hag n u hs
      refine ⟨σ, ?_, hag⟩
      simp [mtch, Ty.inst, hs, hu]
  | con c, τ, σ, hag => ⟨σ, by simp [mtch, Ty.inst], hag⟩
  | app f a, τ, σ, hag => by
    obtain ⟨σ₁, h₁, hag₁⟩ := mtch_complete f τ σ hag
    obtain ⟨σ₂, h₂, hag₂⟩ := mtch_complete a τ σ₁ hag₁
    exact ⟨σ₂, by simp [mtch, Ty.inst, h₁, h₂], hag₂⟩

/-- a ground result of a partial substitution is the instance of any total one that agrees with it -/
theorem apply_eq_inst : ∀ (w : Ty) (σ : Subst) (τ : Nat → Ty), σ.agrees τ → (apply σ w).ground = true →
    apply σ w = Ty.inst τ w
  | var n, σ, τ, hag, hg => by
    cases hs : σ n with
    | none => simp [apply, hs, Ty.ground] at hg
    | some u => simp [apply, hs, Ty.inst, hag n u hs]
  | con _, _, _, _, _ => rfl
  | app f a, σ, τ, hag, hg => by
    simp only [apply, Ty.ground, Bool.and_eq_true] at hg ⊢
    simp only [Ty.inst]
    rw [apply_eq_inst f σ τ hag hg.1, apply_eq_inst a σ τ hag hg.2]

/-- the total substitution that a partial one stands for (unbound variables go to an arbitrary type) -/
def Subst.fill (σ : Subst) : Nat → Ty := fun n => (σ n).getD (con "_")

theorem Subst.agrees_fill (σ : Subst) : σ.agrees σ.fill := by
  intro n u h; simp [Subst.fill, h]

end TraitSolver
