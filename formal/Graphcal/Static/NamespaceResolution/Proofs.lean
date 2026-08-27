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

/-- Executable namespace-owner extraction agrees with the declarative relation. -/
theorem namespaceTarget_ok_iff_owns
    (entity : Entity)
    (target : NamespaceTarget) :
    namespaceTarget entity = .ok target ↔ OwnsNamespace entity target := by
  constructor
  · intro accepted
    cases entity with
    | static entity => simp [namespaceTarget] at accepted
    | term entity =>
        cases entity with
        | mk id kind =>
            cases kind <;> simp [namespaceTarget] at accepted <;>
              cases accepted <;> constructor
    | unit entity => simp [namespaceTarget] at accepted
  · intro owns
    cases owns <;> rfl

/-- Executable child-DAG extraction agrees with the declarative relation. -/
theorem dagTarget_ok_iff_owns
    (entity : Entity)
    (target : DagId) :
    dagTarget entity = .ok target ↔ OwnsDag entity target := by
  constructor
  · intro accepted
    cases entity with
    | static entity => simp [dagTarget] at accepted
    | term entity =>
        cases entity with
        | mk id kind =>
            cases kind <;> simp_all [dagTarget] <;> constructor
    | unit entity => simp [dagTarget] at accepted
  · intro owns
    cases owns <;> rfl

/-- Every declarative dotted-DAG traversal is accepted by the executable one. -/
theorem resolveNamespaceSteps_complete
    {environment : Environment}
    {start finalTarget : NamespaceTarget}
    {children : List NameAtom}
    (resolved : NamespaceSteps environment start children finalTarget) :
    resolveNamespaceSteps environment start children = .ok finalTarget := by
  induction resolved with
  | done target => rfl
  | child member ownsDag remaining ih =>
      have memberAccepted := (lookup_accepts_iff_spec _ _ _).2 member
      have dagAccepted := (dagTarget_ok_iff_owns _ _).2 ownsDag
      rw [resolveNamespaceSteps.eq_2, memberAccepted]
      simp only [Bind.bind, Except.bind]
      rw [dagAccepted]
      exact ih

/-- Every successful executable dotted-DAG traversal satisfies the specification. -/
theorem resolveNamespaceSteps_sound
    {environment : Environment}
    {start finalTarget : NamespaceTarget}
    {children : List NameAtom}
    (accepted : resolveNamespaceSteps environment start children = .ok finalTarget) :
    NamespaceSteps environment start children finalTarget := by
  induction children generalizing start with
  | nil =>
      have targetEq : start = finalTarget := by
        simpa [resolveNamespaceSteps] using accepted
      subst finalTarget
      exact .done start
  | cons child rest ih =>
      cases start with
      | dag current =>
          rw [resolveNamespaceSteps.eq_2] at accepted
          cases memberResult : lookup environment
              (Query.exact (.dag current) .term child) with
          | error failure =>
              rw [memberResult] at accepted
              simp only [Bind.bind, Except.bind] at accepted
              contradiction
          | ok entity =>
              rw [memberResult] at accepted
              simp only [Bind.bind, Except.bind] at accepted
              cases dagResult : dagTarget entity with
              | error failure =>
                  rw [dagResult] at accepted
                  contradiction
              | ok next =>
                  rw [dagResult] at accepted
                  exact .child
                    ((lookup_accepts_iff_spec _ _ _).1 memberResult)
                    ((dagTarget_ok_iff_owns _ _).1 dagResult)
                    (ih accepted)
      | «instance» instanceId =>
          simp [resolveNamespaceSteps] at accepted
      | plugin pluginId =>
          simp [resolveNamespaceSteps] at accepted

/-- Every declarative namespace path is accepted by the executable resolver. -/
theorem resolveNamespacePath_complete
    {environment : Environment}
    {origin : DagId}
    {path : NamespacePath}
    {target : NamespaceTarget}
    (resolved : NamespacePathResolves environment origin path target) :
    resolveNamespacePath environment origin path = .ok target := by
  obtain ⟨rootEntity, firstTarget, rootResolved, ownsNamespace, steps⟩ := resolved
  have rootAccepted := (lookup_accepts_iff_spec _ _ _).2 rootResolved
  have ownerAccepted := (namespaceTarget_ok_iff_owns _ _).2 ownsNamespace
  have stepsAccepted := resolveNamespaceSteps_complete steps
  unfold resolveNamespacePath
  rw [rootAccepted]
  simp only [Bind.bind, Except.bind]
  rw [ownerAccepted]
  exact stepsAccepted

/-- Every successful executable namespace path satisfies the specification. -/
theorem resolveNamespacePath_sound
    {environment : Environment}
    {origin : DagId}
    {path : NamespacePath}
    {target : NamespaceTarget}
    (accepted : resolveNamespacePath environment origin path = .ok target) :
    NamespacePathResolves environment origin path target := by
  unfold resolveNamespacePath at accepted
  cases rootResult : lookup environment
      (Query.exact (.dag origin) .term path.root) with
  | error failure =>
      rw [rootResult] at accepted
      simp only [Bind.bind, Except.bind] at accepted
      contradiction
  | ok rootEntity =>
      rw [rootResult] at accepted
      simp only [Bind.bind, Except.bind] at accepted
      cases ownerResult : namespaceTarget rootEntity with
      | error failure =>
          rw [ownerResult] at accepted
          contradiction
      | ok firstTarget =>
          rw [ownerResult] at accepted
          exact ⟨
            rootEntity,
            firstTarget,
            (lookup_accepts_iff_spec _ _ _).1 rootResult,
            (namespaceTarget_ok_iff_owns _ _).1 ownerResult,
            resolveNamespaceSteps_sound accepted
          ⟩

/-- Every declaratively resolved name head is accepted by the executable resolver. -/
theorem resolveHead_complete
    {environment : Environment}
    {space : Namespace}
    {head : NameHead}
    {entity : Entity}
    (resolved : HeadResolves environment space head entity) :
    resolveHead environment space head = .ok entity := by
  cases head with
  | visible scopes name =>
      exact (lookup_accepts_iff_spec _ _ _).2 resolved
  | member origin owner name =>
      obtain ⟨target, pathResolved, memberResolved⟩ := resolved
      have pathAccepted := resolveNamespacePath_complete pathResolved
      have memberAccepted := (lookup_accepts_iff_spec _ _ _).2 memberResolved
      rw [resolveHead.eq_2, pathAccepted]
      simp only [Bind.bind, Except.bind]
      exact memberAccepted

/-- Every executable name-head success satisfies the declarative resolver. -/
theorem resolveHead_sound
    {environment : Environment}
    {space : Namespace}
    {head : NameHead}
    {entity : Entity}
    (accepted : resolveHead environment space head = .ok entity) :
    HeadResolves environment space head entity := by
  cases head with
  | visible scopes name =>
      exact (lookup_accepts_iff_spec _ _ _).1 accepted
  | member origin owner name =>
      rw [resolveHead.eq_2] at accepted
      cases pathResult : resolveNamespacePath environment origin owner with
      | error failure =>
          rw [pathResult] at accepted
          simp only [Bind.bind, Except.bind] at accepted
          contradiction
      | ok target =>
          rw [pathResult] at accepted
          simp only [Bind.bind, Except.bind] at accepted
          exact ⟨
            target,
            resolveNamespacePath_sound pathResult,
            (lookup_accepts_iff_spec _ _ _).1 accepted
          ⟩

/-- Executable Static validation agrees with the normative capability table. -/
theorem validateStatic_ok_iff_permitted
    (use : StaticUse)
    (entity : StaticEntity) :
    validateStatic use (.static entity) = .ok entity ↔ StaticPermits use entity := by
  cases use <;> cases entity with
  | mk id kind => cases kind <;> simp [validateStatic, StaticPermits]

/-- A successful Static validation preserves the looked-up entity and proves permission. -/
theorem validateStatic_success_iff
    (use : StaticUse)
    (candidate : Entity)
    (result : StaticEntity) :
    validateStatic use candidate = .ok result ↔
      candidate = .static result ∧ StaticPermits use result := by
  cases candidate with
  | static entity =>
      constructor
      · intro accepted
        have resultEq : result = entity := by
          cases use <;> cases entity with
          | mk id kind =>
              cases kind <;> simp [validateStatic] at accepted <;> simp_all
        subst result
        exact ⟨rfl, (validateStatic_ok_iff_permitted use entity).1 accepted⟩
      · rintro ⟨candidateEq, permitted⟩
        have entityEq : entity = result := Entity.static.inj candidateEq
        subst result
        exact (validateStatic_ok_iff_permitted use entity).2 permitted
  | term entity => simp [validateStatic]
  | unit entity => simp [validateStatic]

/-- Executable Term validation agrees with the normative capability table. -/
theorem validateTerm_ok_iff_permitted
    (use : TermUse)
    (entity : TermEntity) :
    validateTerm use (.term entity) = .ok entity ↔ TermPermits use entity := by
  cases use <;> cases entity with
  | mk id kind => cases kind <;> simp [validateTerm, TermPermits]

/-- A successful Term validation preserves the looked-up entity and proves permission. -/
theorem validateTerm_success_iff
    (use : TermUse)
    (candidate : Entity)
    (result : TermEntity) :
    validateTerm use candidate = .ok result ↔
      candidate = .term result ∧ TermPermits use result := by
  cases candidate with
  | static entity => simp [validateTerm]
  | term entity =>
      constructor
      · intro accepted
        have resultEq : result = entity := by
          cases use <;> cases entity with
          | mk id kind =>
              cases kind <;> simp [validateTerm] at accepted <;> simp_all
        subst result
        exact ⟨rfl, (validateTerm_ok_iff_permitted use entity).1 accepted⟩
      · rintro ⟨candidateEq, permitted⟩
        have entityEq : entity = result := Entity.term.inj candidateEq
        subst result
        exact (validateTerm_ok_iff_permitted use entity).2 permitted
  | unit entity => simp [validateTerm]

/-- Concrete-index validation agrees with the label-owner requirement. -/
theorem requireConcreteIndex_ok_iff
    (entity : StaticEntity) :
    requireConcreteIndex entity = .ok entity ↔ entity.kind = .index := by
  cases entity with
  | mk id kind => cases kind <;> simp [requireConcreteIndex]

/-- A successful concrete-index check preserves the candidate and proves its kind. -/
theorem requireConcreteIndex_success_iff
    (candidate result : StaticEntity) :
    requireConcreteIndex candidate = .ok result ↔
      result = candidate ∧ candidate.kind = .index := by
  constructor
  · intro accepted
    cases candidate with
    | mk id kind =>
        cases kind <;> simp [requireConcreteIndex] at accepted
        exact ⟨accepted.symm, rfl⟩
  · rintro ⟨rfl, isIndex⟩
    exact (requireConcreteIndex_ok_iff result).2 isIndex

/-- Executable label validation preserves exactly the expected canonical owner. -/
theorem validateIndexLabel_success_iff
    (expectedOwner : StaticId)
    (candidate : Entity)
    (result : TermEntity) :
    validateIndexLabel expectedOwner candidate = .ok result ↔
      candidate = .term result ∧ result.kind = .indexLabel expectedOwner := by
  constructor
  · intro accepted
    cases candidate with
    | static entity => simp [validateIndexLabel] at accepted
    | unit entity => simp [validateIndexLabel] at accepted
    | term entity =>
        cases entity with
        | mk id kind =>
            cases kind <;> try { simp [validateIndexLabel] at accepted }
            case indexLabel actualOwner =>
              by_cases sameOwner : actualOwner = expectedOwner
              · subst actualOwner
                have resultEq : result = { id := id, kind := .indexLabel expectedOwner } := by
                  simpa [validateIndexLabel] using accepted.symm
                subst result
                exact ⟨rfl, rfl⟩
              · simp [validateIndexLabel, sameOwner] at accepted
  · rintro ⟨rfl, ownerMatches⟩
    cases result with
    | mk id kind =>
        cases ownerMatches
        simp [validateIndexLabel]

/-- The declarative core of `Owner#Label` resolution. -/
def LabelResolves
    (environment : Environment)
    (owner : NameHead)
    (label : NameAtom)
    (result : TermEntity) : Prop :=
  ∃ indexEntity,
    HeadResolves environment .static owner (.static indexEntity) ∧
      indexEntity.kind = .index ∧
      Lookup
        (Query.exact (.indexLabels indexEntity.id) .term label)
        environment
        (.term result) ∧
      result.kind = .indexLabel indexEntity.id

/-- Every declaratively resolved index label is accepted by the executable resolver. -/
theorem resolveLabel_complete
    {environment : Environment}
    {owner : NameHead}
    {label : NameAtom}
    {result : TermEntity}
    (resolved : LabelResolves environment owner label result) :
    resolveLabel environment owner label = .ok result := by
  obtain ⟨indexEntity, ownerResolved, ownerIsIndex, labelResolved, ownerMatches⟩ := resolved
  have ownerAccepted := resolveHead_complete ownerResolved
  have indexPermitted : StaticPermits .index indexEntity := by
    cases indexEntity with
    | mk id kind => cases kind <;> simp_all [StaticPermits]
  have indexAccepted := (validateStatic_ok_iff_permitted .index indexEntity).2 indexPermitted
  have concreteAccepted := (requireConcreteIndex_ok_iff indexEntity).2 ownerIsIndex
  have labelAccepted := (lookup_accepts_iff_spec _ _ _).2 labelResolved
  rw [resolveLabel.eq_1, ownerAccepted]
  simp only [Bind.bind, Except.bind]
  rw [indexAccepted]
  simp only
  rw [concreteAccepted]
  simp only
  rw [labelAccepted]
  cases result with
  | mk id kind =>
      cases ownerMatches
      simp [validateIndexLabel]

/-- Every successful executable index-label resolution satisfies the specification. -/
theorem resolveLabel_sound
    {environment : Environment}
    {owner : NameHead}
    {label : NameAtom}
    {result : TermEntity}
    (accepted : resolveLabel environment owner label = .ok result) :
    LabelResolves environment owner label result := by
  rw [resolveLabel.eq_1] at accepted
  cases ownerResult : resolveHead environment .static owner with
  | error failure =>
      rw [ownerResult] at accepted
      simp only [Bind.bind, Except.bind] at accepted
      contradiction
  | ok ownerCandidate =>
      rw [ownerResult] at accepted
      simp only [Bind.bind, Except.bind] at accepted
      cases indexResult : validateStatic .index ownerCandidate with
      | error failure =>
          rw [indexResult] at accepted
          simp only at accepted
          contradiction
      | ok indexEntity =>
          rw [indexResult] at accepted
          simp only at accepted
          obtain ⟨ownerCandidateEq, indexPermitted⟩ :=
            (validateStatic_success_iff .index ownerCandidate indexEntity).1 indexResult
          cases concreteResult : requireConcreteIndex indexEntity with
          | error failure =>
              rw [concreteResult] at accepted
              simp only at accepted
              contradiction
          | ok concreteEntity =>
              rw [concreteResult] at accepted
              simp only at accepted
              obtain ⟨concreteEq, ownerIsIndex⟩ :=
                (requireConcreteIndex_success_iff indexEntity concreteEntity).1 concreteResult
              subst concreteEntity
              cases labelResult : lookup environment
                  (Query.exact (.indexLabels indexEntity.id) .term label) with
              | error failure =>
                  rw [labelResult] at accepted
                  contradiction
              | ok labelCandidate =>
                  rw [labelResult] at accepted
                  obtain ⟨labelCandidateEq, ownerMatches⟩ :=
                    (validateIndexLabel_success_iff indexEntity.id labelCandidate result).1 accepted
                  have resolvedOwner := resolveHead_sound ownerResult
                  rw [ownerCandidateEq] at resolvedOwner
                  have resolvedLabel := (lookup_accepts_iff_spec _ _ _).1 labelResult
                  rw [labelCandidateEq] at resolvedLabel
                  exact ⟨
                    indexEntity,
                    resolvedOwner,
                    ownerIsIndex,
                    resolvedLabel,
                    ownerMatches
                  ⟩

/-- Every declaratively resolved reference is accepted by the executable resolver. -/
theorem resolve_complete
    {environment : Environment}
    {reference : Reference}
    {entity : Entity}
    (resolved : Resolves environment reference entity) :
    resolve environment reference = .ok entity := by
  cases resolved with
  | static headResolved permitted =>
      have headAccepted := resolveHead_complete headResolved
      have validationAccepted := (validateStatic_ok_iff_permitted _ _).2 permitted
      rw [resolve.eq_1, headAccepted]
      simp only [Bind.bind, Except.bind]
      rw [validationAccepted]
      rfl
  | term headResolved permitted =>
      have headAccepted := resolveHead_complete headResolved
      have validationAccepted := (validateTerm_ok_iff_permitted _ _).2 permitted
      rw [resolve.eq_2, headAccepted]
      simp only [Bind.bind, Except.bind]
      rw [validationAccepted]
      rfl
  | unit headResolved =>
      have headAccepted := resolveHead_complete headResolved
      rw [resolve.eq_3, headAccepted]
      rfl
  | label ownerResolved ownerIsIndex labelResolved ownerMatches =>
      have labelAccepted := resolveLabel_complete ⟨
        _, ownerResolved, ownerIsIndex, labelResolved, ownerMatches
      ⟩
      rw [resolve.eq_4, labelAccepted]
      rfl
  | call headResolved permitted =>
      have headAccepted := resolveHead_complete headResolved
      have validationAccepted := (validateTerm_ok_iff_permitted .call _).2 permitted
      rw [resolve.eq_5, headAccepted]
      simp only [Bind.bind, Except.bind]
      rw [validationAccepted]
      rfl

/-- Unit validation preserves exactly a Unit candidate. -/
theorem validateUnit_success_iff
    (candidate : Entity)
    (result : UnitEntity) :
    validateUnit candidate = .ok result ↔ candidate = .unit result := by
  cases candidate <;> simp [validateUnit]

/-- Every successful executable reference satisfies the normative relation. -/
theorem resolve_sound
    {environment : Environment}
    {reference : Reference}
    {entity : Entity}
    (accepted : resolve environment reference = .ok entity) :
    Resolves environment reference entity := by
  cases reference with
  | static head use =>
      rw [resolve.eq_1] at accepted
      cases headResult : resolveHead environment .static head with
      | error failure =>
          rw [headResult] at accepted
          contradiction
      | ok candidate =>
          rw [headResult] at accepted
          simp only [Bind.bind, Except.bind] at accepted
          cases validationResult : validateStatic use candidate with
          | error failure =>
              rw [validationResult] at accepted
              simp only at accepted
              contradiction
          | ok result =>
              rw [validationResult] at accepted
              simp only at accepted
              have outputEq : Entity.static result = entity := Except.ok.inj accepted
              subst entity
              obtain ⟨candidateEq, permitted⟩ :=
                (validateStatic_success_iff use candidate result).1 validationResult
              have headResolved := resolveHead_sound headResult
              rw [candidateEq] at headResolved
              exact .static headResolved permitted
  | term head use =>
      rw [resolve.eq_2] at accepted
      cases headResult : resolveHead environment .term head with
      | error failure =>
          rw [headResult] at accepted
          contradiction
      | ok candidate =>
          rw [headResult] at accepted
          simp only [Bind.bind, Except.bind] at accepted
          cases validationResult : validateTerm use candidate with
          | error failure =>
              rw [validationResult] at accepted
              simp only at accepted
              contradiction
          | ok result =>
              rw [validationResult] at accepted
              simp only at accepted
              have outputEq : Entity.term result = entity := Except.ok.inj accepted
              subst entity
              obtain ⟨candidateEq, permitted⟩ :=
                (validateTerm_success_iff use candidate result).1 validationResult
              have headResolved := resolveHead_sound headResult
              rw [candidateEq] at headResolved
              exact .term headResolved permitted
  | unit head =>
      rw [resolve.eq_3] at accepted
      cases headResult : resolveHead environment .unit head with
      | error failure =>
          rw [headResult] at accepted
          contradiction
      | ok candidate =>
          rw [headResult] at accepted
          simp only [Bind.bind, Except.bind] at accepted
          cases validationResult : validateUnit candidate with
          | error failure =>
              rw [validationResult] at accepted
              simp only at accepted
              contradiction
          | ok result =>
              rw [validationResult] at accepted
              simp only at accepted
              have outputEq : Entity.unit result = entity := Except.ok.inj accepted
              subst entity
              have candidateEq := (validateUnit_success_iff candidate result).1 validationResult
              have headResolved := resolveHead_sound headResult
              rw [candidateEq] at headResolved
              exact .unit headResolved
  | label owner label =>
      rw [resolve.eq_4] at accepted
      cases labelResult : resolveLabel environment owner label with
      | error failure =>
          rw [labelResult] at accepted
          contradiction
      | ok result =>
          rw [labelResult] at accepted
          have outputEq : Entity.term result = entity := Except.ok.inj accepted
          subst entity
          obtain ⟨indexEntity, ownerResolved, ownerIsIndex, labelResolved, ownerMatches⟩ :=
            resolveLabel_sound labelResult
          exact .label ownerResolved ownerIsIndex labelResolved ownerMatches
  | call callee arguments =>
      rw [resolve.eq_5] at accepted
      cases headResult : resolveHead environment .term callee with
      | error failure =>
          rw [headResult] at accepted
          contradiction
      | ok candidate =>
          rw [headResult] at accepted
          simp only [Bind.bind, Except.bind] at accepted
          cases validationResult : validateTerm .call candidate with
          | error failure =>
              rw [validationResult] at accepted
              simp only at accepted
              contradiction
          | ok result =>
              rw [validationResult] at accepted
              simp only at accepted
              have outputEq : Entity.term result = entity := Except.ok.inj accepted
              subst entity
              obtain ⟨candidateEq, permitted⟩ :=
                (validateTerm_success_iff .call candidate result).1 validationResult
              have headResolved := resolveHead_sound headResult
              rw [candidateEq] at headResolved
              exact .call headResolved permitted

/-- Acceptance is exactly normative resolution, not merely one-way soundness. -/
theorem resolve_accepts_iff_resolves
    (environment : Environment)
    (reference : Reference)
    (entity : Entity) :
    resolve environment reference = .ok entity ↔ Resolves environment reference entity :=
  ⟨resolve_sound, resolve_complete⟩

/-- Normative reference resolution is deterministic. -/
theorem resolves_deterministic
    {environment : Environment}
    {reference : Reference}
    {first second : Entity}
    (firstResolved : Resolves environment reference first)
    (secondResolved : Resolves environment reference second) :
    first = second := by
  have firstAccepted := resolve_complete firstResolved
  have secondAccepted := resolve_complete secondResolved
  exact Except.ok.inj (firstAccepted.symm.trans secondAccepted)

/-- Every successful result inhabits the namespace selected by source syntax. -/
theorem resolves_result_space
    {environment : Environment}
    {reference : Reference}
    {entity : Entity}
    (resolved : Resolves environment reference entity) :
    entity.space = reference.resultSpace := by
  cases resolved <;> rfl

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

/-- Static validation rejects a Term without probing another scope or namespace. -/
theorem static_use_rejects_term
    (use : StaticUse)
    (entity : TermEntity) :
    validateStatic use (.term entity) = .error (.wrongNamespace .static (.term entity)) := by
  rfl

/-- Term validation rejects a Static entity without probing another scope. -/
theorem term_use_rejects_static
    (use : TermUse)
    (entity : StaticEntity) :
    validateTerm use (.static entity) = .error (.wrongNamespace .term (.static entity)) := by
  rfl

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

/-- Paths reaching the same canonical target resolve the same final member. -/
theorem same_namespace_target_preserves_member_identity
    (environment : Environment)
    (space : Namespace)
    (origin : DagId)
    (first second : NamespacePath)
    (member : NameAtom)
    (target : NamespaceTarget)
    (firstTarget : resolveNamespacePath environment origin first = .ok target)
    (secondTarget : resolveNamespacePath environment origin second = .ok target) :
    resolveHead environment space (.member origin first member) =
      resolveHead environment space (.member origin second member) := by
  rw [resolveHead.eq_2, resolveHead.eq_2, firstTarget, secondTarget]

/-- Call argument shape cannot affect callee resolution. -/
theorem call_argument_shape_irrelevant
    (environment : Environment)
    (callee : NameHead)
    (first second : ArgumentShape) :
    resolve environment (.call callee first) = resolve environment (.call callee second) := by
  rfl

end Graphcal.Static.NamespaceResolution
