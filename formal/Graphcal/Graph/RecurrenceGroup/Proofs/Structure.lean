import Graphcal.Graph.RecurrenceGroup.Proofs.Evaluation

/-!
# Recurrence groups: structural consequences of the rule

An accepted group's element graph is acyclic on every axis; every accepted
group has an `unfold` member and therefore steps along a coordinate axis; R7
rejects only groups whose element graph has a cycle on every axis with at least
two elements; and a cycle without an `unfold` member gets the plain diagnostic.
-/

namespace Graphcal.Graph.RecurrenceGroup

namespace Group

variable {size : Nat} {g : Group size}

/-- An accepted group has a plan order, and it is valid. -/
theorem Accepted.planOrder (h : g.Accepted) :
    ∃ order, g.PlanOrder order ∧ g.ValidOrder order := by
  obtain ⟨plan, hplan⟩ := check_ok_of_accepted h
  exact ⟨plan.order, check_ok_planOrder hplan, (check_ok_planOrder hplan).validOrder⟩

/-- Soundness: the element graph of an accepted group is acyclic on every axis. -/
theorem Accepted.elementAcyclic (h : g.Accepted) (n : Nat) (e : Element size) :
    ¬ Relation.TransGen (g.ElementDepWithin n) e e := by
  obtain ⟨_, _, hv⟩ := h.planOrder
  intro hcycle
  exact hv.elementAcyclic e (transGen_mono (fun _ _ hab => hab.2) hcycle)

/-- With only `for` members, every step-keyed read is a same-step read. -/
theorem sameReads_of_reads_without_unfold (hshaped : g.RecurrenceShaped) (hkeyed : g.StepKeyed)
    (hno : ∀ x, g.shape? x ≠ some .unfoldBody) {x y : Fin size} (h : g.Reads x y) :
    g.SameReads x y := by
  obtain ⟨r, hr, hs, ht⟩ := h
  refine ⟨r, hr, hs, ht, ?_⟩
  have hkey := hkeyed r hr
  cases hshape : g.shape? r.source with
  | none => exact absurd hshape (hshaped _)
  | some shape =>
      cases shape with
      | unfoldBody => exact absurd hshape (hno _)
      | forBody =>
          rw [access?_eq hshape] at hkey ⊢
          generalize r.form = form at hkey ⊢
          cases form with
          | element key => cases key <;> simp_all [Shape.access]
          | whole => simp [Shape.access] at hkey
          | implicit => simp [Shape.access] at hkey

/-- Every accepted group has an `unfold` member. -/
theorem Accepted.hasUnfoldMember (h : g.Accepted) (hcycle : g.ReadCycle) :
    ∃ x, g.shape? x = some .unfoldBody := by
  refine Classical.byContradiction fun hno => ?_
  have hno' : ∀ x, g.shape? x ≠ some .unfoldBody := fun x hx => hno ⟨x, hx⟩
  obtain ⟨x, hx⟩ := hcycle
  exact h.noSameStepCycle ⟨x, transGen_mono (fun _ _ hab =>
    sameReads_of_reads_without_unfold h.memberRules.recurrenceShaped h.stepKeyed hno' hab) hx⟩

/-- Every accepted group steps along a coordinate axis. -/
theorem Accepted.axisCoordinate (kind : AxisId → AxisKind) (htyped : g.UnfoldAxesCoordinate kind)
    (h : g.Accepted) (hcycle : g.ReadCycle) :
    ∀ x axis, (g.member x).definition.axis? = some axis → kind axis = .coordinate := by
  obtain ⟨u, hu⟩ := h.hasUnfoldMember hcycle
  have hunfold : ∃ b, (g.member u).definition = .unfoldOver b := by
    unfold shape? at hu
    cases hdef : (g.member u).definition with
    | unfoldOver b => exact ⟨b, rfl⟩
    | forOver _ => simp [hdef, Definition.shape?] at hu
    | otherFormula => simp [hdef, Definition.shape?] at hu
    | unfinished => simp [hdef, Definition.shape?] at hu
  obtain ⟨b, hb⟩ := hunfold
  intro x axis hx
  have haxis := h.memberRules.commonAxis x u
  rw [hx, hb] at haxis
  simp only [Definition.axis?, Option.some.injEq] at haxis
  subst haxis
  exact htyped u _ hb

/-- A same-step read is an element dependency at every step after the first. -/
theorem SameReads.elementDep {x y : Fin size} (h : g.SameReads x y) {k : Nat} (hk : 0 < k) :
    g.ElementDep ⟨x, k⟩ ⟨y, k⟩ := by
  obtain ⟨r, hr, hs, ht, ha⟩ := h
  subst hs
  subst ht
  cases hshape : g.shape? r.source with
  | none => simp [access?, hshape] at ha
  | some shape =>
      rw [access?_eq hshape] at ha
      refine .same hr hshape ha ?_
      cases shape with
      | forBody => rfl
      | unfoldBody => simp [Shape.runsBodyAt]; omega

/-- R7 is exact for the read abstraction: a same-step cycle is an element cycle
on every axis with at least two elements. -/
theorem SameStepCycle.elementCycle (h : g.SameStepCycle) {n : Nat} (hn : 2 ≤ n) :
    ∃ e, Relation.TransGen (g.ElementDepWithin n) e e := by
  obtain ⟨x, hx⟩ := h
  exact ⟨⟨x, 1⟩, transGen_map (fun y => (⟨y, 1⟩ : Element size))
    (fun _ _ hab => ⟨by simp only; omega, hab.elementDep Nat.one_pos⟩) hx⟩

/-! ## Diagnostics -/

/-- The checker accepts exactly when no diagnostic is shown. -/
theorem diagnose_eq_none_iff : g.diagnose = none ↔ g.Accepted := by
  unfold diagnose
  split
  · rename_i plan hplan
    exact ⟨fun _ => accepted_of_check_ok hplan, fun _ => rfl⟩
  · rename_i violation hviolation
    refine ⟨fun h => (by cases h), fun h => ?_⟩
    obtain ⟨plan, hplan⟩ := check_ok_of_accepted h
    rw [hplan] at hviolation
    cases hviolation

/-- A recurrence-group diagnostic names the violation the specification selects. -/
theorem diagnose_reported {violation : Violation size}
    (h : g.diagnose = some (.recurrenceGroupRejected violation)) : violation.Reported g := by
  unfold diagnose at h
  split at h
  · cases h
  · rename_i reported hreported
    split at h
    · cases h
      exact check_error_reported hreported
    · cases h

/-- A cycle without an `unfold` member is rejected with the plain cycle
diagnostic. -/
theorem diagnose_dependencyCycle (hno : g.hasUnfoldMember = false) (hcycle : g.ReadCycle) :
    g.diagnose = some .dependencyCycle := by
  unfold diagnose
  split
  · rename_i plan hplan
    obtain ⟨u, hu⟩ := (accepted_of_check_ok hplan).hasUnfoldMember hcycle
    have hany : g.hasUnfoldMember = true := by
      unfold hasUnfoldMember
      rw [List.any_eq_true]
      exact ⟨u, List.mem_finRange u, by simp [hu]⟩
    rw [hany] at hno
    cases hno
  · simp [hno]

end Group

end Graphcal.Graph.RecurrenceGroup
