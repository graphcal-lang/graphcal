import Graphcal.Static.Interface

namespace Graphcal.Static.TemplateClosure

/-- Shared Static input categories; template closure does not maintain its own copy. -/
abbrev StaticInputKind := Interface.StaticInputKind

/-- Shared validated Static roles. -/
abbrev StaticRole := Interface.StaticRole

/-- The two places where a Static default may be observed. -/
inductive UseContext where
  | parameterDefault
  | templateBody
  deriving DecidableEq, Repr

/-- Whether a use is parametric or relies on a port's concrete default definition. -/
inductive Dependency where
  | abstract
  | defaultDefinition
  deriving DecidableEq, Repr

/-- One semantic template-closure decision. -/
structure Check where
  kind : StaticInputKind
  role : StaticRole
  context : UseContext
  dependency : Dependency
  deriving DecidableEq, Repr

/-- Typed identity of the template-closure diagnostic. -/
inductive Violation where
  | templateBodyDependsOnDefault (kind : StaticInputKind)
  deriving DecidableEq, Repr

end Graphcal.Static.TemplateClosure
