import Lean.Data.Json
import Graphcal.Static.NamespaceResolution.Reference

namespace Graphcal.Static.NamespaceResolution.Oracle

open Lean

/-- Stable JSON spelling for the three modeled namespaces. -/
inductive WireNamespace where
  | static
  | term
  | unit
  deriving DecidableEq, Repr, ToJson

/-- Stable JSON spelling for the four legal DAG-input selector forms. -/
inductive WireInputCategory where
  | unmarked
  | nominalType
  | dimension
  | index
  deriving DecidableEq, Repr, ToJson

/--
A finite differential-test scenario. Structured variants preserve which source
boundary is under test; strings occur only in the label scenario at this JSON
boundary.
-/
inductive WireScenario where
  | visibleLookup (occupied : List WireNamespace) (query : WireNamespace)
  | memberLookup (occupied : List WireNamespace) (query : WireNamespace)
  | duplicateSlot (space : WireNamespace)
  | inputBinding (target : WireInputCategory) (selector : WireInputCategory)
  | label (owner : String) (label : String)
  deriving Repr, ToJson

/-- The oracle reports the reviewed Lean rule constructor, not diagnostic prose. -/
inductive WireDecision where
  | accepted
  | rejected (rule : String)
  deriving Repr, ToJson

/-- One complete semantic observation consumed by the Rust differential test. -/
structure WireCase where
  scenario : WireScenario
  decision : WireDecision
  deriving Repr, ToJson

private def rootDag : DagId := ⟨0, 0⟩
private def libraryDag : DagId := ⟨0, 1⟩
private def blueprintDag : DagId := ⟨0, 2⟩

private def namespaceWire : Namespace → WireNamespace
  | .static => .static
  | .term => .term
  | .unit => .unit

private def inputCategoryWire : InputBindingCategory → WireInputCategory
  | .unmarked => .unmarked
  | .marked .nominalType => .nominalType
  | .marked .dimension => .dimension
  | .marked .index => .index

private def resolutionRule : ResolutionError → String
  | .unknown _ => "unknown"
  | .ambiguous _ => "ambiguous"
  | .notNamespaceOwner _ => "notNamespaceOwner"
  | .nonDagPathSegment _ => "nonDagPathSegment"
  | .cannotTraverse _ _ => "cannotTraverse"
  | .wrongNamespace _ _ => "wrongNamespace"
  | .invalidStaticUse _ _ => "invalidStaticUse"
  | .invalidTermUse _ _ => "invalidTermUse"
  | .invalidInputTarget _ _ => "invalidInputTarget"
  | .labelOwnerNotIndex _ => "labelOwnerNotIndex"
  | .labelOwnerMismatch _ _ => "labelOwnerMismatch"

private def resolutionDecision {α : Type} : Except ResolutionError α → WireDecision
  | .ok _ => .accepted
  | .error error => .rejected (resolutionRule error)

private def scopeDecision {α : Type} : Except ScopeError α → WireDecision
  | .ok _ => .accepted
  | .error (.duplicate _) => .rejected "duplicateSlot"

private def entityForNamespace
    (owner : DagId)
    (serial : Nat) : Namespace → Entity
  | .static => .static {
      id := ⟨owner, serial⟩
      kind := .dimension .fixed
    }
  | .term => .term {
      id := ⟨owner, serial⟩
      kind := .constructor ⟨owner, 100⟩
    }
  | .unit => .unit {
      id := ⟨owner, serial⟩
      kind := .base
    }

private def subjectBinding
    (scope : ScopeId)
    (owner : DagId)
    (serial : Nat)
    (space : Namespace) : Binding := {
  scope
  name := "Subject"
  entity := entityForNamespace owner serial space
}

private def namespaceOccupancies : List (List Namespace) :=
  [
    [],
    [.static],
    [.term],
    [.unit],
    [.static, .term],
    [.static, .unit],
    [.term, .unit],
    [.static, .term, .unit]
  ]

/-- Every subset of the three namespaces appears exactly once in the matrix. -/
theorem namespaceOccupancies_count : namespaceOccupancies.length = 8 := by
  rfl

private def namespaces : List Namespace := [.static, .term, .unit]

private def visibleEnvironment (occupied : List Namespace) : Environment :=
  occupied.mapIdx fun serial space =>
    subjectBinding (.dag rootDag) rootDag serial space

private def memberEnvironment (occupied : List Namespace) : Environment :=
  {
    scope := .dag rootDag
    name := "lib"
    entity := .term {
      id := ⟨rootDag, 99⟩
      kind := .moduleAlias libraryDag
    }
  } :: occupied.mapIdx fun serial space =>
    subjectBinding (.dag libraryDag) libraryDag serial space

private def referenceFor
    (head : NameHead) : Namespace → Reference
  | .static => .static head .dimension
  | .term => .term head .bareValue
  | .unit => .unit head

private def visibleLookupCases : List WireCase :=
  namespaceOccupancies.flatMap fun occupied =>
    namespaces.map fun query => {
      scenario := .visibleLookup (occupied.map namespaceWire) (namespaceWire query)
      decision := resolutionDecision <|
        resolve
          (visibleEnvironment occupied)
          (referenceFor (.visible [.dag rootDag] "Subject") query)
    }

private def memberLookupCases : List WireCase :=
  namespaceOccupancies.flatMap fun occupied =>
    namespaces.map fun query => {
      scenario := .memberLookup (occupied.map namespaceWire) (namespaceWire query)
      decision := resolutionDecision <|
        resolve
          (memberEnvironment occupied)
          (referenceFor
            (.member rootDag { root := "lib", children := [] } "Subject")
            query)
    }

private def duplicateBindings (space : Namespace) : List Binding :=
  [
    subjectBinding (.dag rootDag) rootDag 0 space,
    subjectBinding (.dag rootDag) rootDag 1 space
  ]

private def duplicateCases : List WireCase :=
  namespaces.map fun space => {
    scenario := .duplicateSlot (namespaceWire space)
    decision := scopeDecision (buildScope (duplicateBindings space))
  }

private def inputCategories : List InputBindingCategory :=
  [.unmarked, .marked .nominalType, .marked .dimension, .marked .index]

private def inputEntity : InputBindingCategory → Entity
  | .unmarked => .term {
      id := ⟨blueprintDag, 0⟩
      kind := .param
    }
  | .marked kind => .static {
      id := ⟨blueprintDag, match kind with
        | .nominalType => 1
        | .dimension => 2
        | .index => 3⟩
      kind := kind.toStaticKind .requiredInput
    }

private def inputEnvironment (target : InputBindingCategory) : Environment :=
  [{
    scope := .dag blueprintDag
    name := "Slot"
    entity := inputEntity target
  }]

private def inputCases : List WireCase :=
  inputCategories.flatMap fun target =>
    inputCategories.map fun selector => {
      scenario := .inputBinding (inputCategoryWire target) (inputCategoryWire selector)
      decision := resolutionDecision <|
        resolveInputBinding
          (inputEnvironment target)
          {
            category := selector
            target := .visible [.dag blueprintDag] "Slot"
          }
    }

private def indexAId : StaticId := ⟨rootDag, 10⟩
private def indexBId : StaticId := ⟨rootDag, 11⟩

private def labelEnvironment : Environment :=
  ({
    owner := rootDag
    name := "A"
    id := indexAId
    labels := [{ name := "Same", id := ⟨rootDag, 20⟩ }]
  } : IndexDecl).bindings ++
  ({
    owner := rootDag
    name := "B"
    id := indexBId
    labels := [{ name := "Same", id := ⟨rootDag, 21⟩ }]
  } : IndexDecl).bindings ++
  [{
    scope := .dag rootDag
    name := "NotIndex"
    entity := .static {
      id := ⟨rootDag, 12⟩
      kind := .dimension .fixed
    }
  }]

private def labelQueries : List (String × String) :=
  [
    ("A", "Same"),
    ("B", "Same"),
    ("MissingOwner", "Same"),
    ("A", "Missing"),
    ("NotIndex", "Same")
  ]

private def labelCases : List WireCase :=
  labelQueries.map fun (owner, label) => {
    scenario := .label owner label
    decision := resolutionDecision <|
      resolve
        labelEnvironment
        (.label (.visible [.dag rootDag] owner) label)
  }

/--
The reviewed differential domain covers 24 visible lookups, 24 member lookups,
three duplicate slots, all 4 × 4 input-category combinations, and five label
owner/label outcomes.
-/
def cases : List WireCase :=
  visibleLookupCases ++
    memberLookupCases ++
    duplicateCases ++
    inputCases ++
    labelCases

/-- The finite namespace conformance matrix currently contains 72 states. -/
theorem cases_count : cases.length = 72 := by
  rfl

end Graphcal.Static.NamespaceResolution.Oracle

/-- Emit one JSON document for the Rust differential conformance test. -/
def main : IO Unit :=
  IO.println <|
    Lean.toJson Graphcal.Static.NamespaceResolution.Oracle.cases |>.compress
