import Graphcal.Static.ExternalSurface.Model

namespace Graphcal.Static.ExternalSurface

open Graphcal.Static
open Graphcal.Static.NamespaceResolution

/-- Blueprint categories that may cross a direct `import` boundary. -/
def BlueprintEntityImportable : Entity → Prop
  | .static ⟨_, .nominalType .fixed⟩ => True
  | .static ⟨_, .nominalType .optionalInput⟩ => True
  | .static ⟨_, .dimension .fixed⟩ => True
  | .static ⟨_, .dimension .optionalInput⟩ => True
  | .static ⟨_, .index .fixed⟩ => True
  | .static ⟨_, .index .optionalInput⟩ => True
  | .term ⟨_, .constNode⟩ => True
  | .term ⟨_, .constructor _⟩ => True
  | .term ⟨_, .dag _⟩ => True
  | .term ⟨_, .moduleAlias _⟩ => True
  | .term ⟨_, .publicDagAlias _⟩ => True
  | .unit ⟨_, .prelude⟩ => True
  | .unit ⟨_, .base⟩ => True
  | .unit ⟨_, .constScaled⟩ => True
  | _ => False

/-- No transitive Static dependency may remain a required input hole. -/
def StaticDependenciesClosed (declaration : ExportedDeclaration) : Prop :=
  ∀ dependency,
    dependency ∈ declaration.staticDependencies →
      dependency.requiredInputKind = none

/-- Normative direct-import capability. -/
def Importable (declaration : ExportedDeclaration) : Prop :=
  BlueprintEntityImportable declaration.entity ∧
    StaticDependenciesClosed declaration

/-- Normative semantic target introduced by a successful direct import. -/
def ImportTargetMatches
    (declaration : ExportedDeclaration)
    (target : ProjectionTarget) : Prop :=
  Importable declaration ∧
    target = .staticSpecialization ⟨declaration.entity, []⟩

/-- One required or optional Static input resolves to one concrete target of the same kind. -/
def StaticBindingResolves
    (bindings : StaticBindings)
    (input target : StaticEntity) : Prop :=
  bindings input.id = some target ∧ StaticBindingValid input target

/--
Normative normalization of bindable entries from a resolved transitive Static
dependency closure. Fixed dependencies contribute no substitution, optional
inputs contribute one exactly when supplied, and required inputs must resolve.
-/
def StaticSubstitutionsResolve
    (bindings : StaticBindings) :
    List StaticEntity → List StaticSubstitutionEntry → Prop
  | [], substitutions => substitutions = []
  | dependency :: rest, substitutions =>
      match dependency.inputRole with
      | some (_, .optionalInput) =>
          (bindings dependency.id = none ∧
              StaticSubstitutionsResolve bindings rest substitutions) ∨
            ∃ target tail,
              StaticBindingResolves bindings dependency target ∧
                substitutions = { input := dependency.id, target } :: tail ∧
                StaticSubstitutionsResolve bindings rest tail
      | some (_, .requiredInput) =>
          ∃ target tail,
            StaticBindingResolves bindings dependency target ∧
              substitutions = { input := dependency.id, target } :: tail ∧
              StaticSubstitutionsResolve bindings rest tail
      | _ => StaticSubstitutionsResolve bindings rest substitutions

/-- The exact target category selected by one resolved include declaration. -/
def ProjectionTargetMatches
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (entity : Entity)
    (substitutions : List StaticSubstitutionEntry)
    (target : ProjectionTarget) : Prop :=
  match includeProjection? entity with
  | none => False
  | some (.staticDeclaration staticEntity) =>
      match staticEntity.inputRole with
      | some (_, .requiredInput) =>
          ∃ effective,
            StaticBindingResolves bindings staticEntity effective ∧
              target = .staticSpecialization ⟨.static effective, []⟩
      | some (_, .optionalInput) =>
          (bindings staticEntity.id = none ∧
              target = .staticSpecialization ⟨.static staticEntity, substitutions⟩) ∨
            ∃ effective,
              StaticBindingResolves bindings staticEntity effective ∧
                target = .staticSpecialization ⟨.static effective, []⟩
      | some (_, .fixed) =>
          target = .staticSpecialization ⟨.static staticEntity, substitutions⟩
      | none => False
  | some (.constNode termEntity) =>
      target = .staticSpecialization ⟨.term termEntity, substitutions⟩
  | some (.constructor termEntity owner) =>
      bindings owner = none ∧
        target = .staticSpecialization ⟨.term termEntity, substitutions⟩
  | some (.runtimeTerm termEntity) =>
      target = .runtimeTerm ⟨.term termEntity, instanceId, substitutions⟩
  | some (.assertion termEntity) =>
      target = .assertion ⟨.term termEntity, instanceId, substitutions⟩
  | some (.visualization termEntity) =>
      target = .visualization ⟨.term termEntity, instanceId, substitutions⟩
  | some (.staticUnit unitEntity) =>
      target = .staticSpecialization ⟨.unit unitEntity, substitutions⟩
  | some (.runtimeUnit unitEntity) =>
      target = .runtimeUnit ⟨unitEntity.id, instanceId⟩ substitutions

/-- Normative include projection after namespace and visibility resolution. -/
def Projects
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (declaration : ExportedDeclaration)
    (target : ProjectionTarget) : Prop :=
  ∃ substitutions,
    StaticSubstitutionsResolve
      bindings declaration.staticDependencies substitutions ∧
    ProjectionTargetMatches
      bindings instanceId declaration.entity substitutions target

/-- Normative source-visible projection binding. -/
def ProjectionBindingResolves
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (localName : NameAtom)
    (declaration : ExportedDeclaration)
    (binding : ProjectionBinding) : Prop :=
  binding.instanceId = instanceId ∧
    binding.name = localName ∧
    binding.source = entityId declaration.entity ∧
    Projects bindings instanceId declaration binding.target

end Graphcal.Static.ExternalSurface
