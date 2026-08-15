import Graphcal.Static.Dimension
import Graphcal.Static.Nat

namespace Graphcal.Static

/-- Owner-qualified identity of a declared index axis. -/
structure IndexDeclId where
  owner : ModuleId
  ordinal : Nat
  deriving DecidableEq, Repr

/--
A semantic Graphcal index is finite, ordered, and nonempty. Every constructor
therefore carries a positive cardinality.
-/
inductive Index where
  | named (declaration : IndexDeclId) (cardinality : PositiveNat)
  | coordinate
      (declaration : IndexDeclId)
      (dimension : Dimension)
      (cardinality : PositiveNat)
  | fin (cardinality : PositiveNat)

/-- Resolved index syntax before cardinality obligations are discharged. -/
inductive IndexSyntax where
  | resolved (index : Index)
  | fin (cardinality : NatExpr)

inductive IndexElaborationError where
  | nonPositiveFin (cardinality : Nat)
  deriving DecidableEq, Repr

namespace IndexSyntax

/-- Validate index syntax and construct an invariant-preserving semantic axis. -/
def elaborate : IndexSyntax → Except IndexElaborationError Index
  | .resolved index => .ok index
  | .fin cardinality =>
      let value := cardinality.interpret
      match PositiveNat.ofNat? value with
      | some positive => .ok (.fin positive)
      | none => .error (.nonPositiveFin value)

/-- `Fin(0)` cannot cross the elaboration boundary into the semantic core. -/
@[simp]
theorem elaborate_fin_zero :
    elaborate (.fin (.literal 0)) = .error (.nonPositiveFin 0) := by
  rfl

end IndexSyntax

end Graphcal.Static
