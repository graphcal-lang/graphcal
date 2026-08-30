import Lean.Data.Json
import Graphcal.Static.ExternalSurface.Reference

namespace Graphcal.Static.ExternalSurface.Oracle

open Lean
open Graphcal.Static.NamespaceResolution

/-- Stable boundary spelling for the three Static declaration roles. -/
inductive WireRole where
  | fixed
  | optionalInput
  | requiredInput
  deriving DecidableEq, Repr, ToJson

/-- Optional dependency/binding target without overloading JSON null semantics. -/
inductive WireTarget where
  | none
  | some (role : WireRole) (sameKind : Bool)
  deriving DecidableEq, Repr, ToJson

/-- Include entities whose distinction is not represented by a Static role. -/
inductive WireProjectionEntity where
  | constructor
  | dag
  deriving DecidableEq, Repr, ToJson

/-- Finite external-surface scenarios consumed by the Rust differential test. -/
inductive WireScenario where
  | importStatic (source : WireRole) (dependency : Option WireRole)
  | staticBinding (input target : WireRole) (sameKind : Bool)
  | projectStatic (source : WireRole) (target : WireTarget)
  | projectEntity (entity : WireProjectionEntity) (ownerRebound : Bool)
  deriving DecidableEq, Repr, ToJson

/-- Stable semantic outcome rather than implementation-specific diagnostic text. -/
inductive WireDecision where
  | accepted
  | projectedSource
  | projectedTarget
  | rejected (rule : String)
  deriving DecidableEq, Repr, ToJson

structure WireCase where
  scenario : WireScenario
  decision : WireDecision
  deriving DecidableEq, Repr, ToJson

private def rootDag : DagId := ⟨0, 0⟩
private def instanceId : InstanceId := ⟨rootDag, 0⟩

private def roleOfWire : WireRole → StaticRole
  | .fixed => .fixed
  | .optionalInput => .optionalInput
  | .requiredInput => .requiredInput

private def sourceStatic (role : WireRole) : StaticEntity := {
  id := ⟨rootDag, 0⟩
  kind := .nominalType (roleOfWire role)
}

private def targetStatic (role : WireRole) (sameKind : Bool) : StaticEntity := {
  id := ⟨rootDag, 1⟩
  kind := if sameKind then
    .nominalType (roleOfWire role)
  else
    .dimension (roleOfWire role)
}

private def dependencyStatic (role : WireRole) : StaticEntity := {
  id := ⟨rootDag, 2⟩
  kind := .index (roleOfWire role)
}

private def constructorOwner : StaticEntity := {
  id := ⟨rootDag, 3⟩
  kind := .nominalType .fixed
}

private def projectionEntity : WireProjectionEntity → Entity
  | .constructor => .term ⟨⟨rootDag, 0⟩, .constructor constructorOwner.id⟩
  | .dag => .term ⟨⟨rootDag, 1⟩, .dag rootDag⟩

private def importDecision
    (source : WireRole)
    (dependency : Option WireRole) : WireDecision :=
  let declaration : ExportedDeclaration := {
    name := "Subject"
    entity := .static (sourceStatic source)
    staticDependencies := dependency.map dependencyStatic |>.toList
  }
  match validateImport declaration with
  | .ok _ => .accepted
  | .error (.declarationNotImportable _) => .rejected "declarationNotImportable"
  | .error (.unresolvedStaticDependency _ _) => .rejected "unresolvedStaticDependency"

private def bindingDecision
    (input target : WireRole)
    (sameKind : Bool) : WireDecision :=
  let inputEntity := sourceStatic input
  let targetEntity := targetStatic target sameKind
  let bindings : StaticBindings := fun id =>
    if id = inputEntity.id then some targetEntity else none
  match resolveStaticBinding bindings inputEntity with
  | .ok _ => .accepted
  | .error (.missingStaticBinding _) => .rejected "missingStaticBinding"
  | .error (.invalidStaticBinding _ _) => .rejected "invalidStaticBinding"
  | .error (.declarationNotProjectable _) => .rejected "declarationNotProjectable"
  | .error (.constructorOwnerRebound _ _) => .rejected "constructorOwnerRebound"

private def projectionDecision
    (source : WireRole)
    (target : WireTarget) : WireDecision :=
  let sourceEntity := sourceStatic source
  let targetEntity := match target with
    | .none => none
    | .some role sameKind => some (targetStatic role sameKind)
  let bindings : StaticBindings := fun id =>
    if id = sourceEntity.id then targetEntity else none
  let declaration : ExportedDeclaration := {
    name := "Subject"
    entity := .static sourceEntity
    staticDependencies := []
  }
  match project bindings instanceId declaration with
  | .ok (.staticSpecialization specialization) =>
      if specialization.declaration = .static sourceEntity then
        .projectedSource
      else
        .projectedTarget
  | .ok _ => .rejected "unexpectedProjectionCategory"
  | .error (.missingStaticBinding _) => .rejected "missingStaticBinding"
  | .error (.invalidStaticBinding _ _) => .rejected "invalidStaticBinding"
  | .error (.declarationNotProjectable _) => .rejected "declarationNotProjectable"
  | .error (.constructorOwnerRebound _ _) => .rejected "constructorOwnerRebound"

private def entityProjectionDecision
    (kind : WireProjectionEntity)
    (ownerRebound : Bool) : WireDecision :=
  let entity := projectionEntity kind
  let bindings : StaticBindings := fun id =>
    if ownerRebound && id = constructorOwner.id then
      some (targetStatic .fixed true)
    else
      none
  let declaration : ExportedDeclaration := {
    name := "Subject"
    entity
    staticDependencies := []
  }
  match project bindings instanceId declaration with
  | .ok (.staticSpecialization specialization) =>
      if specialization.declaration = entity then
        .projectedSource
      else
        .projectedTarget
  | .ok _ => .rejected "unexpectedProjectionCategory"
  | .error (.missingStaticBinding _) => .rejected "missingStaticBinding"
  | .error (.invalidStaticBinding _ _) => .rejected "invalidStaticBinding"
  | .error (.declarationNotProjectable _) => .rejected "declarationNotProjectable"
  | .error (.constructorOwnerRebound _ _) => .rejected "constructorOwnerRebound"

private def roles : List WireRole := [.fixed, .optionalInput, .requiredInput]
private def dependencies : List (Option WireRole) :=
  [none, some .fixed, some .optionalInput, some .requiredInput]
private def targets : List WireTarget :=
  [.none] ++ roles.flatMap fun role => [.some role false, .some role true]
private def sameKinds : List Bool := [false, true]
private def projectionEntities : List WireProjectionEntity := [.constructor, .dag]

private def importCases : List WireCase :=
  roles.flatMap fun source =>
    dependencies.map fun dependency => {
      scenario := .importStatic source dependency
      decision := importDecision source dependency
    }

private def bindingCases : List WireCase :=
  roles.flatMap fun input =>
    roles.flatMap fun target =>
      sameKinds.map fun sameKind => {
        scenario := .staticBinding input target sameKind
        decision := bindingDecision input target sameKind
      }

private def projectionCases : List WireCase :=
  roles.flatMap fun source =>
    targets.map fun target => {
      scenario := .projectStatic source target
      decision := projectionDecision source target
    }

private def entityProjectionCases : List WireCase :=
  projectionEntities.flatMap fun entity =>
    sameKinds.map fun ownerRebound => {
      scenario := .projectEntity entity ownerRebound
      decision := entityProjectionDecision entity ownerRebound
    }

/-- Reviewed finite domain: 12 import + 18 binding + 21 Static projection + 4 entity states. -/
def cases : List WireCase :=
  importCases ++ bindingCases ++ projectionCases ++ entityProjectionCases

theorem cases_count : cases.length = 55 := by
  rfl

end Graphcal.Static.ExternalSurface.Oracle

/-- Emit one JSON document for the production Rust differential test. -/
def main : IO Unit :=
  IO.println <| Lean.toJson Graphcal.Static.ExternalSurface.Oracle.cases |>.compress
