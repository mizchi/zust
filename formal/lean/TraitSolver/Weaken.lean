import TraitSolver.Solver

/-!
# Weakening: what zustc does with what it cannot read

zustc's clauses are not exactly the program's impls: a where clause it cannot parse is left out of the body, a
self type or trait argument it cannot resolve becomes a parameter (matches anything), and a goal's `i32` / `f64`
(possibly a defaulted literal) matches any integer / float. Each makes the clause *cover* the real one: every
instance of the real impl is an instance of zustc's, with fewer (or the same) where clauses.

`Holds.weaken`: a program whose clauses each are covered by one of `P'`'s proves no more than `P'`. So
`solve_no_sound_weaken`: a `no` of zustc's clauses `P'` is a `no` of the real program `P`, which is the only
answer zustc reports an error on.
-/

namespace TraitSolver

open Ty

/-- `c'` covers `c`: every instance of `c` is one of `c'` whose where clauses are among `c`'s -/
def Clause.covers (c' c : Clause) : Prop :=
  ∀ τ : Nat → Ty, ∃ τ' : Nat → Ty, Ty.inst τ' c'.head = Ty.inst τ c.head ∧
    ∀ w' ∈ c'.body, ∃ w ∈ c.body, Ty.inst τ' w' = Ty.inst τ w

def Prog.weakens (P' P : Prog) : Prop := ∀ c ∈ P, ∃ c' ∈ P', c'.covers c

theorem Holds.weaken {P P' : Prog} {env env' : List Ty} (hw : P'.weakens P) (he : ∀ g ∈ env, g ∈ env') :
    ∀ {g}, Holds P env g → Holds P' env' g := by
  intro g h
  induction h with
  | assume hg => exact Holds.assume (he _ hg)
  | @impl g c τ hc hh _ ih =>
    obtain ⟨c', hc', hcov⟩ := hw c hc
    obtain ⟨τ', hhead, hbody⟩ := hcov τ
    refine Holds.impl c' τ' hc' (hhead.trans hh) ?_
    intro w' hw'
    obtain ⟨w, hw, heq⟩ := hbody w' hw'
    rw [heq]; exact ih w hw

theorem solve_no_sound_weaken {P P' : Prog} {env env' : List Ty} (hw : P'.weakens P) (he : ∀ g ∈ env, g ∈ env')
    (fuel : Nat) (g : Ty) : solve P' env' fuel [] g = .no → ¬ Holds P env g :=
  fun h hh => solve_no_sound P' env' fuel g h (hh.weaken hw he)

/-- Leaving where clauses out covers: the same head, a sublist of the body -/
theorem Clause.covers_drop (c : Clause) (body' : List Ty) (hs : ∀ w ∈ body', w ∈ c.body) :
    Clause.covers ⟨c.head, body'⟩ c :=
  fun τ => ⟨τ, rfl, fun w hw => ⟨w, hs w hw, rfl⟩⟩

/-- Replacing a part of the head by a fresh parameter covers: `x` not in `c` (so it names nothing else), the head
`h` with `var x` where `c.head` had `t` -/
theorem Clause.covers_generalize (c : Clause) (h : Ty) (x : Nat) (t : Ty)
    (hfresh : ∀ w ∈ c.body, ¬ w.Occ x)
    (hgen : ∀ τ : Nat → Ty, Ty.inst (fun n => if n = x then Ty.inst τ t else τ n) h = Ty.inst τ c.head) :
    Clause.covers ⟨h, c.body⟩ c := by
  intro τ
  refine ⟨fun n => if n = x then Ty.inst τ t else τ n, hgen τ, ?_⟩
  intro w hw
  refine ⟨w, hw, ?_⟩
  apply Classical.byContradiction
  intro hne
  apply hne
  -- the body does not name x: the two substitutions agree on it
  have : ∀ u : Ty, ¬ u.Occ x → Ty.inst (fun n => if n = x then Ty.inst τ t else τ n) u = Ty.inst τ u := by
    intro u
    induction u with
    | var n =>
      intro hn
      have : n ≠ x := fun e => hn (by simp [Ty.Occ, e])
      simp [Ty.inst, this]
    | con _ => intro _; rfl
    | app f a ihf iha =>
      intro hn
      simp only [Ty.Occ, not_or] at hn
      simp only [Ty.inst]; rw [ihf hn.1, iha hn.2]
  exact this w (hfresh w hw)

end TraitSolver
