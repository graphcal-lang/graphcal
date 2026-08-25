import Graphcal.Static.RequiredBindability.Model

namespace Graphcal.Static.RequiredBindability

/-- A declaration is required exactly when it has no definition or default. -/
def Required : InterfaceDecl → Prop
  | .param .required
  | .nominal _ _ .required => True
  | _ => False

/--
Parameters are bindable because they are input ports. A nominal declaration is
bindable only through an explicit `pub(bind)` annotation.
-/
def Bindable : InterfaceDecl → Prop
  | .param _ => True
  | .nominal _ .exportedBindable _ => True
  | _ => False

/--
V002's normative rule: every declaration that needs an external definition
must provide a legal binding path. Optional declarations impose no obligation.
-/
def WellFormed (decl : InterfaceDecl) : Prop :=
  Required decl → Bindable decl

/--
Connects a typed rejection to the state that justifies it. This relation lets
us prove that the executable validator cannot report V002 for a parameter,
for a declaration with a definition, or for a bindable nominal declaration.
-/
def Violation.AppliesTo : Violation → InterfaceDecl → Prop
  | .requiredMustBeBindable expectedKind,
      .nominal actualKind visibility .required =>
        actualKind = expectedKind ∧ visibility ≠ .exportedBindable
  | _, _ => False

/--
The following lemmas characterize the normative `WellFormed` predicate itself.
They intentionally do not refer to `validate`; they are specification-only
lemmas, not proofs about the executable reference validator.
-/

/-- A required nominal declaration is well-formed exactly when it is bindable. -/
theorem required_nominal_wellFormed_iff
    (kind : NominalKind)
    (visibility : Visibility) :
    WellFormed (.nominal kind visibility .required) ↔
      visibility = .exportedBindable := by
  cases visibility <;> simp [WellFormed, Required, Bindable]

/-- Every nominal declaration with a definition is valid under V002. -/
theorem defaulted_nominal_wellFormed
    (kind : NominalKind)
    (visibility : Visibility) :
    WellFormed (.nominal kind visibility .defaulted) := by
  simp [WellFormed, Required]

/-- Both required and defaulted parameters are valid because they are ports. -/
theorem param_wellFormed (requirement : Requirement) :
    WellFormed (.param requirement) := by
  cases requirement <;> simp [WellFormed, Required, Bindable]

end Graphcal.Static.RequiredBindability
