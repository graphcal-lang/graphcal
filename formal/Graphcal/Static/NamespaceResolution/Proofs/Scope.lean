import Graphcal.Static.NamespaceResolution.Proofs.Lookup

namespace Graphcal.Static.NamespaceResolution

/-- A candidate matches a binding's exact query exactly when their slots agree. -/
theorem matches_exact_query_iff_slot_eq
    (candidate binding : Binding) :
    candidate.Matches
        (Query.exact binding.scope binding.entity.space binding.name) ↔
      candidate.slot = binding.slot := by
  simp [Binding.Matches, Query.exact, Binding.slot]

/-- Exact-query absence is the declarative fresh-slot condition. -/
theorem noMatch_exact_iff_slotAbsent
    (binding : Binding)
    (environment : Environment) :
    NoMatch environment
        (Query.exact binding.scope binding.entity.space binding.name) ↔
      SlotAbsent binding.slot environment := by
  constructor
  · intro noMatch candidate member sameSlot
    exact noMatch candidate member ((matches_exact_query_iff_slot_eq candidate binding).2 sameSlot)
  · intro absent candidate member doesMatch
    exact absent candidate member ((matches_exact_query_iff_slot_eq candidate binding).1 doesMatch)

/-- Executable insertion succeeds exactly for a fresh namespace slot. -/
theorem insertBinding_accepts_iff_slotAbsent
    (binding : Binding)
    (environment : Environment) :
    insertBinding binding environment = .ok (binding :: environment) ↔
      SlotAbsent binding.slot environment := by
  rw [← noMatch_exact_iff_slotAbsent]
  let query := Query.exact binding.scope binding.entity.space binding.name
  cases summary : summarizeLookup environment query with
  | none =>
      have absent : NoMatch environment query :=
        (summarizeLookup_none_iff_noMatch environment query).1 summary
      simp [insertBinding, query, summary, absent]
  | one existing =>
      have occupied : ¬ NoMatch environment query := by
        intro absent
        have empty := (summarizeLookup_none_iff_noMatch environment query).2 absent
        rw [summary] at empty
        contradiction
      simp [insertBinding, query, summary, occupied]
  | many =>
      have occupied : ¬ NoMatch environment query := by
        intro absent
        have empty := (summarizeLookup_none_iff_noMatch environment query).2 absent
        rw [summary] at empty
        contradiction
      simp [insertBinding, query, summary, occupied]

/-- Every successful insertion returns the input environment with one new head. -/
theorem insertBinding_success_result
    {binding : Binding}
    {environment result : Environment}
    (accepted : insertBinding binding environment = .ok result) :
    result = binding :: environment := by
  let query := Query.exact binding.scope binding.entity.space binding.name
  cases summary : summarizeLookup environment query with
  | none =>
      simpa [insertBinding, query, summary] using accepted.symm
  | one existing => simp [insertBinding, query, summary] at accepted
  | many => simp [insertBinding, query, summary] at accepted

/-- Successful scope construction preserves every input binding. -/
theorem buildScope_success_result
    {environment result : Environment}
    (accepted : buildScope environment = .ok result) :
    result = environment := by
  induction environment generalizing result with
  | nil => simpa [buildScope] using accepted.symm
  | cons binding rest ih =>
      cases built : buildScope rest with
      | error failure =>
          rw [buildScope.eq_2, built] at accepted
          contradiction
      | ok builtEnvironment =>
          have builtEq : builtEnvironment = rest := ih built
          subst builtEnvironment
          rw [buildScope.eq_2, built] at accepted
          exact insertBinding_success_result accepted

/-- Executable scope construction accepts exactly the declarative valid scopes. -/
theorem buildScope_accepts_iff_wellFormed (environment : Environment) :
    buildScope environment = .ok environment ↔ ScopeWellFormed environment := by
  induction environment with
  | nil => simp [buildScope, ScopeWellFormed]
  | cons binding rest ih =>
      constructor
      · intro accepted
        cases built : buildScope rest with
        | error failure =>
            rw [buildScope.eq_2, built] at accepted
            contradiction
        | ok builtEnvironment =>
            have builtEq : builtEnvironment = rest := buildScope_success_result built
            subst builtEnvironment
            rw [buildScope.eq_2, built] at accepted
            exact ⟨
              (insertBinding_accepts_iff_slotAbsent binding rest).1 accepted,
              ih.mp built
            ⟩
      · rintro ⟨fresh, restValid⟩
        have built : buildScope rest = .ok rest := ih.mpr restValid
        have inserted : insertBinding binding rest = .ok (binding :: rest) :=
          (insertBinding_accepts_iff_slotAbsent binding rest).2 fresh
        rw [buildScope.eq_2, built]
        exact inserted

/-- Executable binder validation is exactly the approved no-shadowing rule. -/
theorem validateBinder_accepts_iff_fresh
    (environment : Environment)
    (request : BinderRequest) :
    validateBinder environment request = .ok () ↔ BinderFresh environment request := by
  let query : Query := {
    scopes := request.scope :: request.visibleScopes
    space := request.kind.space
    name := request.name
  }
  cases summary : summarizeLookup environment query with
  | none =>
      have fresh : NoMatch environment query :=
        (summarizeLookup_none_iff_noMatch environment query).1 summary
      simpa [validateBinder, BinderFresh, query, summary] using fresh
  | one existing =>
      have occupied : ¬ NoMatch environment query := by
        intro fresh
        have empty := (summarizeLookup_none_iff_noMatch environment query).2 fresh
        rw [summary] at empty
        contradiction
      simp [validateBinder, BinderFresh, query, summary, occupied]
  | many =>
      have occupied : ¬ NoMatch environment query := by
        intro fresh
        have empty := (summarizeLookup_none_iff_noMatch environment query).2 fresh
        rw [summary] at empty
        contradiction
      simp [validateBinder, BinderFresh, query, summary, occupied]

/-- The custom scope predicate is ordinary pairwise slot uniqueness. -/
theorem scopeWellFormed_iff_pairwise (environment : Environment) :
    ScopeWellFormed environment ↔
      environment.Pairwise fun first second => first.slot ≠ second.slot := by
  induction environment with
  | nil => simp [ScopeWellFormed]
  | cons binding rest ih =>
      constructor
      · rintro ⟨absent, restValid⟩
        rw [List.pairwise_cons]
        refine ⟨?_, ih.mp restValid⟩
        intro candidate member sameSlot
        exact absent candidate member sameSlot.symm
      · intro pairwise
        rw [List.pairwise_cons] at pairwise
        refine ⟨?_, ih.mpr pairwise.2⟩
        intro candidate member sameSlot
        exact pairwise.1 candidate member sameSlot.symm

/-- Declarative scope validity is independent of declaration order. -/
theorem scopeWellFormed_perm
    {first second : Environment}
    (permutation : first.Perm second) :
    ScopeWellFormed first ↔ ScopeWellFormed second := by
  rw [scopeWellFormed_iff_pairwise, scopeWellFormed_iff_pairwise]
  exact permutation.pairwise_iff (fun distinct => Ne.symm distinct)

/-- Executable scope acceptance is independent of declaration order. -/
theorem buildScope_acceptance_perm
    {first second : Environment}
    (permutation : first.Perm second) :
    buildScope first = .ok first ↔ buildScope second = .ok second := by
  rw [buildScope_accepts_iff_wellFormed, buildScope_accepts_iff_wellFormed]
  exact scopeWellFormed_perm permutation


end Graphcal.Static.NamespaceResolution
