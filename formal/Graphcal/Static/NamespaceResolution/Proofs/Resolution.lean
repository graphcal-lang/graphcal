import Graphcal.Static.NamespaceResolution.Proofs.InputBinding

namespace Graphcal.Static.NamespaceResolution

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
              cases kind <;>
                simp [validateTerm, IncludeProjectable, includeProjection?] at accepted <;>
                simp_all
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
    requireConcreteIndex entity = .ok entity ↔
      entity.concreteInputKind = some .index := by
  cases entity with
  | mk id kind =>
      cases kind with
      | nominalType role | dimension role | index role =>
          cases role <;> simp [requireConcreteIndex, StaticEntity.concreteInputKind,
            StaticEntity.inputRole]
      | genericTypeParam | genericDimParam | genericIndexParam
      | genericNatParam | timeScale =>
          simp [requireConcreteIndex, StaticEntity.concreteInputKind,
            StaticEntity.inputRole]

/-- A successful concrete-index check preserves the candidate and proves its kind. -/
theorem requireConcreteIndex_success_iff
    (candidate result : StaticEntity) :
    requireConcreteIndex candidate = .ok result ↔
      result = candidate ∧ candidate.concreteInputKind = some .index := by
  constructor
  · intro accepted
    cases candidate with
    | mk id kind =>
        cases kind <;> simp [requireConcreteIndex] at accepted
        case index role =>
          cases role <;> simp at accepted
          all_goals exact ⟨accepted.symm, rfl⟩
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
      indexEntity.concreteInputKind = some .index ∧
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
    | mk id kind =>
        cases kind with
        | nominalType role | dimension role | index role =>
            cases role <;> simp_all [StaticPermits, StaticEntity.concreteInputKind,
              StaticEntity.inputRole]
        | genericTypeParam | genericDimParam | genericIndexParam
        | genericNatParam | timeScale =>
            simp_all [StaticEntity.concreteInputKind, StaticEntity.inputRole]
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

/-- Call argument shape cannot affect callee resolution. -/
theorem call_argument_shape_irrelevant
    (environment : Environment)
    (callee : NameHead)
    (first second : ArgumentShape) :
    resolve environment (.call callee first) = resolve environment (.call callee second) := by
  rfl

end Graphcal.Static.NamespaceResolution
