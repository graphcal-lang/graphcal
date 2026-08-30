import Graphcal.Static.Interface

namespace Graphcal.Static.RequiredBindability

/-- Shared Static input categories; V002 does not maintain its own copy. -/
abbrev NominalKind := Interface.StaticInputKind

/-- Shared source visibility states. -/
abbrev Visibility := Interface.Visibility

/-- Shared definition/input requirement states. -/
abbrev Requirement := Interface.Requirement

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
