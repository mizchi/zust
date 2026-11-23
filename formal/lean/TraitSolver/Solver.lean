import TraitSolver.Ty

/-!
# Trait goals, the solver, and what its answers mean

An impl is a Horn clause: `impl<T: Clone> Tr for Vec<T> where Vec<T>: Debug` is the head `Vec<T>: Tr` and the body
`[T: Clone, Vec<T>: Debug]`. The environment is what the item being checked assumes (its own where clauses, on its
placeholders).

`Holds` is the specification: the least set of goals closed under the impls (inductive: a proof is a finite tree, so
`impl<T: B> A for T` with `impl<T: A> B for T` proves neither). This is what rustc means by a trait goal holding,
for the traits that are not auto traits (those are coinductive: `Coinductive.lean`).

`solve` is the algorithm zustc runs (`crates/sema/src/traits.rs`): depth first over the impls, with fuel (rustc's
recursion limit), and with the goals being proved on a stack, so a goal that comes back round fails that candidate.
It answers `yes`, `no` or `overflow` (out of fuel, or a where clause that names a parameter the head does not bind:
zustc cannot tell). The theorems:

* `solve_yes_sound`: `yes` only when the goal holds;
* `solve_no_sound`: `no` only when it does not. So an error reported on `no` is never a false one, and `overflow`
  must be treated as "maybe" (zustc accepts);
* `solve_complete`: with enough fuel, a goal that holds gets `yes` (`overflow` is not an answer for a goal that holds and has a
  proof shallower than the fuel).
-/

namespace TraitSolver

open Ty

structure Clause where
  head : Ty
  body : List Ty

abbrev Prog := List Clause

/-- The specification. -/
inductive Holds (P : Prog) (env : List Ty) : Ty → Prop where
  | assume {g} : g ∈ env → Holds P env g
  | impl {g} (c : Clause) (τ : Nat → Ty) : c ∈ P → Ty.inst τ c.head = g →
      (∀ w ∈ c.body, Holds P env (Ty.inst τ w)) → Holds P env g

/-- Proofs of height at most `n` none of whose impl steps proves a goal in `S` (the goals being proved above). -/
inductive HA (P : Prog) (env : List Ty) (S : List Ty) : Nat → Ty → Prop where
  | assume {n g} : g ∈ env → HA P env S n g
  | impl {n g} (c : Clause) (τ : Nat → Ty) : g ∉ S → c ∈ P → Ty.inst τ c.head = g →
      (∀ w ∈ c.body, HA P env S n (Ty.inst τ w)) → HA P env S (n + 1) g

inductive Res where
  | yes | no | overflow
  deriving DecidableEq, Repr

/-- both: `no` if either is `no` (that candidate surely fails), `yes` if both are -/
def Res.and : Res → Res → Res
  | .yes, .yes => .yes
  | .yes, .no => .no
  | .yes, .overflow => .overflow
  | .no, _ => .no
  | .overflow, .yes => .overflow
  | .overflow, .no => .no
  | .overflow, .overflow => .overflow

/-- either: `yes` if either is `yes`, `no` if both are -/
def Res.or : Res → Res → Res
  | .no, .no => .no
  | .no, .yes => .yes
  | .no, .overflow => .overflow
  | .yes, _ => .yes
  | .overflow, .no => .overflow
  | .overflow, .yes => .yes
  | .overflow, .overflow => .overflow

/-- all of the goals -/
def conj : List Res → Res
  | [] => .yes
  | r :: rs => r.and (conj rs)

/-- one of the candidates -/
def disj : List Res → Res
  | [] => .no
  | r :: rs => r.or (disj rs)

/-- one impl as a candidate for the goal `g` -/
def candidate (solveSub : Ty → Res) (g : Ty) (c : Clause) : Res :=
  match mtch c.head g Subst.empty with
  | none => .no
  | some σ =>
    let ws := c.body.map (apply σ)
    if ws.all Ty.ground then conj (ws.map solveSub) else .overflow

def solve (P : Prog) (env : List Ty) : Nat → List Ty → Ty → Res
  | 0, _, _ => .overflow
  | fuel + 1, S, g =>
    if g.ground = false then .overflow
    else if g ∈ env then .yes
    else if g ∈ S then .no
    else disj (P.map (candidate (solve P env fuel (g :: S)) g))

/-! ## conj / disj -/

theorem conj_yes : ∀ {l : List Res}, conj l = .yes → ∀ r ∈ l, r = .yes
  | [], _, _, h => by cases h
  | r :: rs, h, x, hx => by
    simp only [conj] at h
    have hr : r = .yes := by
      cases r <;> cases hc : conj rs <;> simp_all [Res.and]
    have hrs : conj rs = .yes := by
      cases r <;> cases hc : conj rs <;> simp_all [Res.and]
    rcases List.mem_cons.mp hx with hx | hx
    · rw [hx, hr]
    · exact conj_yes hrs x hx

theorem conj_no : ∀ {l : List Res}, conj l = .no → ∃ r ∈ l, r = .no
  | [], h => by cases h
  | r :: rs, h => by
    simp only [conj] at h
    by_cases hr : r = .no
    · exact ⟨r, List.mem_cons_self _ _, hr⟩
    · have hrs : conj rs = .no := by
        cases r <;> cases hc : conj rs <;> simp_all [Res.and]
      obtain ⟨x, hx, hxn⟩ := conj_no hrs
      exact ⟨x, List.mem_cons_of_mem _ hx, hxn⟩

theorem conj_all_yes : ∀ {l : List Res}, (∀ r ∈ l, r = .yes) → conj l = .yes
  | [], _ => rfl
  | r :: rs, h => by
    have hr := h r (List.mem_cons_self _ _)
    have hrs := conj_all_yes (fun x hx => h x (List.mem_cons_of_mem _ hx))
    simp [conj, hr, hrs, Res.and]

theorem disj_yes : ∀ {l : List Res}, disj l = .yes → ∃ r ∈ l, r = .yes
  | [], h => by cases h
  | r :: rs, h => by
    simp only [disj] at h
    by_cases hr : r = .yes
    · exact ⟨r, List.mem_cons_self _ _, hr⟩
    · have hrs : disj rs = .yes := by
        cases r <;> cases hc : disj rs <;> simp_all [Res.or]
      obtain ⟨x, hx, hxy⟩ := disj_yes hrs
      exact ⟨x, List.mem_cons_of_mem _ hx, hxy⟩

theorem disj_no : ∀ {l : List Res}, disj l = .no → ∀ r ∈ l, r = .no
  | [], _, _, h => by cases h
  | r :: rs, h, x, hx => by
    simp only [disj] at h
    have hr : r = .no := by
      cases r <;> cases hc : disj rs <;> simp_all [Res.or]
    have hrs : disj rs = .no := by
      cases r <;> cases hc : disj rs <;> simp_all [Res.or]
    rcases List.mem_cons.mp hx with hx | hx
    · rw [hx, hr]
    · exact disj_no hrs x hx

theorem disj_of_yes : ∀ {l : List Res} {r}, r ∈ l → r = .yes → disj l = .yes
  | [], _, h, _ => by cases h
  | x :: xs, r, hm, hr => by
    rcases List.mem_cons.mp hm with he | he
    · subst he; subst hr; simp [disj, Res.or]
    · have := disj_of_yes he hr
      cases x <;> simp [disj, this, Res.or]

/-! ## `yes` is sound -/

theorem solve_yes_sound (P : Prog) (env : List Ty) :
    ∀ fuel S g, solve P env fuel S g = .yes → Holds P env g
  | 0, _, _, h => by cases h
  | fuel + 1, S, g, h => by
    simp only [solve] at h
    split at h
    · cases h
    · split at h
      · next hg => exact Holds.assume hg
      · split at h
        · cases h
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
              refine Holds.impl c σ.fill hc ?_ ?_
              · have hs := mtch_sound _ _ _ _ hm
                rw [← apply_eq_inst c.head σ σ.fill σ.agrees_fill (by rw [hs]; exact hgnd), hs]
              · intro w hw
                have hwg : (apply σ w).ground = true :=
                  List.all_eq_true.mp hgr _ (List.mem_map_of_mem _ hw)
                rw [← apply_eq_inst w σ σ.fill σ.agrees_fill hwg]
                exact solve_yes_sound P env fuel (g :: S) (apply σ w)
                  (hall _ (List.mem_map_of_mem _ (List.mem_map_of_mem _ hw)))
            · cases hry

/-! ## `no` is sound

A proof can always be cut so that no goal is proved twice on one path (`HA_cut`), so the candidates the stack
rules out lose nothing. -/

theorem HA.mono {P env S} : ∀ {n m g}, n ≤ m → HA P env S n g → HA P env S m g := by
  intro n m g hle h
  induction h generalizing m with
  | assume hg => exact HA.assume hg
  | @impl n g c τ hS hc hh _ ih =>
    cases m with
    | zero => omega
    | succ m => exact HA.impl c τ hS hc hh (fun w hw => ih w hw (by omega))

/-- a proof avoiding `S` either avoids `g` too, or contains a proof of `g` (avoiding `S`) no higher than it -/
theorem HA.split {P env S} (g : Ty) : ∀ {n w}, HA P env S n w → HA P env (g :: S) n w ∨ HA P env S n g := by
  intro n w h
  induction h with
  | assume hw => exact Or.inl (HA.assume hw)
  | @impl n w c τ hS hc hh hb ih =>
    by_cases hwg : w = g
    · subst hwg; exact Or.inr (HA.impl c τ hS hc hh hb)
    · by_cases hall : ∀ x ∈ c.body, HA P env (g :: S) n (Ty.inst τ x)
      · refine Or.inl (HA.impl c τ ?_ hc hh hall)
        intro hm
        rcases List.mem_cons.mp hm with he | he
        · exact hwg he
        · exact hS he
      · have : ∃ x ∈ c.body, ¬ HA P env (g :: S) n (Ty.inst τ x) := by
          apply Classical.byContradiction
          intro hn; apply hall; intro x hx
          apply Classical.byContradiction
          intro hx'; exact hn ⟨x, hx, hx'⟩
        obtain ⟨x, hx, hnx⟩ := this
        rcases ih x hx with h' | h'
        · exact absurd h' hnx
        · exact Or.inr (HA.mono (by omega) h')

/-- a goal with a proof avoiding `S` that is not assumed has an impl whose where clauses have proofs avoiding the
goal itself as well -/
theorem HA_cut {P env S} : ∀ n g, HA P env S n g → g ∉ env →
    ∃ c ∈ P, ∃ τ, Ty.inst τ c.head = g ∧ ∃ m, m < n ∧ ∀ w ∈ c.body, HA P env (g :: S) m (Ty.inst τ w) := by
  intro n
  induction n using Nat.strongRecOn with
  | ind n ih =>
    intro g h hne
    cases h with
    | assume hg => exact absurd hg hne
    | @impl n' _ c τ hS hc hh hb =>
      by_cases hall : ∀ x ∈ c.body, HA P env (g :: S) n' (Ty.inst τ x)
      · exact ⟨c, hc, τ, hh, n', by omega, hall⟩
      · have : ∃ x ∈ c.body, ¬ HA P env (g :: S) n' (Ty.inst τ x) := by
          apply Classical.byContradiction
          intro hn; apply hall; intro x hx
          apply Classical.byContradiction
          intro hx'; exact hn ⟨x, hx, hx'⟩
        obtain ⟨x, hx, hnx⟩ := this
        rcases HA.split g (hb x hx) with h' | h'
        · exact absurd h' hnx
        · obtain ⟨c', hc', τ', hh', m, hm, hb'⟩ := ih n' (by omega) g h' hne
          exact ⟨c', hc', τ', hh', m, by omega, hb'⟩

theorem solve_no_avoid (P : Prog) (env : List Ty) :
    ∀ fuel S g, solve P env fuel S g = .no → ∀ n, ¬ HA P env S n g
  | 0, _, _, h => by cases h
  | fuel + 1, S, g, h => by
    intro n hn
    simp only [solve] at h
    split at h
    · cases h
    · next hgnd =>
      have hgnd : g.ground = true := by cases hgg : g.ground <;> simp_all
      split at h
      · cases h
      · next hne =>
        split at h
        · next hS =>
          cases hn with
          | assume hg => exact hne hg
          | impl _ _ hS' => exact hS' hS
        · obtain ⟨c, hc, τ, hh, m, _, hb⟩ := HA_cut n g hn hne
          have hcn := disj_no h _ (List.mem_map_of_mem _ hc)
          obtain ⟨σ, hm, hag⟩ := mtch_complete c.head τ Subst.empty (fun _ _ h => by cases h)
          rw [hh] at hm
          simp only [candidate, hm] at hcn
          split at hcn
          · next hgr =>
            obtain ⟨r, hr, hrn⟩ := conj_no hcn
            obtain ⟨w', hw', rfl⟩ := List.mem_map.mp hr
            obtain ⟨w, hw, rfl⟩ := List.mem_map.mp hw'
            have hwg : (apply σ w).ground = true := List.all_eq_true.mp hgr _ hw'
            have hm := hb w hw
            rw [← apply_eq_inst w σ τ hag hwg] at hm
            exact solve_no_avoid P env fuel (g :: S) (apply σ w) hrn m hm
          · cases hcn

theorem Holds.toHA {P env} : ∀ {g}, Holds P env g → ∃ n, HA P env [] n g := by
  intro g h
  induction h with
  | assume hg => exact ⟨0, HA.assume hg⟩
  | @impl g c τ hc hh _ ih =>
    -- one height for all the where clauses
    have : ∀ l : List Ty, (∀ w ∈ l, ∃ n, HA P env [] n (Ty.inst τ w)) →
        ∃ N, ∀ w ∈ l, HA P env [] N (Ty.inst τ w) := by
      intro l
      induction l with
      | nil => intro _; exact ⟨0, fun _ h => by cases h⟩
      | cons x xs ihl =>
        intro hl
        obtain ⟨a, ha⟩ := hl x (List.mem_cons_self _ _)
        obtain ⟨b, hb⟩ := ihl (fun w hw => hl w (List.mem_cons_of_mem _ hw))
        refine ⟨max a b, fun w hw => ?_⟩
        rcases List.mem_cons.mp hw with he | he
        · subst he; exact HA.mono (Nat.le_max_left _ _) ha
        · exact HA.mono (Nat.le_max_right _ _) (hb w he)
    obtain ⟨N, hN⟩ := this c.body ih
    exact ⟨N + 1, HA.impl c τ (fun h => by cases h) hc hh hN⟩

theorem solve_no_sound (P : Prog) (env : List Ty) (fuel : Nat) (g : Ty) :
    solve P env fuel [] g = .no → ¬ Holds P env g := by
  intro h hh
  obtain ⟨n, hn⟩ := hh.toHA
  exact solve_no_avoid P env fuel [] g h n hn

/-! ## Completeness: a goal that holds gets `yes`, with fuel enough

A ground goal proved by impls whose where clauses only name parameters their heads bind (rustc's rule E0207 for
impl parameters makes most of them so) gets `yes` with more fuel than the proof's height. -/

/-- every parameter a where clause names, its head binds -/
def Clause.WF (c : Clause) : Prop := ∀ w ∈ c.body, ∀ x, w.Occ x → c.head.Occ x

theorem ground_inst : ∀ (t : Ty) (τ : Nat → Ty), (∀ x, t.Occ x → (τ x).ground = true) → (Ty.inst τ t).ground = true
  | var n, τ, h => h n rfl
  | con _, _, _ => rfl
  | app f a, τ, h => by
    simp only [Ty.inst, Ty.ground, Bool.and_eq_true]
    exact ⟨ground_inst f τ (fun x hx => h x (Or.inl hx)), ground_inst a τ (fun x hx => h x (Or.inr hx))⟩

theorem ground_occ : ∀ (t : Ty) (τ : Nat → Ty), (Ty.inst τ t).ground = true → ∀ x, t.Occ x → (τ x).ground = true
  | var n, τ, h, x, hx => by simp only [Ty.Occ] at hx; subst hx; exact h
  | con _, _, _, x, hx => by simp [Ty.Occ] at hx
  | app f a, τ, h, x, hx => by
    simp only [Ty.inst, Ty.ground, Bool.and_eq_true] at h
    rcases hx with hx | hx
    · exact ground_occ f τ h.1 x hx
    · exact ground_occ a τ h.2 x hx

/-- Proofs of height at most `n` (no stack). -/
inductive HN (P : Prog) (env : List Ty) : Nat → Ty → Prop where
  | assume {n g} : g ∈ env → HN P env n g
  | impl {n g} (c : Clause) (τ : Nat → Ty) : c ∈ P → Ty.inst τ c.head = g →
      (∀ w ∈ c.body, HN P env n (Ty.inst τ w)) → HN P env (n + 1) g

/-- with fuel above the height of a proof and none of the stack in it, a ground goal gets `yes`
(the stack only removes candidates when the goal comes round again: `HA` with an empty stack and a cut proof).
Stated over `HA` so the stack can grow. -/
theorem solve_complete_HA (P : Prog) (env : List Ty) (hwf : ∀ c ∈ P, c.WF) :
    ∀ n S g, HA P env S n g → g.ground = true → solve P env (n + 1) S g = .yes := by
  intro n
  induction n with
  | zero =>
    intro S g h hg
    cases h with
    | assume he => simp [solve, hg, he]
  | succ n ih =>
    intro S g h hg
    cases h with
    | assume he => simp [solve, hg, he]
    | @impl _ _ c₀ τ₀ hS hc₀ hh₀ hb₀ =>
      rw [solve]
      have hgf : ¬ (g.ground = false) := by simp [hg]
      rw [if_neg hgf]
      split
      · rfl
      · next hne =>
        try rw [if_neg hS]
        obtain ⟨c, hc, τ, hh, m, hmn, hb⟩ := HA_cut (n + 1) g (HA.impl c₀ τ₀ hS hc₀ hh₀ hb₀) hne
        have hb : ∀ w ∈ c.body, HA P env (g :: S) n (Ty.inst τ w) := fun w hw => HA.mono (by omega) (hb w hw)
        apply disj_of_yes (List.mem_map_of_mem _ hc)
        obtain ⟨σ, hm, hag⟩ := mtch_complete c.head τ Subst.empty (fun _ _ h => by cases h)
        rw [hh] at hm
        simp only [candidate, hm]
        -- every parameter of the head is bound to a ground type
        have hhead : ∀ x, c.head.Occ x → (τ x).ground = true := by
          rw [← hh] at hg; exact ground_occ c.head τ hg
        have hbound : ∀ x, c.head.Occ x → σ x = some (τ x) := by
          intro x hx
          have hb := mtch_binds _ _ _ _ hm x hx
          cases hs : σ x with
          | none => rw [hs] at hb; cases hb
          | some u => rw [hag x u hs]
        have hws : ∀ w ∈ c.body, apply σ w = Ty.inst τ w := by
          intro w hw
          have : ∀ t : Ty, (∀ x, t.Occ x → c.head.Occ x) → apply σ t = Ty.inst τ t := by
            intro t
            induction t with
            | var x => intro ht; simp [apply, Ty.inst, hbound x (ht x rfl)]
            | con _ => intro _; rfl
            | app f a ihf iha =>
              intro ht
              simp only [apply, Ty.inst]
              rw [ihf (fun x hx => ht x (Or.inl hx)), iha (fun x hx => ht x (Or.inr hx))]
          exact this w (hwf c hc w hw)
        have hwg : ∀ w ∈ c.body, (Ty.inst τ w).ground = true := fun w hw =>
          ground_inst w τ (fun x hx => hhead x (hwf c hc w hw x hx))
        have hall : (c.body.map (apply σ)).all Ty.ground = true := by
          apply List.all_eq_true.mpr
          intro w' hw'
          obtain ⟨w, hw, rfl⟩ := List.mem_map.mp hw'
          rw [hws w hw]; exact hwg w hw
        rw [if_pos hall]
        apply conj_all_yes
        intro r hr
        obtain ⟨w', hw', rfl⟩ := List.mem_map.mp hr
        obtain ⟨w, hw, rfl⟩ := List.mem_map.mp hw'
        rw [hws w hw]
        exact ih (g :: S) _ (hb w hw) (hwg w hw)

/-- Completeness: a ground goal that holds (by well-formed impls) gets `yes` once the fuel is enough. -/
theorem solve_complete (P : Prog) (env : List Ty) (hwf : ∀ c ∈ P, c.WF) (g : Ty) (hg : g.ground = true) :
    Holds P env g → ∃ fuel, solve P env fuel [] g = .yes := by
  intro h
  obtain ⟨n, hn⟩ := h.toHA
  exact ⟨n + 1, solve_complete_HA P env hwf n [] g hn hg⟩

end TraitSolver
