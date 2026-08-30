import Graphcal.Static.ExternalSurface.Reference
import Graphcal.Static.ExternalSurface.Spec

namespace Graphcal.Static.ExternalSurface

open Graphcal.Static.NamespaceResolution

/-- Executable category validation accepts exactly normative import categories. -/
theorem validateBlueprintImportCategory_accepts_iff
    (entity : Entity) :
    validateBlueprintImportCategory entity = .ok () ↔
      BlueprintEntityImportable entity := by
  cases entity with
  | static staticEntity =>
      cases staticEntity with
      | mk id kind =>
          cases kind with
          | nominalType requirement => cases requirement <;> simp [validateBlueprintImportCategory, BlueprintEntityImportable]
          | dimension requirement => cases requirement <;> simp [validateBlueprintImportCategory, BlueprintEntityImportable]
          | index requirement => cases requirement <;> simp [validateBlueprintImportCategory, BlueprintEntityImportable]
          | genericTypeParam => simp [validateBlueprintImportCategory, BlueprintEntityImportable]
          | genericDimParam => simp [validateBlueprintImportCategory, BlueprintEntityImportable]
          | genericIndexParam => simp [validateBlueprintImportCategory, BlueprintEntityImportable]
          | genericNatParam => simp [validateBlueprintImportCategory, BlueprintEntityImportable]
          | timeScale => simp [validateBlueprintImportCategory, BlueprintEntityImportable]
  | term termEntity =>
      cases termEntity with
      | mk id kind => cases kind <;> simp [validateBlueprintImportCategory, BlueprintEntityImportable]
  | unit unitEntity =>
      cases unitEntity with
      | mk id kind => cases kind <;> simp [validateBlueprintImportCategory, BlueprintEntityImportable]

/-- Executable dependency closure validation is exactly declarative closure. -/
theorem validateStaticDependenciesClosed_accepts_iff
    (declaration : Entity)
    (dependencies : List StaticEntity) :
    validateStaticDependenciesClosed declaration dependencies = .ok () ↔
      ∀ dependency, dependency ∈ dependencies → dependency.requiredInputKind = none := by
  induction dependencies with
  | nil => simp [validateStaticDependenciesClosed]
  | cons dependency rest ih =>
      cases required : dependency.requiredInputKind with
      | none => simp [validateStaticDependenciesClosed, required, ih]
      | some kind => simp [validateStaticDependenciesClosed, required]

/-- Direct-import acceptance is exactly the normative capability. -/
theorem validateImport_accepts_iff_importable
    (declaration : ExportedDeclaration) :
    validateImport declaration = .ok () ↔ Importable declaration := by
  unfold validateImport Importable StaticDependenciesClosed
  cases categoryResult : validateBlueprintImportCategory declaration.entity with
  | error failure =>
      have notImportable : ¬ BlueprintEntityImportable declaration.entity := by
        intro importable
        have accepted :=
          (validateBlueprintImportCategory_accepts_iff declaration.entity).2 importable
        rw [categoryResult] at accepted
        contradiction
      simp [notImportable]
  | ok result =>
      cases result
      have importable : BlueprintEntityImportable declaration.entity :=
        (validateBlueprintImportCategory_accepts_iff declaration.entity).1 categoryResult
      rw [validateStaticDependenciesClosed_accepts_iff]
      simp [importable]

/-- Executable import target construction agrees with its normative identity. -/
theorem importTarget_accepts_iff
    (declaration : ExportedDeclaration)
    (target : ProjectionTarget) :
    importTarget declaration = .ok target ↔
      ImportTargetMatches declaration target := by
  unfold importTarget ImportTargetMatches
  cases validation : validateImport declaration with
  | error failure =>
      have notImportable : ¬ Importable declaration := by
        intro importable
        have accepted :=
          (validateImport_accepts_iff_importable declaration).2 importable
        rw [validation] at accepted
        contradiction
      simp [notImportable]
  | ok result =>
      cases result
      have importable : Importable declaration :=
        (validateImport_accepts_iff_importable declaration).1 validation
      simp [importable, eq_comm]

/-- Executable required-input lookup agrees with the typed binding relation. -/
theorem resolveStaticBinding_accepts_iff
    (bindings : StaticBindings)
    (input target : StaticEntity) :
    resolveStaticBinding bindings input = .ok target ↔
      StaticBindingResolves bindings input target := by
  constructor
  · intro accepted
    unfold resolveStaticBinding at accepted
    cases found : bindings input.id with
    | none => simp [found] at accepted
    | some candidate =>
        rw [found] at accepted
        by_cases valid : StaticBindingValid input candidate
        · simp [valid] at accepted
          subst target
          exact ⟨found, valid⟩
        · simp [valid] at accepted
  · rintro ⟨found, valid⟩
    unfold resolveStaticBinding
    rw [found]
    simp [valid]

/-- A successful deterministic lookup characterizes its unique normative target. -/
theorem staticBindingResolves_iff_eq_of_accepted
    (bindings : StaticBindings)
    (input acceptedTarget target : StaticEntity)
    (accepted : resolveStaticBinding bindings input = .ok acceptedTarget) :
    StaticBindingResolves bindings input target ↔ target = acceptedTarget := by
  constructor
  · intro resolved
    have targetAccepted :=
      (resolveStaticBinding_accepts_iff bindings input target).2 resolved
    rw [accepted] at targetAccepted
    exact (Except.ok.inj targetAccepted).symm
  · intro equal
    subst target
    exact (resolveStaticBinding_accepts_iff bindings input acceptedTarget).1 accepted

/-- Executable substitution normalization agrees with the normative relation. -/
theorem resolveStaticSubstitutions_accepts_iff
    (bindings : StaticBindings)
    (dependencies : List StaticEntity)
    (substitutions : List StaticSubstitutionEntry) :
    resolveStaticSubstitutions bindings dependencies = .ok substitutions ↔
      StaticSubstitutionsResolve bindings dependencies substitutions := by
  induction dependencies generalizing substitutions with
  | nil => simp [resolveStaticSubstitutions, StaticSubstitutionsResolve, eq_comm]
  | cons dependency rest ih =>
      cases role : dependency.inputRole with
      | none =>
          simp [resolveStaticSubstitutions, StaticSubstitutionsResolve, role, ih]
      | some inputRole =>
          obtain ⟨kind, inputRole⟩ := inputRole
          cases inputRole with
          | fixed =>
              simp [resolveStaticSubstitutions, StaticSubstitutionsResolve, role, ih]
          | optionalInput =>
              cases found : bindings dependency.id with
              | none =>
                  simp [resolveStaticSubstitutions, StaticSubstitutionsResolve,
                    StaticBindingResolves, role, found, ih]
              | some candidate =>
                  cases bindingResult : resolveStaticBinding bindings dependency with
                  | error failure =>
                      have noBinding : ∀ target,
                          ¬ StaticBindingResolves bindings dependency target := by
                        intro target resolved
                        have accepted :=
                          (resolveStaticBinding_accepts_iff bindings dependency target).2 resolved
                        rw [bindingResult] at accepted
                        contradiction
                      simp [resolveStaticSubstitutions, StaticSubstitutionsResolve,
                        role, found, bindingResult, noBinding]
                  | ok target =>
                      have unique (other : StaticEntity) :
                          StaticBindingResolves bindings dependency other ↔ other = target :=
                        staticBindingResolves_iff_eq_of_accepted
                          bindings dependency target other bindingResult
                      cases tailResult : resolveStaticSubstitutions bindings rest with
                      | error failure =>
                          have noTail : ∀ tail,
                              ¬ StaticSubstitutionsResolve bindings rest tail := by
                            intro tail resolved
                            have tailAccepted := (ih tail).2 resolved
                            rw [tailResult] at tailAccepted
                            contradiction
                          simp [resolveStaticSubstitutions, StaticSubstitutionsResolve,
                            role, found, bindingResult, tailResult, unique, noTail]
                      | ok tail =>
                          have tailResolved : StaticSubstitutionsResolve bindings rest tail :=
                            (ih tail).1 tailResult
                          simp [resolveStaticSubstitutions, role, found, bindingResult,
                            tailResult, StaticSubstitutionsResolve, unique]
                          constructor
                          · intro equal
                            exact ⟨tail, equal.symm, tailResolved⟩
                          · rintro ⟨other, equal, otherResolved⟩
                            have otherAccepted := (ih other).2 otherResolved
                            rw [tailResult] at otherAccepted
                            have same : tail = other := Except.ok.inj otherAccepted
                            subst other
                            exact equal.symm
          | requiredInput =>
              cases bindingResult : resolveStaticBinding bindings dependency with
              | error failure =>
                  have noBinding : ∀ target,
                      ¬ StaticBindingResolves bindings dependency target := by
                    intro target resolved
                    have accepted :=
                      (resolveStaticBinding_accepts_iff bindings dependency target).2 resolved
                    rw [bindingResult] at accepted
                    contradiction
                  simp [resolveStaticSubstitutions, StaticSubstitutionsResolve,
                    role, bindingResult, noBinding]
              | ok target =>
                  have unique (other : StaticEntity) :
                      StaticBindingResolves bindings dependency other ↔ other = target :=
                    staticBindingResolves_iff_eq_of_accepted
                      bindings dependency target other bindingResult
                  cases tailResult : resolveStaticSubstitutions bindings rest with
                  | error failure =>
                      have noTail : ∀ tail,
                          ¬ StaticSubstitutionsResolve bindings rest tail := by
                        intro tail resolved
                        have tailAccepted := (ih tail).2 resolved
                        rw [tailResult] at tailAccepted
                        contradiction
                      simp [resolveStaticSubstitutions, StaticSubstitutionsResolve,
                        role, bindingResult, tailResult, unique, noTail]
                  | ok tail =>
                      have tailResolved : StaticSubstitutionsResolve bindings rest tail :=
                        (ih tail).1 tailResult
                      simp [resolveStaticSubstitutions, role, bindingResult,
                        tailResult, StaticSubstitutionsResolve, unique]
                      constructor
                      · intro equal
                        exact ⟨tail, equal.symm, tailResolved⟩
                      · rintro ⟨other, equal, otherResolved⟩
                        have otherAccepted := (ih other).2 otherResolved
                        rw [tailResult] at otherAccepted
                        have same : tail = other := Except.ok.inj otherAccepted
                        subst other
                        exact equal.symm

/-- Executable target construction agrees with the normative category table. -/
theorem projectTarget_accepts_iff
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (entity : Entity)
    (substitutions : List StaticSubstitutionEntry)
    (target : ProjectionTarget) :
    projectTarget bindings instanceId entity substitutions = .ok target ↔
      ProjectionTargetMatches bindings instanceId entity substitutions target := by
  cases classified : includeProjection? entity with
  | none => simp [projectTarget, ProjectionTargetMatches, classified]
  | some projection =>
      cases projection with
      | staticDeclaration staticEntity =>
          cases role : staticEntity.inputRole with
          | none => simp [projectTarget, ProjectionTargetMatches, classified, role]
          | some inputRole =>
              obtain ⟨kind, inputRole⟩ := inputRole
              cases inputRole with
              | fixed =>
                  simp [projectTarget, ProjectionTargetMatches, classified, role, eq_comm]
              | optionalInput =>
                  cases found : bindings staticEntity.id with
                  | none =>
                      simp [projectTarget, ProjectionTargetMatches, classified,
                        StaticBindingResolves, role, found, eq_comm]
                  | some candidate =>
                      cases bindingResult : resolveStaticBinding bindings staticEntity with
                      | error failure =>
                          have noBinding : ∀ effective,
                              ¬ StaticBindingResolves bindings staticEntity effective := by
                            intro effective resolved
                            have accepted :=
                              (resolveStaticBinding_accepts_iff
                                bindings staticEntity effective).2 resolved
                            rw [bindingResult] at accepted
                            contradiction
                          simp [projectTarget, ProjectionTargetMatches, classified,
                            role, found, bindingResult, noBinding]
                      | ok effective =>
                          have unique (other : StaticEntity) :
                              StaticBindingResolves bindings staticEntity other ↔
                                other = effective :=
                            staticBindingResolves_iff_eq_of_accepted
                              bindings staticEntity effective other bindingResult
                          simp [projectTarget, ProjectionTargetMatches, classified,
                            role, found, bindingResult, unique, eq_comm]
              | requiredInput =>
                  cases bindingResult : resolveStaticBinding bindings staticEntity with
                  | error failure =>
                      have noBinding : ∀ effective,
                          ¬ StaticBindingResolves bindings staticEntity effective := by
                        intro effective resolved
                        have accepted :=
                          (resolveStaticBinding_accepts_iff
                            bindings staticEntity effective).2 resolved
                        rw [bindingResult] at accepted
                        contradiction
                      simp [projectTarget, ProjectionTargetMatches, classified,
                        role, bindingResult, noBinding]
                  | ok effective =>
                      have unique (other : StaticEntity) :
                          StaticBindingResolves bindings staticEntity other ↔
                            other = effective :=
                        staticBindingResolves_iff_eq_of_accepted
                          bindings staticEntity effective other bindingResult
                      simp [projectTarget, ProjectionTargetMatches, classified,
                        role, bindingResult, unique, eq_comm]
      | constructor termEntity owner =>
          cases found : bindings owner <;>
            simp [projectTarget, ProjectionTargetMatches, classified, found, eq_comm]
      | constNode termEntity
      | runtimeTerm termEntity
      | assertion termEntity
      | visualization termEntity
      | staticUnit termEntity
      | runtimeUnit termEntity =>
          simp [projectTarget, ProjectionTargetMatches, classified, eq_comm]

/-- Blueprint-importable target construction never observes runtime occurrence. -/
theorem blueprintImportable_projectTarget_instance_irrelevant
    (bindings : StaticBindings)
    (first second : InstanceId)
    (entity : Entity)
    (substitutions : List StaticSubstitutionEntry)
    (importable : BlueprintEntityImportable entity) :
    projectTarget bindings first entity substitutions =
      projectTarget bindings second entity substitutions := by
  cases entity with
  | static staticEntity =>
      cases staticEntity with
      | mk id kind => cases kind <;> try { rename_i role; cases role } <;>
          simp_all [BlueprintEntityImportable, projectTarget, includeProjection?]
  | term termEntity =>
      cases termEntity with
      | mk id kind => cases kind <;>
          simp_all [BlueprintEntityImportable, projectTarget, includeProjection?]
  | unit unitEntity =>
      cases unitEntity with
      | mk id kind => cases kind <;>
          simp_all [BlueprintEntityImportable, projectTarget, includeProjection?]

/-- Executable include projection succeeds exactly for the normative relation. -/
theorem project_accepts_iff_projects
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (declaration : ExportedDeclaration)
    (target : ProjectionTarget) :
    project bindings instanceId declaration = .ok target ↔
      Projects bindings instanceId declaration target := by
  unfold project Projects
  cases substitutionsResult : resolveStaticSubstitutions
      bindings declaration.staticDependencies with
  | error failure =>
      have noneResolve : ∀ substitutions,
          ¬ StaticSubstitutionsResolve
            bindings declaration.staticDependencies substitutions := by
        intro substitutions resolved
        have accepted :=
          (resolveStaticSubstitutions_accepts_iff
            bindings declaration.staticDependencies substitutions).2 resolved
        rw [substitutionsResult] at accepted
        contradiction
      simp [noneResolve]
  | ok substitutions =>
      have substitutionsResolve : StaticSubstitutionsResolve
          bindings declaration.staticDependencies substitutions :=
        (resolveStaticSubstitutions_accepts_iff
          bindings declaration.staticDependencies substitutions).1 substitutionsResult
      rw [projectTarget_accepts_iff]
      constructor
      · intro targetMatches
        exact ⟨substitutions, substitutionsResolve, targetMatches⟩
      · rintro ⟨other, otherResolve, targetMatches⟩
        have accepted :=
          (resolveStaticSubstitutions_accepts_iff
            bindings declaration.staticDependencies other).2 otherResolve
        rw [substitutionsResult] at accepted
        have same : substitutions = other := Except.ok.inj accepted
        subst other
        exact targetMatches

/-- Executable binding construction records exactly the occurrence, name, source, and target. -/
theorem projectBinding_accepts_iff
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (localName : NameAtom)
    (declaration : ExportedDeclaration)
    (binding : ProjectionBinding) :
    projectBinding bindings instanceId localName declaration = .ok binding ↔
      ProjectionBindingResolves bindings instanceId localName declaration binding := by
  unfold projectBinding ProjectionBindingResolves
  cases projected : project bindings instanceId declaration with
  | error failure =>
      have noneProjects : ∀ target, ¬ Projects bindings instanceId declaration target := by
        intro target projects
        have accepted :=
          (project_accepts_iff_projects bindings instanceId declaration target).2 projects
        rw [projected] at accepted
        contradiction
      simp [noneProjects]
  | ok target =>
      have projects : Projects bindings instanceId declaration target :=
        (project_accepts_iff_projects bindings instanceId declaration target).1 projected
      constructor
      · intro equal
        cases equal
        exact ⟨rfl, rfl, rfl, projects⟩
      · rintro ⟨instanceEq, nameEq, sourceEq, resolves⟩
        have targetEq : binding.target = target := by
          have accepted :=
            (project_accepts_iff_projects bindings instanceId declaration binding.target).2 resolves
          rw [projected] at accepted
          exact (Except.ok.inj accepted).symm
        cases binding
        simp_all

/-- A closed dependency closure normalizes to empty when no override is supplied. -/
theorem closedStaticDependencies_resolve_empty
    (dependencies : List StaticEntity)
    (closed : ∀ dependency, dependency ∈ dependencies →
      dependency.requiredInputKind = none) :
    resolveStaticSubstitutions (fun _ => none) dependencies = .ok [] := by
  induction dependencies with
  | nil => rfl
  | cons dependency rest ih =>
      have dependencyClosed : dependency.requiredInputKind = none :=
        closed dependency (by simp)
      have restClosed : ∀ candidate, candidate ∈ rest →
          candidate.requiredInputKind = none := by
        intro candidate member
        exact closed candidate (by simp [member])
      cases role : dependency.inputRole with
      | none => simp [resolveStaticSubstitutions, role, ih restClosed]
      | some inputRole =>
          obtain ⟨kind, inputRole⟩ := inputRole
          cases inputRole <;>
            simp_all [resolveStaticSubstitutions, StaticEntity.requiredInputKind]

/--
An entity accepted by both independent capabilities—blueprint import and
instance projection—has one shared static identity when no override is supplied.
-/
theorem importable_projectable_projects_to_static_identity
    (instanceId : InstanceId)
    (declaration : ExportedDeclaration)
    (importable : Importable declaration)
    (projectable : IncludeProjectable declaration.entity) :
    project (fun _ => none) instanceId declaration =
      .ok (.staticSpecialization ⟨declaration.entity, []⟩) := by
  cases declaration with
  | mk name entity dependencies =>
      obtain ⟨category, closed⟩ := importable
      have substitutions := closedStaticDependencies_resolve_empty dependencies closed
      unfold project
      rw [substitutions]
      cases entity with
      | static staticEntity =>
          cases staticEntity with
          | mk id kind =>
              cases kind with
              | nominalType role | dimension role | index role =>
                  cases role <;>
                    simp_all [BlueprintEntityImportable, IncludeProjectable,
                      projectTarget, includeProjection?, StaticEntity.inputRole]
              | genericTypeParam | genericDimParam | genericIndexParam
              | genericNatParam | timeScale =>
                  simp_all [IncludeProjectable, includeProjection?]
      | term termEntity =>
          cases termEntity with
          | mk id kind => cases kind <;>
              simp_all [BlueprintEntityImportable, IncludeProjectable,
                projectTarget, includeProjection?]
      | unit unitEntity =>
          cases unitEntity with
          | mk id kind => cases kind <;>
              simp_all [BlueprintEntityImportable, IncludeProjectable,
                projectTarget, includeProjection?]

/--
Direct import and unconfigured include projection share one semantic identity
only in the intersection of their independent capabilities.
-/
theorem import_target_matches_projection
    (instanceId : InstanceId)
    (declaration : ExportedDeclaration)
    (target : ProjectionTarget)
    (projectable : IncludeProjectable declaration.entity)
    (imported : importTarget declaration = .ok target) :
    project (fun _ => none) instanceId declaration = .ok target := by
  obtain ⟨importable, rfl⟩ :=
    (importTarget_accepts_iff declaration target).1 imported
  exact importable_projectable_projects_to_static_identity
    instanceId declaration importable projectable

/--
Importable declarations project applicatively: changing only the runtime include
occurrence cannot change either success or the semantic target.
-/
theorem importable_projection_instance_irrelevant
    (bindings : StaticBindings)
    (first second : InstanceId)
    (declaration : ExportedDeclaration)
    (importable : Importable declaration) :
    project bindings first declaration = project bindings second declaration := by
  obtain ⟨category, closed⟩ := importable
  unfold project
  cases resolveStaticSubstitutions bindings declaration.staticDependencies with
  | error failure => rfl
  | ok substitutions =>
      exact blueprintImportable_projectTarget_instance_irrelevant
        bindings first second declaration.entity substitutions category

end Graphcal.Static.ExternalSurface
