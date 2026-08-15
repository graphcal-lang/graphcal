import Graphcal.Static.DeclType
import Graphcal.Static.Kind

namespace Graphcal.Static

/-- A resolved semantic entity before exposing its kind as a type index. -/
inductive ResolvedEntity where
  | dimension (value : Dimension)
  | timeScale (value : TimeScale)
  | valueType (value : ValueType)
  | index (value : Index)
  | nat (value : Nat)

/-- Declarative classification of resolved semantic entities. -/
inductive HasKind : ResolvedEntity → Kind → Prop where
  | dimension (value : Dimension) : HasKind (.dimension value) .dim
  | timeScale (value : TimeScale) : HasKind (.timeScale value) .timeScale
  | valueType (value : ValueType) : HasKind (.valueType value) .valueType
  | index (value : Index) : HasKind (.index value) .index
  | nat (value : Nat) : HasKind (.nat value) .nat

namespace HasKind

/-- Every resolved semantic entity has exactly one Graphcal kind. -/
theorem unique {entity : ResolvedEntity} {left right : Kind}
    (leftDerivation : HasKind entity left)
    (rightDerivation : HasKind entity right) : left = right := by
  cases leftDerivation <;> cases rightDerivation <;> rfl

/-- A dimension cannot also be classified as an index. -/
theorem dimension_ne_index {entity : ResolvedEntity}
    (dimensionDerivation : HasKind entity .dim)
    (indexDerivation : HasKind entity .index) : False := by
  have impossible : Kind.dim = Kind.index := unique dimensionDerivation indexDerivation
  cases impossible

/-- A type-level Nat cannot also be classified as a runtime value type. -/
theorem nat_ne_valueType {entity : ResolvedEntity}
    (natDerivation : HasKind entity .nat)
    (typeDerivation : HasKind entity .valueType) : False := by
  have impossible : Kind.nat = Kind.valueType := unique natDerivation typeDerivation
  cases impossible

end HasKind

end Graphcal.Static
