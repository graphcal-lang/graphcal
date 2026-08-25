import Lean.Data.Json
import Graphcal.Static.RequiredBindability

namespace Graphcal.Static.RequiredBindability.Oracle

open Lean

/--
The wire declaration preserves the semantic sum: a parameter has no visibility,
while a nominal declaration does. Strings are introduced only here, at the JSON
boundary, so the formal rule itself continues to pattern-match on closed types.
-/
inductive WireDeclaration where
  | param (requirement : String)
  | nominal
      (kind : String)
      (visibility : String)
      (requirement : String)
  deriving ToJson

/-- The oracle reports a typed rule identity rather than diagnostic prose. -/
inductive WireDecision where
  | accepted
  | rejected (rule : String) (kind : String)
  deriving ToJson

/-- One complete input/output observation used by the Rust conformance test. -/
structure WireCase where
  declaration : WireDeclaration
  decision : WireDecision
  deriving ToJson

/-- Stable boundary spelling for the declaration kinds modeled by V002. -/
def nominalKindWireName : NominalKind → String
  | .index => "index"
  | .type => "type"
  | .dimension => "dimension"

/-- Stable boundary spelling for the three visibility capabilities. -/
def visibilityWireName : Visibility → String
  | .local => "local"
  | .exported => "exported"
  | .exportedBindable => "exportedBindable"

/-- Stable boundary spelling for whether an external definition is needed. -/
def requirementWireName : Requirement → String
  | .defaulted => "defaulted"
  | .required => "required"

/-- Convert one semantic declaration without changing its meaning. -/
def declarationToWire : InterfaceDecl → WireDeclaration
  | .param requirement => .param (requirementWireName requirement)
  | .nominal kind visibility requirement =>
      .nominal
        (nominalKindWireName kind)
        (visibilityWireName visibility)
        (requirementWireName requirement)

/-- Convert the executable validator's result to the cross-language schema. -/
def decisionFor (decl : InterfaceDecl) : WireDecision :=
  match validate decl with
  | .ok () => .accepted
  | .error (.requiredMustBeBindable kind) =>
      .rejected "requiredMustBeBindable" (nominalKindWireName kind)

/--
These are the complete finite domains of the first VGD slice. Listing them
explicitly makes adding a new semantic state a reviewed change rather than
silently omitting it from conformance testing.
-/
def nominalKinds : List NominalKind := [.index, .type, .dimension]
def visibilities : List Visibility := [.local, .exported, .exportedBindable]
def requirements : List Requirement := [.defaulted, .required]

/--
Enumerate the two parameter states and all 3 × 3 × 2 nominal states. This is an
exhaustive truth table, not a random sample.
-/
def allDeclarations : List InterfaceDecl :=
  [.param .defaulted, .param .required] ++
    nominalKinds.flatMap fun kind =>
      visibilities.flatMap fun visibility =>
        requirements.map fun requirement =>
          .nominal kind visibility requirement

/--
Every semantic input appears in the oracle table. If a constructor is added to
one of the modeled types without extending the lists above, this proof stops
checking instead of silently leaving the new state untested.
-/
theorem allDeclarations_complete (decl : InterfaceDecl) :
    decl ∈ allDeclarations := by
  cases decl with
  | param requirement =>
      cases requirement <;>
        simp [allDeclarations]
  | nominal kind visibility requirement =>
      cases kind <;> cases visibility <;> cases requirement <;>
        simp [allDeclarations, nominalKinds, visibilities, requirements]

/-- The reviewed finite domain currently contains exactly twenty states. -/
theorem allDeclarations_count : allDeclarations.length = 20 := by
  rfl

/-- Evaluate the reviewed Lean validator for every state in the finite domain. -/
def cases : List WireCase :=
  allDeclarations.map fun declaration => {
    declaration := declarationToWire declaration
    decision := decisionFor declaration
  }

end Graphcal.Static.RequiredBindability.Oracle

/-- Emit one JSON document so a Rust test can compare the production pass. -/
def main : IO Unit :=
  IO.println <| Lean.toJson Graphcal.Static.RequiredBindability.Oracle.cases |>.compress
