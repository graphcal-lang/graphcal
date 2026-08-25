namespace Graphcal.Static.RequiredBindability

/--
The nominal declaration kinds whose missing definitions may be supplied by an
`include` binding. Keeping this set closed makes the formal rule explicitly
exclude declarations such as nodes and units, which can never be bindable.
-/
inductive NominalKind where
  | index
  | type
  | dimension
  deriving DecidableEq, Repr

/--
The three semantic states represented by no annotation, `pub`, and `pub(bind)`.
Only `exportedBindable` promises that an importer may supply a replacement.
-/
inductive Visibility where
  | local
  | exported
  | exportedBindable
  deriving DecidableEq, Repr

/--
Whether a declaration already supplies a definition/default or requires one
from outside the declaring module.
-/
inductive Requirement where
  | defaulted
  | required
  deriving DecidableEq, Repr

/--
The external-interface declarations relevant to V002.

A parameter is represented separately because its declaration kind creates an
input port; it does not secretly carry `pub(bind)`. Nominal declarations use
visibility annotations to opt into binding. This distinction prevents the
formal model from erasing the different reasons that both forms are bindable.
-/
inductive InterfaceDecl where
  | param (requirement : Requirement)
  | nominal
      (kind : NominalKind)
      (visibility : Visibility)
      (requirement : Requirement)
  deriving DecidableEq, Repr

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
Typed identity of the V002 failure. Names and source spans are intentionally
absent: they belong to diagnostics, while this formal core records only the
semantic reason for rejection.
-/
inductive Violation where
  | requiredMustBeBindable (kind : NominalKind)
  deriving DecidableEq, Repr

/--
The executable reference validator. Its cases mirror the reviewable state
matrix: only required nominal declarations without `pub(bind)` are rejected.
-/
def validate : InterfaceDecl → Except Violation Unit
  | .param _ => .ok ()
  | .nominal _ _ .defaulted => .ok ()
  | .nominal _ .exportedBindable .required => .ok ()
  | .nominal kind .local .required =>
      .error (.requiredMustBeBindable kind)
  | .nominal kind .exported .required =>
      .error (.requiredMustBeBindable kind)

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
Acceptance is exactly the normative rule—not merely one direction. The reverse
direction is important because a validator that rejected everything would be
sound in the weak sense but unusable.
-/
theorem validate_accepts_iff_wellFormed (decl : InterfaceDecl) :
    validate decl = .ok () ↔ WellFormed decl := by
  cases decl with
  | param requirement =>
      cases requirement <;> simp [validate, WellFormed, Required, Bindable]
  | nominal kind visibility requirement =>
      cases visibility <;> cases requirement <;>
        simp [validate, WellFormed, Required, Bindable]

/-- Every executable rejection identifies a violation that really applies. -/
theorem validate_error_sound
    (decl : InterfaceDecl)
    (violation : Violation)
    (rejected : validate decl = .error violation) :
    violation.AppliesTo decl := by
  cases decl with
  | param requirement =>
      cases requirement <;> cases violation <;>
        simp_all [validate]
  | nominal kind visibility requirement =>
      cases visibility <;> cases requirement <;> cases violation <;>
        simp_all [validate, Violation.AppliesTo]

/--
A required nominal declaration satisfies V002 precisely when it is explicitly
bindable. This theorem is the required row of the state matrix in one line.
-/
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
