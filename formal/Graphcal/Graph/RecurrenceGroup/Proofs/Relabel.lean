import Graphcal.Graph.RecurrenceGroup.Proofs.Lists

/-!
# Recurrence groups: declaration order does not matter

Renumbering the members and reordering the reads of a group does not change
whether it is accepted. Declaration order only selects which violation a
rejection reports and which valid order the reference evaluator follows.
-/

namespace Graphcal.Graph.RecurrenceGroup

namespace Group

variable {size : Nat} {g h : Group size}

/-- The inverse renumbering. -/
def Relabeling.symm (ρ : Relabeling g h) : Relabeling h g where
  f := ρ.inv
  inv := ρ.f
  inv_f := ρ.f_inv
  f_inv := ρ.inv_f
  member x := by rw [← ρ.member (ρ.inv x), ρ.f_inv]
  reads r := by
    constructor
    · intro hr
      refine ⟨⟨ρ.f r.source, ρ.f r.target, r.form⟩, (ρ.reads _).2 ⟨r, hr, rfl⟩, ?_⟩
      simp [ρ.inv_f]
    · rintro ⟨r', hr', rfl⟩
      obtain ⟨r'', hr'', rfl⟩ := (ρ.reads r').1 hr'
      simpa [ρ.inv_f] using hr''

theorem Relabeling.shape? (ρ : Relabeling g h) (x : Fin size) :
    h.shape? (ρ.f x) = g.shape? x := by
  simp [Group.shape?, ρ.member]

theorem Relabeling.sameReads (ρ : Relabeling g h) {a b : Fin size} (hab : h.SameReads a b) :
    g.SameReads (ρ.inv a) (ρ.inv b) := by
  obtain ⟨r, hr, rfl, rfl, ha⟩ := hab
  obtain ⟨r', hr', rfl⟩ := (ρ.reads r).1 hr
  refine ⟨r', hr', by simp [ρ.inv_f], by simp [ρ.inv_f], ?_⟩
  simpa [access?, ρ.shape?] using ha

/-- A renumbered group is accepted exactly when the original is. -/
theorem Relabeling.accepted (ρ : Relabeling g h) (hg : g.Accepted) : h.Accepted := by
  have hmember : ∀ y, h.member y = g.member (ρ.inv y) := fun y => by
    rw [← ρ.member, ρ.f_inv]
  obtain ⟨⟨howned, hshaped, haxis⟩, hself, hkeyed, hcycle⟩ := hg
  refine ⟨⟨fun x y => ?_, fun y => ?_, fun x y => ?_⟩, ?_, ?_, ?_⟩
  · rw [hmember, hmember]
    exact howned _ _
  · simpa [Group.shape?, hmember] using hshaped (ρ.inv y)
  · rw [hmember, hmember]
    exact haxis _ _
  · intro r hr hst
    obtain ⟨r', hr', rfl⟩ := (ρ.reads r).1 hr
    apply hself r' hr'
    simpa [ρ.inv_f] using congrArg ρ.inv hst
  · intro r hr
    obtain ⟨r', hr', rfl⟩ := (ρ.reads r).1 hr
    simpa [access?, ρ.shape?] using hkeyed r' hr'
  · rintro ⟨x, hx⟩
    exact hcycle ⟨ρ.inv x, transGen_map ρ.inv (fun _ _ hab => ρ.sameReads hab) hx⟩

/-- Acceptance does not depend on declaration order. -/
theorem Relabeling.accepted_iff (ρ : Relabeling g h) : g.Accepted ↔ h.Accepted :=
  ⟨ρ.accepted, ρ.symm.accepted⟩

end Group

end Graphcal.Graph.RecurrenceGroup
