import Graphcal.Static.ExternalSurface.Proofs

namespace Graphcal.Static.ExternalSurface.Examples

open Graphcal.Static.NamespaceResolution
open Graphcal.Static.ExternalSurface

/-- Lean core intentionally leaves `Except` equality to clients. -/
private instance {error value : Type} [DecidableEq error] [DecidableEq value] :
    DecidableEq (Except error value)
  | .error first, .error second =>
      match decEq first second with
      | .isTrue equal => .isTrue (by cases equal; rfl)
      | .isFalse different => .isFalse fun equal => different (Except.error.inj equal)
  | .ok first, .ok second =>
      match decEq first second with
      | .isTrue equal => .isTrue (by cases equal; rfl)
      | .isFalse different => .isFalse fun equal => different (Except.ok.inj equal)
  | .error _, .ok _ => .isFalse fun equal => by contradiction
  | .ok _, .error _ => .isFalse fun equal => by contradiction

private def rootDag : DagId := ⟨0, 0⟩
private def libraryDag : DagId := ⟨1, 0⟩
private def firstInstance : InstanceId := ⟨libraryDag, 0⟩
private def secondInstance : InstanceId := ⟨libraryDag, 1⟩

private def staticId (owner : DagId) (serial : Nat) : StaticId := ⟨owner, serial⟩
private def termId (owner : DagId) (serial : Nat) : TermId := ⟨owner, serial⟩
private def unitId (owner : DagId) (serial : Nat) : UnitId := ⟨owner, serial⟩

private def requiredElement : StaticEntity := {
  id := staticId libraryDag 0
  kind := .nominalType .requiredInput
}

private def localElement : StaticEntity := {
  id := staticId rootDag 0
  kind := .nominalType .fixed
}

private def elementBindings : StaticBindings := fun id =>
  if id = requiredElement.id then some localElement else none

private def noBindings : StaticBindings := fun _ => none

private def requiredElementDecl : ExportedDeclaration := {
  name := "Element"
  entity := .static requiredElement
  staticDependencies := []
}

private def localElementDecl : ExportedDeclaration := {
  name := "LocalElement"
  entity := .static localElement
  staticDependencies := []
}

/-- A required Static input hole cannot cross a direct import boundary. -/
theorem required_static_input_is_not_importable :
    validateImport requiredElementDecl =
      .error (.declarationNotImportable (.static requiredElement)) := by
  decide

private def boxEntity : StaticEntity := {
  id := staticId libraryDag 1
  kind := .nominalType .fixed
}

private def boxDecl : ExportedDeclaration := {
  name := "Box"
  entity := .static boxEntity
  staticDependencies := [requiredElement]
}

/-- A concrete declaration with an unresolved transitive input is not importable. -/
theorem static_dependent_declaration_is_not_importable :
    validateImport boxDecl =
      .error (.unresolvedStaticDependency (.static boxEntity) requiredElement) := by
  decide

private def optionalElement : StaticEntity := {
  id := staticId libraryDag 3
  kind := .nominalType .optionalInput
}

private def alternateElement : StaticEntity := {
  id := staticId rootDag 1
  kind := .nominalType .fixed
}

private def optionalBindings : StaticBindings := fun id =>
  if id = optionalElement.id then some alternateElement else none

private def optionalElementDecl : ExportedDeclaration := {
  name := "DefaultElement"
  entity := .static optionalElement
  staticDependencies := []
}

private def optionalBoxEntity : StaticEntity := {
  id := staticId libraryDag 4
  kind := .nominalType .fixed
}

private def optionalBoxDecl : ExportedDeclaration := {
  name := "DefaultBox"
  entity := .static optionalBoxEntity
  staticDependencies := [optionalElement]
}

/-- Optional Static inputs remain importable through their local defaults. -/
theorem optional_static_default_is_importable :
    validateImport optionalElementDecl = .ok () ∧
      validateImport optionalBoxDecl = .ok () := by
  decide

/-- Supplying an optional input projects its effective target instead of its default. -/
theorem optional_static_override_returns_effective_target :
    project optionalBindings firstInstance optionalElementDecl =
      .ok (.staticSpecialization ⟨.static alternateElement, []⟩) := by
  decide

/-- An optional override participates in applicative specialization identity. -/
theorem optional_static_override_specializes_dependents :
    project optionalBindings firstInstance optionalBoxDecl =
      .ok (.staticSpecialization ⟨.static optionalBoxEntity, [{
        input := optionalElement.id
        target := alternateElement
      }]⟩) := by
  decide

private def tokenEntity : StaticEntity := {
  id := staticId libraryDag 2
  kind := .nominalType .fixed
}

private def tokenDecl : ExportedDeclaration := {
  name := "Token"
  entity := .static tokenEntity
  staticDependencies := []
}

/-- A closed concrete type remains directly importable. -/
theorem closed_type_is_importable : validateImport tokenDecl = .ok () := by
  decide

private def tokenTarget : ProjectionTarget :=
  .staticSpecialization {
    declaration := .static tokenEntity
    substitutions := []
  }

/-- Direct import and include projection of a closed type share one identity. -/
theorem imported_and_projected_closed_type_have_same_identity :
    importTarget tokenDecl = .ok tokenTarget ∧
      project noBindings firstInstance tokenDecl = .ok tokenTarget := by
  decide

/-- Projecting a bound required input has exactly its concrete target's identity. -/
theorem required_static_projection_returns_effective_target :
    project elementBindings firstInstance requiredElementDecl =
      project noBindings secondInstance localElementDecl := by
  decide

private def localSubstitution : StaticSubstitutionEntry := {
  input := requiredElement.id
  target := localElement
}

private def specializedBox : ProjectionTarget :=
  .staticSpecialization {
    declaration := .static boxEntity
    substitutions := [localSubstitution]
  }

/-- Runtime include occurrence does not change one Static specialization. -/
theorem same_static_binding_is_applicative :
    project elementBindings firstInstance boxDecl = .ok specializedBox ∧
      project elementBindings secondInstance boxDecl = .ok specializedBox := by
  decide

private def firstTokenBinding : ProjectionBinding := {
  instanceId := firstInstance
  name := "FirstToken"
  source := .static tokenEntity.id
  target := tokenTarget
}

private def secondTokenBinding : ProjectionBinding := {
  instanceId := secondInstance
  name := "SecondToken"
  source := .static tokenEntity.id
  target := tokenTarget
}

/-- Projection bindings retain provenance while sharing one closed Static target. -/
theorem closed_static_projection_has_instance_provenance_and_shared_identity :
    projectBinding noBindings firstInstance "FirstToken" tokenDecl =
        .ok firstTokenBinding ∧
      projectBinding noBindings secondInstance "SecondToken" tokenDecl =
        .ok secondTokenBinding ∧
      firstTokenBinding.target = secondTokenBinding.target ∧
      firstTokenBinding.instanceId ≠ secondTokenBinding.instanceId := by
  decide

private def runtimeNode : TermEntity := {
  id := termId libraryDag 0
  kind := .node
}

private def runtimeNodeDecl : ExportedDeclaration := {
  name := "result"
  entity := .term runtimeNode
  staticDependencies := []
}

/-- Runtime nodes retain concrete include occurrence in semantic identity. -/
theorem runtime_node_projections_are_instance_specific :
    project noBindings firstInstance runtimeNodeDecl ≠
      project noBindings secondInstance runtimeNodeDecl := by
  decide

private def runtimeUnit : UnitEntity := {
  id := unitId libraryDag 0
  kind := .runtimeScaled
}

private def runtimeUnitDecl : ExportedDeclaration := {
  name := "EUR"
  entity := .unit runtimeUnit
  staticDependencies := []
}

/-- Plain units are projectable but never directly importable. -/
theorem runtime_unit_is_projectable_not_importable :
    validateImport runtimeUnitDecl =
        .error (.declarationNotImportable (.unit runtimeUnit)) ∧
      project noBindings firstInstance runtimeUnitDecl =
        .ok (.runtimeUnit ⟨runtimeUnit.id, firstInstance⟩ []) := by
  decide

/-- Repeated includes retain distinct runtime unit scale identities. -/
theorem runtime_unit_scales_are_instance_specific :
    project noBindings firstInstance runtimeUnitDecl ≠
      project noBindings secondInstance runtimeUnitDecl := by
  decide

private def constUnit : UnitEntity := {
  id := unitId libraryDag 1
  kind := .constScaled
}

private def constUnitDecl : ExportedDeclaration := {
  name := "km"
  entity := .unit constUnit
  staticDependencies := []
}

/-- A const unit is both importable and projectable to one Static target. -/
theorem const_unit_supports_both_boundaries :
    validateImport constUnitDecl = .ok () ∧
      project noBindings firstInstance constUnitDecl =
        .ok (.staticSpecialization ⟨.unit constUnit, []⟩) := by
  decide

private def constNode : TermEntity := {
  id := termId libraryDag 1
  kind := .constNode
}

private def constNodeDecl : ExportedDeclaration := {
  name := "factor"
  entity := .term constNode
  staticDependencies := []
}

/-- A const node remains valid through both import and include projection. -/
theorem const_node_supports_both_boundaries :
    validateImport constNodeDecl = .ok () ∧
      project noBindings firstInstance constNodeDecl =
        .ok (.staticSpecialization ⟨.term constNode, []⟩) := by
  decide

end Graphcal.Static.ExternalSurface.Examples
