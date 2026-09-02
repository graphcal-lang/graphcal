import Lean.Data.Json
import Graphcal.Static.TemplateClosure.Reference

namespace Graphcal.Static.TemplateClosure.Oracle

open Lean

structure WireCheck where
  kind : String
  role : String
  context : String
  dependency : String
  deriving ToJson

inductive WireDecision where
  | accepted
  | rejected (rule : String) (kind : String)
  deriving ToJson

structure WireCase where
  check : WireCheck
  decision : WireDecision
  deriving ToJson

def kindWireName : StaticInputKind → String
  | .nominalType => "type"
  | .dimension => "dimension"
  | .index => "index"

def roleWireName : StaticRole → String
  | .fixed => "fixed"
  | .optionalInput => "optionalInput"
  | .requiredInput => "requiredInput"

def contextWireName : UseContext → String
  | .parameterDefault => "parameterDefault"
  | .templateBody => "templateBody"

def dependencyWireName : Dependency → String
  | .abstract => "abstract"
  | .defaultDefinition => "defaultDefinition"

def checkToWire (check : Check) : WireCheck := {
  kind := kindWireName check.kind
  role := roleWireName check.role
  context := contextWireName check.context
  dependency := dependencyWireName check.dependency
}

def decisionFor (check : Check) : WireDecision :=
  match validate check with
  | .ok () => .accepted
  | .error (.templateBodyDependsOnDefault kind) =>
      .rejected "templateBodyDependsOnDefault" (kindWireName kind)

def kinds : List StaticInputKind := [.nominalType, .dimension, .index]
def roles : List StaticRole := [.fixed, .optionalInput, .requiredInput]
def contexts : List UseContext := [.parameterDefault, .templateBody]
def dependencies : List Dependency := [.abstract, .defaultDefinition]

/-- Exhaustive 3 × 3 × 2 × 2 semantic state matrix. -/
def allChecks : List Check :=
  kinds.flatMap fun kind =>
    roles.flatMap fun role =>
      contexts.flatMap fun context =>
        dependencies.map fun dependency => ⟨kind, role, context, dependency⟩

theorem allChecks_complete (check : Check) : check ∈ allChecks := by
  cases check with
  | mk kind role context dependency =>
      cases kind <;> cases role <;> cases context <;> cases dependency <;>
        simp [allChecks, kinds, roles, contexts, dependencies]

theorem allChecks_count : allChecks.length = 36 := by
  rfl

def cases : List WireCase :=
  allChecks.map fun check => {
    check := checkToWire check
    decision := decisionFor check
  }

end Graphcal.Static.TemplateClosure.Oracle

def main : IO Unit :=
  IO.println <| Lean.toJson Graphcal.Static.TemplateClosure.Oracle.cases |>.compress
