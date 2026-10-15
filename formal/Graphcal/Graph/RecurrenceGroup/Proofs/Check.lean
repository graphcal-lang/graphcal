import Graphcal.Graph.RecurrenceGroup.Proofs.Lists

/-!
# Recurrence groups: checker proofs

The reference checker accepts exactly the groups satisfying R1–R7, reports a
violation that `Violation.Reported` describes, and returns the plan order.
-/

namespace Graphcal.Graph.RecurrenceGroup

namespace Group

variable {size : Nat} {g : Group size}

/-! ## Same-step targets and readiness -/

theorem mem_sameTargets {x y : Fin size} : y ∈ g.sameTargets x ↔ g.SameReads x y := by
  simp only [sameTargets, List.mem_filterMap, SameReads]
  constructor
  · rintro ⟨r, hr, h⟩
    split at h
    · rename_i hc
      cases h
      exact ⟨r, hr, hc.1, rfl, hc.2⟩
    · cases h
  · rintro ⟨r, hr, hs, ht, ha⟩
    exact ⟨r, hr, by rw [if_pos ⟨hs, ha⟩, ht]⟩

theorem readyB_iff {placed : List (Fin size)} {x : Fin size} :
    g.readyB placed x = true ↔ g.Ready placed x := by
  simp only [readyB, Bool.and_eq_true, decide_eq_true_eq, List.all_eq_true, Ready,
    mem_sameTargets]

theorem mem_unplaced {placed : List (Fin size)} {x : Fin size} :
    x ∈ unplaced placed ↔ x ∉ placed := by
  simp [unplaced, List.mem_filter, List.mem_finRange]

theorem unplaced_eq_nil {placed : List (Fin size)} :
    unplaced placed = [] ↔ ∀ x, x ∈ placed := by
  simp [unplaced, List.filter_eq_nil_iff, List.mem_finRange]

/-! ## Greedy placement -/

/-- Invariant of greedy placement: every placed member follows the members it
reads at the same step. -/
structure KahnInv (g : Group size) (placed : List (Fin size)) : Prop where
  nodup : placed.Nodup
  earlier : ∀ x ∈ placed, ∀ y, g.SameReads x y → y ∈ placed ∧ placed.idxOf y < placed.idxOf x

theorem kahnInv_nil : g.KahnInv [] :=
  ⟨List.nodup_nil, fun _ hx => absurd hx List.not_mem_nil⟩

theorem kahnInv_place {placed : List (Fin size)} {x : Fin size} (h : g.KahnInv placed)
    (hx : g.Ready placed x) : g.KahnInv (placed ++ [x]) := by
  refine ⟨?_, ?_⟩
  · refine List.nodup_append.2 ⟨h.nodup, List.nodup_cons.2 ⟨List.not_mem_nil, List.nodup_nil⟩, ?_⟩
    intro a ha b hb heq
    rw [List.mem_singleton] at hb
    exact hx.1 (hb ▸ heq ▸ ha)
  · intro z hz y hzy
    rcases List.mem_append.1 hz with hz | hz
    · obtain ⟨hy, hlt⟩ := h.earlier z hz y hzy
      refine ⟨List.mem_append_left _ hy, ?_⟩
      rw [List.idxOf_append, List.idxOf_append, if_pos hy, if_pos hz]
      exact hlt
    · rw [List.mem_singleton] at hz
      subst hz
      have hy := hx.2 y hzy
      refine ⟨List.mem_append_left _ hy, ?_⟩
      rw [List.idxOf_append, List.idxOf_append, if_pos hy, if_neg hx.1, List.idxOf_cons_self]
      have := List.idxOf_lt_length_of_mem hy
      omega

theorem kahn_ok : ∀ (fuel : Nat) (placed order : List (Fin size)), g.KahnInv placed →
    g.kahn fuel placed = .ok order → g.KahnInv order ∧ ∀ x, x ∈ order
  | 0, placed, order, hinv, h => by
      unfold kahn at h
      split at h
      · rename_i hnil
        cases h
        exact ⟨hinv, unplaced_eq_nil.1 hnil⟩
      · cases h
  | fuel + 1, placed, order, hinv, h => by
      unfold kahn at h
      split at h
      · rename_i hnil
        cases h
        exact ⟨hinv, unplaced_eq_nil.1 hnil⟩
      · split at h
        · rename_i x hx
          exact kahn_ok fuel _ order (kahnInv_place hinv (readyB_iff.1 (List.find?_some hx))) h
        · cases h

theorem kahn_ok_greedy : ∀ (fuel : Nat) (placed order : List (Fin size)),
    g.kahn fuel placed = .ok order → ∃ rest, order = placed ++ rest ∧ g.GreedyFrom placed rest
  | 0, placed, order, h => by
      unfold kahn at h
      split at h
      · rename_i hnil
        cases h
        exact ⟨[], by simp, .complete (unplaced_eq_nil.1 hnil)⟩
      · cases h
  | fuel + 1, placed, order, h => by
      unfold kahn at h
      split at h
      · rename_i hnil
        cases h
        exact ⟨[], by simp, .complete (unplaced_eq_nil.1 hnil)⟩
      · split at h
        · rename_i x hx
          have hready := readyB_iff.1 (List.find?_some hx)
          have hleast : ∀ y, g.Ready placed y → x ≤ y := by
            intro y hy
            refine Classical.byContradiction fun hle => ?_
            have hfalse := find?_finRange_least hx y (Fin.not_le.1 hle)
            rw [readyB_iff.2 hy] at hfalse
            cases hfalse
          obtain ⟨rest, hord, hgreedy⟩ := kahn_ok_greedy fuel (placed ++ [x]) order h
          exact ⟨x :: rest, by simp [hord], .place hready hleast hgreedy⟩
        · cases h

/-- Members that cannot be placed all wait on one another. -/
def Deadlocked (g : Group size) (stuck : List (Fin size)) : Prop :=
  ∀ x ∈ stuck, ∃ y ∈ stuck, g.SameReads x y

theorem kahn_error : ∀ (fuel : Nat) (placed stuck : List (Fin size)), placed.Nodup →
    placed.length + fuel = size → g.kahn fuel placed = .error stuck →
    stuck ≠ [] ∧ g.Deadlocked stuck
  | 0, placed, stuck, hnodup, hlen, h => by
      unfold kahn at h
      split at h
      · cases h
      · rename_i hnil
        exact absurd (unplaced_eq_nil.2 fun x => mem_of_nodup_of_length hnodup (by omega) x) hnil
  | fuel + 1, placed, stuck, hnodup, hlen, h => by
      unfold kahn at h
      split at h
      · cases h
      · rename_i hnil
        split at h
        · rename_i x hx
          have hready := readyB_iff.1 (List.find?_some hx)
          refine kahn_error fuel _ stuck ?_ (by simp; omega) h
          refine List.nodup_append.2 ⟨hnodup, List.nodup_cons.2 ⟨List.not_mem_nil, List.nodup_nil⟩, ?_⟩
          intro a ha b hb heq
          rw [List.mem_singleton] at hb
          exact hready.1 (hb ▸ heq ▸ ha)
        · rename_i hnone
          cases h
          refine ⟨hnil, fun x hx => ?_⟩
          have hxn := mem_unplaced.1 hx
          have hnr : ¬ g.Ready placed x := fun hr => by
            have := List.find?_eq_none.1 hnone x (List.mem_finRange x)
            exact this (readyB_iff.2 hr)
          have hnot : ¬ ∀ y, g.SameReads x y → y ∈ placed := fun hall => hnr ⟨hxn, hall⟩
          obtain ⟨y, hy⟩ := Classical.not_forall.1 hnot
          refine ⟨y, mem_unplaced.2 fun hmem => hy fun _ => hmem, ?_⟩
          exact Classical.byContradiction fun hxy => hy fun h' => absurd h' hxy

/-! ## Cycle extraction -/

theorem stuckNext_spec {stuck : List (Fin size)} (hdead : g.Deadlocked stuck) {x : Fin size}
    (hx : x ∈ stuck) :
    g.SameReads x (g.stuckNext stuck x) ∧ g.stuckNext stuck x ∈ stuck := by
  obtain ⟨y, hy, hxy⟩ := hdead x hx
  unfold stuckNext
  cases hf : (g.sameTargets x).find? (fun y => decide (y ∈ stuck)) with
  | none =>
      have := List.find?_eq_none.1 hf y (mem_sameTargets.2 hxy)
      simp [hy] at this
  | some z =>
      simp only [Option.getD_some]
      exact ⟨mem_sameTargets.1 (List.mem_of_find?_eq_some hf), by simpa using List.find?_some hf⟩

/-- The visited members of a walk, ending at the member about to be visited. -/
def WalkChain (g : Group size) : List (Fin size) → Fin size → Prop
  | [], _ => True
  | v :: rest, x => ChainFrom g.SameReads v (rest ++ [x])

theorem walkCycle_spec {stuck : List (Fin size)} (hdead : g.Deadlocked stuck) :
    ∀ (fuel : Nat) (visited : List (Fin size)) (x : Fin size), visited.Nodup →
      (∀ v ∈ visited, v ∈ stuck) → x ∈ stuck → g.WalkChain visited x →
      visited.length + fuel = size + 1 → g.IsSameStepCycle (g.walkCycle stuck fuel visited x)
  | 0, visited, _, hnodup, _, _, _, hlen => by
      have := length_le_of_nodup hnodup
      omega
  | fuel + 1, visited, x, hnodup, hvisited, hx, hchain, hlen => by
      unfold walkCycle
      split
      · rename_i hmem
        obtain ⟨pre, post, hsplit, hsuffix⟩ := suffixFrom_spec hmem
        rw [hsuffix]
        show ChainFrom g.SameReads x (post ++ [x])
        cases visited with
        | nil => exact absurd hmem List.not_mem_nil
        | cons v rest =>
            cases pre with
            | nil =>
                simp only [List.nil_append, List.cons.injEq] at hsplit
                obtain ⟨rfl, rfl⟩ := hsplit
                exact hchain
            | cons p pre' =>
                simp only [List.cons_append, List.cons.injEq] at hsplit
                obtain ⟨rfl, rfl⟩ := hsplit
                have hchain' : ChainFrom g.SameReads v (pre' ++ x :: (post ++ [x])) := by
                  simpa [WalkChain] using hchain
                exact chainFrom_suffix hchain'
      · rename_i hnot
        have hnext := stuckNext_spec hdead hx
        refine walkCycle_spec hdead fuel _ _ ?_ ?_ hnext.2 ?_ (by simp; omega)
        · refine List.nodup_append.2 ⟨hnodup, List.nodup_cons.2 ⟨List.not_mem_nil, List.nodup_nil⟩, ?_⟩
          intro a ha b hb heq
          rw [List.mem_singleton] at hb
          exact hnot (hb ▸ heq ▸ ha)
        · intro v hv
          rcases List.mem_append.1 hv with hv | hv
          · exact hvisited v hv
          · rw [List.mem_singleton] at hv
            exact hv ▸ hx
        · cases visited with
          | nil => exact ⟨hnext.1, trivial⟩
          | cons v rest =>
              show ChainFrom g.SameReads v (rest ++ [x] ++ [g.stuckNext stuck x])
              exact chainFrom_snoc hchain hnext.1

theorem extractCycle_spec {stuck : List (Fin size)} (hne : stuck ≠ [])
    (hdead : g.Deadlocked stuck) : g.IsSameStepCycle (g.extractCycle stuck) := by
  cases stuck with
  | nil => exact absurd rfl hne
  | cons x rest =>
      exact walkCycle_spec hdead (size + 1) [] x List.nodup_nil (fun _ hv => absurd hv List.not_mem_nil)
        List.mem_cons_self trivial (by simp)

theorem sameStepCycle_of_isSameStepCycle :
    ∀ {cycle : List (Fin size)}, g.IsSameStepCycle cycle → g.SameStepCycle
  | [], h => h.elim
  | x :: _, h => ⟨x, chainFrom_transGen h⟩

/-! ## Valid and greedy orders -/

theorem ValidOrder.noSameStepCycle {order : List (Fin size)} (h : g.ValidOrder order) :
    ¬ g.SameStepCycle := by
  rintro ⟨x, hx⟩
  exact not_transGen_self_of_measure (fun y => order.idxOf y) (fun a b hab => h.sameReadsEarlier a b hab) x hx

theorem kahn_validOrder {order : List (Fin size)} (h : g.kahn size [] = .ok order) :
    g.ValidOrder order := by
  obtain ⟨hinv, hall⟩ := kahn_ok size [] order kahnInv_nil h
  exact ⟨hinv.nodup, hall, fun x y hxy => (hinv.earlier x (hall x) y hxy).2⟩

theorem greedyFrom_inv {placed rest : List (Fin size)} (h : g.GreedyFrom placed rest) :
    g.KahnInv placed → g.KahnInv (placed ++ rest) ∧ ∀ x, x ∈ placed ++ rest := by
  induction h with
  | complete hall => intro hinv; simpa using ⟨hinv, hall⟩
  | place hready _ _ ih =>
      intro hinv
      simpa [List.append_assoc] using ih (kahnInv_place hinv hready)

/-- The plan order is a valid order. -/
theorem PlanOrder.validOrder {order : List (Fin size)} (h : g.PlanOrder order) :
    g.ValidOrder order := by
  obtain ⟨hinv, hall⟩ := greedyFrom_inv h kahnInv_nil
  simp only [List.nil_append] at hinv hall
  exact ⟨hinv.nodup, hall, fun x y hxy => (hinv.earlier x (hall x) y hxy).2⟩

theorem greedyFrom_unique {placed o₁ o₂ : List (Fin size)} (h₁ : g.GreedyFrom placed o₁)
    (h₂ : g.GreedyFrom placed o₂) : o₁ = o₂ := by
  induction h₁ generalizing o₂ with
  | complete hall =>
      cases h₂ with
      | complete _ => rfl
      | place hready _ _ => exact absurd (hall _) hready.1
  | place hready hleast _ ih =>
      cases h₂ with
      | complete hall => exact absurd (hall _) hready.1
      | place hready' hleast' hrest' =>
          have hxx := Fin.le_antisymm (hleast _ hready') (hleast' _ hready)
          subst hxx
          rw [ih hrest']

/-- A group has at most one plan order. -/
theorem planOrder_unique {o₁ o₂ : List (Fin size)} (h₁ : g.PlanOrder o₁) (h₂ : g.PlanOrder o₂) :
    o₁ = o₂ :=
  greedyFrom_unique h₁ h₂

/-! ## Rule lemmas -/

theorem find?_finRange_eq_none {p : Fin size → Bool} :
    (List.finRange size).find? p = none ↔ ∀ x, p x = false := by
  simp [List.find?_eq_none, List.mem_finRange]

theorem callableOwned_iff :
    (List.finRange size).find? (fun x => decide ((g.member x).owner ≠ .callable)) = none ↔
      g.CallableOwned := by
  simp [CallableOwned]

theorem finished_iff :
    (List.finRange size).find? (fun x => decide ((g.member x).definition = .unfinished)) = none ↔
      g.Finished := by
  simp [Finished]

theorem recurrenceShaped_iff :
    (List.finRange size).find? (fun x => decide (g.shape? x = none)) = none ↔
      g.RecurrenceShaped := by
  simp [RecurrenceShaped]

theorem commonAxis_iff :
    (List.finRange size).find? (fun x => !g.axisMatchesFirst x) = none ↔ g.CommonAxis := by
  simp only [find?_finRange_eq_none, Bool.not_eq_false', axisMatchesFirst, decide_eq_true_eq,
    CommonAxis]
  constructor
  · intro h x y
    rw [h x, h y]
  · intro h x
    exact h _ _

theorem noSelfRead_iff :
    g.reads.find? (fun r => decide (r.source = r.target)) = none ↔ g.NoSelfRead := by
  simp [List.find?_eq_none, NoSelfRead]

theorem stepKeyed_iff :
    g.reads.find? (fun r => decide (g.access? r = none)) = none ↔ g.StepKeyed := by
  simp [List.find?_eq_none, StepKeyed]

/-! ## The checker against the specification -/

/-- Every accepted plan comes from an accepted group. -/
theorem accepted_of_check_ok {plan : Plan size} (h : g.check = .ok plan) : g.Accepted := by
  unfold check at h
  split at h
  · cases h
  rename_i h1
  split at h
  · cases h
  rename_i h2
  split at h
  · cases h
  rename_i h3
  split at h
  · cases h
  rename_i h4
  split at h
  · cases h
  rename_i h5
  split at h
  · cases h
  rename_i h6
  split at h
  · rename_i order hk
    exact ⟨⟨callableOwned_iff.1 h1, finished_iff.1 h2, recurrenceShaped_iff.1 h3,
      commonAxis_iff.1 h4⟩, noSelfRead_iff.1 h5, stepKeyed_iff.1 h6,
      (kahn_validOrder hk).noSameStepCycle⟩
  · cases h

/-- Every accepted group gets a plan. -/
theorem check_ok_of_accepted (h : g.Accepted) : ∃ plan, g.check = .ok plan := by
  unfold check
  rw [callableOwned_iff.2 h.memberRules.callableOwned, finished_iff.2 h.memberRules.finished,
    recurrenceShaped_iff.2 h.memberRules.recurrenceShaped,
    commonAxis_iff.2 h.memberRules.commonAxis, noSelfRead_iff.2 h.noSelfRead,
    stepKeyed_iff.2 h.stepKeyed]
  simp only
  split
  · rename_i order _
    exact ⟨⟨order⟩, rfl⟩
  · rename_i stuck hk
    obtain ⟨hne, hdead⟩ := kahn_error size [] stuck List.nodup_nil (by simp) hk
    exact absurd (sameStepCycle_of_isSameStepCycle (extractCycle_spec hne hdead)) h.noSameStepCycle

/-- The checker accepts exactly the groups satisfying R1–R7. -/
theorem check_ok_iff_accepted : (∃ plan, g.check = .ok plan) ↔ g.Accepted :=
  ⟨fun ⟨_, h⟩ => accepted_of_check_ok h, check_ok_of_accepted⟩

/-- The plan returned by the checker is the group's plan order. -/
theorem check_ok_planOrder {plan : Plan size} (h : g.check = .ok plan) : g.PlanOrder plan.order := by
  unfold check at h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · rename_i order hk
    cases h
    obtain ⟨rest, hord, hgreedy⟩ := kahn_ok_greedy size [] order hk
    simp only [List.nil_append] at hord
    subst hord
    exact hgreedy
  · cases h

/-- Every rejection reports the violation that the specification selects. -/
theorem check_error_reported {violation : Violation size} (h : g.check = .error violation) :
    violation.Reported g := by
  unfold check at h
  split at h
  · rename_i x hx
    cases h
    refine ⟨by simpa using List.find?_some hx, fun y hy => ?_⟩
    simpa using find?_finRange_least hx y hy
  rename_i h1
  split at h
  · rename_i x hx
    cases h
    refine ⟨callableOwned_iff.1 h1, by simpa using List.find?_some hx, fun y hy => ?_⟩
    simpa using find?_finRange_least hx y hy
  rename_i h2
  split at h
  · rename_i x hx
    cases h
    refine ⟨callableOwned_iff.1 h1, finished_iff.1 h2, by simpa using List.find?_some hx,
      fun y hy => ?_⟩
    simpa using find?_finRange_least hx y hy
  rename_i h3
  split at h
  · rename_i x hx
    cases h
    have hfirst : ∀ y : Fin size, (⟨0, y.pos⟩ : Fin size) = ⟨0, x.pos⟩ := fun _ => rfl
    refine ⟨callableOwned_iff.1 h1, finished_iff.1 h2, recurrenceShaped_iff.1 h3, ?_, fun y hy => ?_⟩
    · simpa [axisMatchesFirst] using List.find?_some hx
    · have := find?_finRange_least hx y hy
      simp only [axisMatchesFirst, Bool.not_eq_false', decide_eq_true_eq] at this
      rw [this, hfirst y]
  rename_i h4
  have hmember : g.MemberRules :=
    ⟨callableOwned_iff.1 h1, finished_iff.1 h2, recurrenceShaped_iff.1 h3, commonAxis_iff.1 h4⟩
  split at h
  · rename_i r hr
    cases h
    obtain ⟨hp, before, after, hsplit, hbefore⟩ := List.find?_eq_some_iff_append.1 hr
    exact ⟨hmember, by simpa using hp, before, after, hsplit, fun r' hr' => by
      simpa using hbefore r' hr'⟩
  rename_i h5
  split at h
  · rename_i r hr
    cases h
    obtain ⟨hp, before, after, hsplit, hbefore⟩ := List.find?_eq_some_iff_append.1 hr
    exact ⟨hmember, noSelfRead_iff.1 h5, by simpa using hp, before, after, hsplit,
      fun r' hr' => by simpa using hbefore r' hr'⟩
  rename_i h6
  split at h
  · cases h
  · rename_i stuck hk
    cases h
    obtain ⟨hne, hdead⟩ := kahn_error size [] stuck List.nodup_nil (by simp) hk
    exact ⟨hmember, noSelfRead_iff.1 h5, stepKeyed_iff.1 h6, extractCycle_spec hne hdead⟩

end Group

end Graphcal.Graph.RecurrenceGroup
