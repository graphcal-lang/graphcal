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

/-- The typed identity of a V002 failure. -/
inductive Violation where
  | requiredMustBeBindable (kind : NominalKind)
  deriving DecidableEq, Repr

end Graphcal.Static.RequiredBindability
