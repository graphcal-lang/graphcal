import Graphcal.Graph.RecurrenceGroup.Proofs.Lists

/-!
# Recurrence groups: filling a hole keeps a group accepted

A `todo<T> { … }` hole lists every possible direct dependency of its missing
part, and a formula that fills it may read only those. Filling a hole therefore
only removes reads, and removing reads never turns an accepted group into a
rejected one.
-/

namespace Graphcal.Graph.RecurrenceGroup

namespace Group

variable {size : Nat} {g h : Group size}

/-- Removing reads keeps a group accepted: its members are unchanged, and every
remaining read was already classified, so R1 and R3–R7 still hold. -/
theorem Accepted.of_reads_subset (hg : g.Accepted) (hmember : h.member = g.member)
    (hreads : ∀ r ∈ h.reads, r ∈ g.reads) : h.Accepted := by
  have hshape : ∀ x, h.shape? x = g.shape? x := fun x => by simp [shape?, hmember]
  have haccess : ∀ r, h.access? r = g.access? r := fun r => by simp [access?, hshape]
  obtain ⟨⟨howned, hshaped, haxis⟩, hself, hkeyed, hcycle⟩ := hg
  refine ⟨⟨fun x y => ?_, fun x => ?_, fun x y => ?_⟩, fun r hr => hself r (hreads r hr),
    fun r hr => ?_, ?_⟩
  · rw [hmember]
    exact howned x y
  · rw [hshape]
    exact hshaped x
  · rw [hmember]
    exact haxis x y
  · rw [haccess]
    exact hkeyed r (hreads r hr)
  · rintro ⟨x, hx⟩
    exact hcycle ⟨x, transGen_mono (fun _ _ ⟨r, hr, hs, ht, ha⟩ =>
      ⟨r, hreads r hr, hs, ht, by rwa [← haccess]⟩) hx⟩

end Group

end Graphcal.Graph.RecurrenceGroup
