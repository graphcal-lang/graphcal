import Graphcal.Static.NamespaceResolution.Proofs.Lookup

namespace Graphcal.Static.NamespaceResolution

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


end Graphcal.Static.NamespaceResolution
