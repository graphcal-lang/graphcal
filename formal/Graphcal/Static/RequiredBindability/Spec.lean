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

end Graphcal.Static.RequiredBindability
