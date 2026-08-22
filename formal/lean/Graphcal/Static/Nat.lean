namespace Graphcal.Static

/-- Resolved type-level natural-number syntax in the initial closed fragment. -/
inductive NatExpr where
  | literal (value : Nat)
  | add (left right : NatExpr)
  | multiply (left right : NatExpr)
  deriving DecidableEq, Repr

namespace NatExpr

/-- Interpret a closed type-level Nat expression. -/
def interpret : NatExpr → Nat
  | .literal value => value
  | .add left right => left.interpret + right.interpret
  | .multiply left right => left.interpret * right.interpret

end NatExpr

/-- A natural number proven to be strictly positive. -/
structure PositiveNat where
  value : Nat
  positive : 0 < value

namespace PositiveNat

/-- Validate the positivity obligation at the surface-to-semantic boundary. -/
def ofNat? (value : Nat) : Option PositiveNat :=
  if positive : 0 < value then
    some ⟨value, positive⟩
  else
    none

@[simp]
theorem ofNat?_zero : ofNat? 0 = none := by
  simp [ofNat?]

/-- Successful validation preserves the source cardinality. -/
theorem ofNat?_value {value : Nat} {positive : PositiveNat}
    (accepted : ofNat? value = some positive) : positive.value = value := by
  simp only [ofNat?] at accepted
  split at accepted
  · simpa using (congrArg PositiveNat.value (Option.some.inj accepted)).symm
  · contradiction

end PositiveNat

end Graphcal.Static
