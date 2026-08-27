import Graphcal.Static.NamespaceResolution.Proofs

namespace Graphcal.Static.NamespaceResolution.Examples

open Graphcal.Static.NamespaceResolution

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
private def instanceId : InstanceId := ⟨libraryDag, 0⟩
private def localScope : LexicalScopeId := ⟨rootDag, 0⟩

private def staticId (owner : DagId) (serial : Nat) : StaticId := ⟨owner, serial⟩
private def termId (owner : DagId) (serial : Nat) : TermId := ⟨owner, serial⟩

private def positionDecl : NominalTypeDecl := {
  owner := rootDag
  name := "Position"
  id := staticId rootDag 0
  constructors := [{ name := "Position", id := termId rootDag 0 }]
}

/-- A Static type and same-spelled flat Term constructor occupy distinct slots. -/
theorem type_and_primary_constructor_coexist :
    buildScope positionDecl.bindings = .ok positionDecl.bindings := by
  decide

private def phaseDecl : IndexDecl := {
  owner := rootDag
  name := "Phase"
  id := staticId rootDag 1
  labels := [
    { name := "Start", id := termId rootDag 1 },
    { name := "End", id := termId rootDag 2 }
  ]
}

private def missionDecl : IndexDecl := {
  owner := rootDag
  name := "Mission"
  id := staticId rootDag 2
  labels := [
    { name := "Start", id := termId rootDag 3 },
    { name := "End", id := termId rootDag 4 }
  ]
}

/-- Equal label spellings under different canonical indexes do not collide. -/
theorem labels_with_different_owners_coexist :
    buildScope (phaseDecl.bindings ++ missionDecl.bindings) =
      .ok (phaseDecl.bindings ++ missionDecl.bindings) := by
  decide

private def duplicateLabelDecl : IndexDecl := {
  owner := rootDag
  name := "Duplicate"
  id := staticId rootDag 3
  labels := [
    { name := "Same", id := termId rootDag 5 },
    { name := "Same", id := termId rootDag 6 }
  ]
}

/-- Equal label spellings under one canonical index are rejected. -/
theorem duplicate_label_under_one_owner_is_rejected :
    buildScope duplicateLabelDecl.bindings ≠ .ok duplicateLabelDecl.bindings := by
  decide

private def phaseAlias : Binding := {
  scope := .dag rootDag
  name := "Phase"
  entity := .term {
    id := termId rootDag 7
    kind := .moduleAlias libraryDag
  }
}

private def externalState : StaticEntity := {
  id := staticId libraryDag 0
  kind := .nominalType
}

private def externalStateBinding : Binding := {
  scope := .dag libraryDag
  name := "State"
  entity := .static externalState
}

private def phaseAliasEnvironment : Environment :=
  phaseDecl.bindings ++ [phaseAlias, externalStateBinding]

private def phaseLabelEntity : TermEntity := {
  id := termId rootDag 1
  kind := .indexLabel phaseDecl.id
}

/-- `Phase#Start` selects Static `Phase`, even beside same-spelled Term alias. -/
theorem label_selector_chooses_static_owner :
    resolve phaseAliasEnvironment
      (.label (.visible [.dag rootDag] "Phase") "Start") =
      .ok (.term phaseLabelEntity) := by
  decide

/-- `Phase::State` selects the same-spelled Term alias as a namespace owner. -/
theorem member_boundary_chooses_term_owner :
    resolve phaseAliasEnvironment
      (.static
        (.member rootDag { root := "Phase", children := [] } "State")
        .type) =
      .ok (.static externalState) := by
  decide

private def modelAlias (name : NameAtom) (serial : Nat) : Binding := {
  scope := .dag rootDag
  name
  entity := .term {
    id := termId rootDag serial
    kind := .moduleAlias libraryDag
  }
}

private def factorEntity : TermEntity := {
  id := termId libraryDag 0
  kind := .constNode
}

private def resultEntity : TermEntity := {
  id := termId libraryDag 1
  kind := .node
}

private def libraryEnvironment : Environment := [
  modelAlias "model" 8,
  modelAlias "facade" 9,
  {
    scope := .dag libraryDag
    name := "FACTOR"
    entity := .term factorEntity
  },
  {
    scope := .dag libraryDag
    name := "result"
    entity := .term resultEntity
  }
]

private def modelHead (root : NameAtom) (member : NameAtom) : NameHead :=
  .member rootDag { root, children := [] } member

/-- An exported constant can be read through an uninstantiated blueprint. -/
theorem blueprint_constant_is_readable :
    resolve libraryEnvironment
      (.term (modelHead "model" "FACTOR") .blueprintGraphRead) =
      .ok (.term factorEntity) := by
  decide

/-- A runtime node cannot be read through an uninstantiated blueprint. -/
theorem blueprint_runtime_node_is_rejected :
    resolve libraryEnvironment
      (.term (modelHead "model" "result") .blueprintGraphRead) =
      .error (.invalidTermUse .blueprintGraphRead resultEntity) := by
  decide

/-- Distinct aliases of one canonical DAG resolve a member to one identity. -/
theorem dag_aliases_preserve_member_identity :
    resolveHead libraryEnvironment .term (modelHead "model" "FACTOR") =
      resolveHead libraryEnvironment .term (modelHead "facade" "FACTOR") := by
  decide

private def configuredAlias : Binding := {
  scope := .dag rootDag
  name := "configured"
  entity := .term {
    id := termId rootDag 10
    kind := .includeInstanceAlias instanceId
  }
}

private def configuredResult : TermEntity := {
  id := termId libraryDag 2
  kind := .node
}

private def configuredEnvironment : Environment := [
  configuredAlias,
  {
    scope := .instance instanceId
    name := "result"
    entity := .term configuredResult
  }
]

/-- A configured include alias exposes runtime instance members. -/
theorem configured_instance_result_is_readable :
    resolve configuredEnvironment
      (.term
        (.member rootDag { root := "configured", children := [] } "result")
        .instanceGraphRead) =
      .ok (.term configuredResult) := by
  decide

private def massNode : Binding := {
  scope := .dag rootDag
  name := "mass"
  entity := .term {
    id := termId rootDag 11
    kind := .node
  }
}

private def termMassBinder : BinderRequest := {
  kind := .term
  scope := .lexical localScope
  visibleScopes := [.dag rootDag]
  name := "mass"
}

private def staticMassBinder : BinderRequest := {
  kind := .static
  scope := .lexical localScope
  visibleScopes := [.dag rootDag]
  name := "mass"
}

/-- A local Term binder cannot shadow a visible flat Term of any category. -/
theorem local_term_shadowing_is_rejected :
    validateBinder [massNode] termMassBinder =
      .error (.visibleNameOccupied {
        scopes := [.lexical localScope, .dag rootDag]
        space := .term
        name := "mass"
      }) := by
  decide

/-- The same spelling remains valid for a Static binder. -/
theorem cross_namespace_binder_reuse_is_accepted :
    validateBinder [massNode] staticMassBinder = .ok () := by
  decide

/-- Argument shape is observationally irrelevant to callee resolution. -/
theorem named_and_positional_calls_choose_the_same_callee
    (environment : Environment)
    (callee : NameHead) :
    resolve environment (.call callee .named) =
      resolve environment (.call callee .positional) :=
  call_argument_shape_irrelevant environment callee .named .positional

end Graphcal.Static.NamespaceResolution.Examples
