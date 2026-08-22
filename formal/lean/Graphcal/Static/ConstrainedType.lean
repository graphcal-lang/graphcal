import Graphcal.Static.ValueType

namespace Graphcal.Static

/--
A statically known bound value indexed by the value type it can constrain.
There are deliberately no constructors for Boolean, complex, key, or nominal
types.
-/
inductive Bound : ValueType → Type where
  | quantity {dimension : Dimension} (value : Rat) : Bound (.quantity dimension)
  | int (value : Int) : Bound .int
  | datetime {scale : TimeScale} (instant : Int) : Bound (.datetime scale)

/-- At least one inclusive lower or upper bound on a particular value type. -/
inductive NonEmptyBounds (type : ValueType) where
  | lower (value : Bound type)
  | upper (value : Bound type)
  | both (lower upper : Bound type)

/-- A value type with either no bounds or a well-typed nonempty bound set. -/
inductive ConstrainedType where
  | unconstrained (base : ValueType)
  | constrained (base : ValueType) (bounds : NonEmptyBounds base)

namespace ConstrainedType

/-- Recover the underlying single-value type. -/
def base : ConstrainedType → ValueType
  | .unconstrained type => type
  | .constrained type _ => type

end ConstrainedType

/-- A Boolean bound is unrepresentable in the semantic core. -/
theorem no_bool_bound (bound : Bound .bool) : False := by
  cases bound

/-- Consequently, a Boolean cannot carry a nonempty constraint set. -/
theorem no_constrained_bool (bounds : NonEmptyBounds .bool) : False := by
  cases bounds with
  | lower value => exact no_bool_bound value
  | upper value => exact no_bool_bound value
  | both lower _ => exact no_bool_bound lower

end Graphcal.Static
