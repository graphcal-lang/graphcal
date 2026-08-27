import Graphcal.Static.NamespaceResolution.Reference

import Graphcal.Static.NamespaceResolution.Spec

namespace Graphcal.Static.NamespaceResolution


/-- The executable empty summary is exactly declarative absence. -/
theorem summarizeLookup_none_iff_noMatch
    (environment : Environment)
    (query : Query) :
    summarizeLookup environment query = .none ↔ NoMatch environment query := by
  induction environment with
  | nil => simp [summarizeLookup, NoMatch]
  | cons binding rest ih =>
      by_cases doesMatch : binding.Matches query
      · cases summary : summarizeLookup rest query <;>
          simp [summarizeLookup, LookupSummary.add, doesMatch, NoMatch, summary]
      · simpa [summarizeLookup, LookupSummary.add, doesMatch, NoMatch] using ih

/-- Declarative unique lookup always produces one executable candidate. -/
theorem lookup_spec_implies_summary_one
    {environment : Environment}
    {query : Query}
    {entity : Entity}
    (resolved : Lookup query environment entity) :
    ∃ binding,
      summarizeLookup environment query = .one binding ∧
        binding.entity = entity := by
  induction resolved with
  | head doesMatch restMisses =>
      refine ⟨_, ?_, rfl⟩
      have empty := (summarizeLookup_none_iff_noMatch _ _).2 restMisses
      simp [summarizeLookup, LookupSummary.add, doesMatch, empty]
  | tail misses found ih =>
      obtain ⟨binding, summary, entityEq⟩ := ih
      refine ⟨binding, ?_, entityEq⟩
      simpa [summarizeLookup, LookupSummary.add, misses] using summary

/-- One executable candidate reconstructs the declarative unique lookup. -/
theorem summary_one_implies_lookup_spec
    {environment : Environment}
    {query : Query}
    {binding : Binding}
    (summary : summarizeLookup environment query = .one binding) :
    Lookup query environment binding.entity := by
  induction environment with
  | nil => simp [summarizeLookup] at summary
  | cons candidate rest ih =>
      by_cases doesMatch : candidate.Matches query
      · cases restSummary : summarizeLookup rest query with
        | none =>
            have candidateEq : candidate = binding := by
              simpa [summarizeLookup, LookupSummary.add, doesMatch, restSummary] using summary
            subst candidateEq
            exact .head doesMatch ((summarizeLookup_none_iff_noMatch _ _).1 restSummary)
        | one other =>
            simp [summarizeLookup, LookupSummary.add, doesMatch, restSummary] at summary
        | many =>
            simp [summarizeLookup, LookupSummary.add, doesMatch, restSummary] at summary
      · have restOne : summarizeLookup rest query = .one binding := by
          simpa [summarizeLookup, LookupSummary.add, doesMatch] using summary
        exact .tail doesMatch (ih restOne)

/-- Executable exact lookup accepts exactly the declarative lookup relation. -/
theorem lookup_accepts_iff_spec
    (environment : Environment)
    (query : Query)
    (entity : Entity) :
    lookup environment query = .ok entity ↔ Lookup query environment entity := by
  constructor
  · intro accepted
    cases summary : summarizeLookup environment query with
    | none => simp [lookup, summary] at accepted
    | one binding =>
        have entityEq : binding.entity = entity := by
          simpa [lookup, summary] using accepted
        rw [← entityEq]
        exact summary_one_implies_lookup_spec summary
    | many => simp [lookup, summary] at accepted
  · intro resolved
    obtain ⟨binding, summary, entityEq⟩ := lookup_spec_implies_summary_one resolved
    simp [lookup, summary, entityEq]

/-- Declarative lookup has at most one result, even for invalid environments. -/
theorem lookup_deterministic
    {environment : Environment}
    {query : Query}
    {first second : Entity}
    (firstResolved : Lookup query environment first)
    (secondResolved : Lookup query environment second) :
    first = second := by
  have firstAccepted := (lookup_accepts_iff_spec environment query first).2 firstResolved
  have secondAccepted := (lookup_accepts_iff_spec environment query second).2 secondResolved
  exact Except.ok.inj (firstAccepted.symm.trans secondAccepted)

/-- Adding two bindings to a lookup summary commutes. -/
theorem LookupSummary.add_comm
    (summary : LookupSummary)
    (query : Query)
    (first second : Binding) :
    (summary.add query second).add query first =
      (summary.add query first).add query second := by
  by_cases firstMatches : first.Matches query <;>
    by_cases secondMatches : second.Matches query <;>
    cases summary <;>
    simp [LookupSummary.add, firstMatches, secondMatches]

/-- Lookup cardinality and identity are independent of binding order. -/
theorem summarizeLookup_perm
    {first second : Environment}
    (permutation : first.Perm second)
    (query : Query) :
    summarizeLookup first query = summarizeLookup second query := by
  induction permutation with
  | nil => rfl
  | cons binding _ ih =>
      simp [summarizeLookup, ih]
  | swap first second rest =>
      simp [summarizeLookup, LookupSummary.add_comm]
  | trans _ _ ih₁ ih₂ =>
      exact ih₁.trans ih₂

/-- The executable lookup decision is independent of declaration order. -/
theorem lookup_perm
    {first second : Environment}
    (permutation : first.Perm second)
    (query : Query) :
    lookup first query = lookup second query := by
  simp [lookup, summarizeLookup_perm permutation query]

/-- A binding from an unselected namespace cannot affect one lookup query. -/
theorem lookup_ignores_other_namespace
    (environment : Environment)
    (query : Query)
    (binding : Binding)
    (different : binding.entity.space ≠ query.space) :
    lookup (binding :: environment) query = lookup environment query := by
  have misses : ¬ binding.Matches query := by
    intro doesMatch
    exact different doesMatch.2.1
  simp [lookup, summarizeLookup, LookupSummary.add, misses]

/-- A dependency extension confined to other namespaces cannot retarget a query. -/
theorem lookup_ignores_other_namespaces
    (environment extension : Environment)
    (query : Query)
    (different :
      ∀ binding, binding ∈ extension → binding.entity.space ≠ query.space) :
    lookup (extension ++ environment) query = lookup environment query := by
  induction extension with
  | nil => rfl
  | cons binding rest ih =>
      rw [List.cons_append, lookup_ignores_other_namespace]
      · exact ih fun candidate member => different candidate (by simp [member])
      · exact different binding (by simp)

end Graphcal.Static.NamespaceResolution
