import Graphcal.Graph.RecurrenceGroup.Spec
import Graphcal.Graph.RecurrenceGroup.Reference

/-!
# Recurrence groups: list and relation lemmas

Generic facts used by the checker and evaluator proofs.
-/

namespace Graphcal.Graph.RecurrenceGroup

/-! ## Transitive closure -/

theorem transGen_map {α β : Type} {r : α → α → Prop} {s : β → β → Prop} (f : α → β)
    (hrs : ∀ a b, r a b → s (f a) (f b)) {a b : α} (h : Relation.TransGen r a b) :
    Relation.TransGen s (f a) (f b) := by
  induction h with
  | single hab => exact .single (hrs _ _ hab)
  | tail _ hbc ih => exact .tail ih (hrs _ _ hbc)

theorem transGen_mono {α : Type} {r s : α → α → Prop} (hrs : ∀ a b, r a b → s a b)
    {a b : α} (h : Relation.TransGen r a b) : Relation.TransGen s a b :=
  transGen_map id hrs h

/-- A relation that strictly decreases a measure has no cycle. -/
theorem transGen_lt_of_measure {α : Type} {r : α → α → Prop} (f : α → Nat)
    (hdec : ∀ a b, r a b → f b < f a) {a b : α} (h : Relation.TransGen r a b) : f b < f a := by
  induction h with
  | single hab => exact hdec _ _ hab
  | tail _ hbc ih => exact Nat.lt_trans (hdec _ _ hbc) ih

theorem not_transGen_self_of_measure {α : Type} {r : α → α → Prop} (f : α → Nat)
    (hdec : ∀ a b, r a b → f b < f a) (a : α) : ¬ Relation.TransGen r a a :=
  fun h => Nat.lt_irrefl _ (transGen_lt_of_measure f hdec h)

/-! ## Chains -/

theorem chainFrom_snoc {α : Type} {r : α → α → Prop} :
    ∀ {a : α} {l : List α} {b c : α},
      ChainFrom r a (l ++ [b]) → r b c → ChainFrom r a (l ++ [b] ++ [c])
  | _, [], _, _, hchain, hbc => ⟨hchain.1, hbc, trivial⟩
  | _, _ :: _, _, _, hchain, hbc => ⟨hchain.1, chainFrom_snoc hchain.2 hbc⟩

theorem chainFrom_suffix {α : Type} {r : α → α → Prop} :
    ∀ {a x : α} {pre post : List α}, ChainFrom r a (pre ++ x :: post) → ChainFrom r x post
  | _, _, [], _, hchain => hchain.2
  | _, _, _ :: _, _, hchain => chainFrom_suffix hchain.2

theorem chainFrom_transGen {α : Type} {r : α → α → Prop} :
    ∀ {a b : α} {l : List α}, ChainFrom r a (l ++ [b]) → Relation.TransGen r a b
  | _, _, [], hchain => .single hchain.1
  | _, _, _ :: _, hchain => Relation.TransGen.trans (.single hchain.1) (chainFrom_transGen hchain.2)

/-! ## Finite ranges -/

/-- `find?` over `finRange` returns the least member satisfying the predicate. -/
theorem find?_finRange_least {n : Nat} {p : Fin n → Bool} {x : Fin n}
    (h : (List.finRange n).find? p = some x) : ∀ y, y < x → p y = false := by
  intro y hyx
  obtain ⟨_, before, after, hsplit, hbefore⟩ := List.find?_eq_some_iff_append.1 h
  have hy : y ∈ before ++ x :: after := hsplit ▸ List.mem_finRange y
  have hsorted : List.Pairwise (· < ·) (before ++ x :: after) :=
    hsplit ▸ List.pairwise_lt_finRange n
  rcases List.mem_append.1 hy with hmem | hmem
  · simpa using hbefore y hmem
  · rcases List.mem_cons.1 hmem with heq | hmem
    · exact absurd hyx (heq ▸ Nat.lt_irrefl _)
    · have hxy : x < y := (List.pairwise_cons.1 (List.pairwise_append.1 hsorted).2.1).1 y hmem
      exact absurd (Nat.lt_trans hxy hyx) (Nat.lt_irrefl _)

theorem length_le_of_nodup {n : Nat} {l : List (Fin n)} (h : l.Nodup) : l.length ≤ n := by
  have := h.length_le_of_subset (l₂ := List.finRange n) fun x _ => List.mem_finRange x
  simpa using this

/-- A duplicate-free list of `n` members of `Fin n` lists every member. -/
theorem mem_of_nodup_of_length {n : Nat} {l : List (Fin n)} (h : l.Nodup) (hlen : n ≤ l.length)
    (x : Fin n) : x ∈ l := by
  refine Classical.byContradiction fun hx => ?_
  have hsub : l ⊆ (List.finRange n).erase x := by
    intro y hy
    have hne : y ≠ x := fun heq => hx (heq ▸ hy)
    exact (List.mem_erase_of_ne hne).2 (List.mem_finRange y)
  have hle := h.length_le_of_subset hsub
  rw [List.length_erase_of_mem (List.mem_finRange x), List.length_finRange] at hle
  have := x.pos
  omega

/-! ## Suffixes and positions -/

theorem suffixFrom_spec {α : Type} [DecidableEq α] {x : α} :
    ∀ {l : List α}, x ∈ l → ∃ pre post, l = pre ++ x :: post ∧ Group.suffixFrom x l = x :: post
  | [], hx => absurd hx List.not_mem_nil
  | y :: rest, hx => by
      by_cases hyx : y = x
      · subst hyx
        exact ⟨[], rest, rfl, by simp [Group.suffixFrom]⟩
      · have hrest : x ∈ rest := by
          rcases List.mem_cons.1 hx with heq | hmem
          · exact absurd heq.symm hyx
          · exact hmem
        obtain ⟨pre, post, hsplit, hsuffix⟩ := suffixFrom_spec hrest
        exact ⟨y :: pre, post, by simp [hsplit], by simp [Group.suffixFrom, hyx, hsuffix]⟩

/-- An element whose position precedes `e` lies in the part before `e`. -/
theorem mem_before_of_idxOf_lt {α : Type} [DecidableEq α] {pre post : List α} {d e : α}
    (he : e ∉ pre) (hlt : (pre ++ e :: post).idxOf d < (pre ++ e :: post).idxOf e) :
    d ∈ pre := by
  refine Classical.byContradiction fun hd => ?_
  rw [List.idxOf_append, List.idxOf_append, if_neg hd, if_neg he,
    List.idxOf_cons_self] at hlt
  omega

/-! ## Folds that update one point at a time -/

section Fold

variable {size : Nat} {Val Err : Type}

theorem foldl_update_other
    (step : (Element size → Status Val Err) → Element size → Status Val Err) :
    ∀ (l : List (Element size)) (status : Element size → Status Val Err) {e : Element size},
      e ∉ l →
      (l.foldl (fun status d => update status d (step status d)) status) e = status e
  | [], _, _, _ => rfl
  | d :: rest, status, e, he => by
      have hne : e ≠ d := fun heq => he (heq ▸ List.mem_cons_self)
      have hrest : e ∉ rest := fun hmem => he (List.mem_cons_of_mem d hmem)
      simp only [List.foldl_cons]
      rw [foldl_update_other step rest _ hrest]
      simp [update, hne]

end Fold

theorem all_congr_mem {α : Type} {l : List α} {p q : α → Bool} (h : ∀ a ∈ l, p a = q a) :
    l.all p = l.all q := by
  induction l with
  | nil => rfl
  | cons a rest ih =>
      simp only [List.all_cons]
      rw [h a List.mem_cons_self, ih fun b hb => h b (List.mem_cons_of_mem a hb)]

end Graphcal.Graph.RecurrenceGroup
