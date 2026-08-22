import Std

namespace Graphcal.Static

/-- Prelude base dimensions: the seven SI bases plus plane angle. -/
inductive PreludeBaseDimension where
  | length
  | time
  | mass
  | temperature
  | electricCurrent
  | amount
  | luminousIntensity
  | angle
  deriving DecidableEq, Repr

/-- Opaque, typed identity of a resolved Graphcal module. -/
structure ModuleId where
  serial : Nat
  deriving DecidableEq, Repr

/-- Owner-qualified identity of a user-defined base dimension. -/
structure DimensionDeclId where
  owner : ModuleId
  ordinal : Nat
  deriving DecidableEq, Repr

/-- Canonical identity of a base dimension. -/
inductive BaseDimension where
  | prelude (dimension : PreludeBaseDimension)
  | userDefined (declaration : DimensionDeclId)
  deriving DecidableEq, Repr

/--
The extensional semantic model of a dimension: every base dimension is mapped
to its rational exponent. Dimension expressions denote values of this type.

The production compiler stores only the finite nonzero support. This
extensional representation is intentionally simpler for the initial semantic
model; expressions generated below still mention only finitely many bases.
-/
structure Dimension where
  exponent : BaseDimension → Rat

namespace Dimension

/-- The identity dimension, whose every base exponent is zero. -/
def dimensionless : Dimension := ⟨fun _ => 0⟩

/-- The dimension consisting of one canonical base to the first power. -/
def base (target : BaseDimension) : Dimension :=
  ⟨fun candidate => if candidate = target then 1 else 0⟩

/-- Dimension multiplication adds corresponding base exponents. -/
def multiply (left right : Dimension) : Dimension :=
  ⟨fun candidate => left.exponent candidate + right.exponent candidate⟩

/-- Dimension division subtracts corresponding base exponents. -/
def divide (left right : Dimension) : Dimension :=
  ⟨fun candidate => left.exponent candidate - right.exponent candidate⟩

/-- Raising a dimension to a rational power scales every base exponent. -/
def pow (dimension : Dimension) (power : Rat) : Dimension :=
  ⟨fun candidate => dimension.exponent candidate * power⟩

/-- Dimensions are equal when all canonical base exponents are equal. -/
@[ext]
theorem ext {left right : Dimension}
    (equal : ∀ candidate, left.exponent candidate = right.exponent candidate) :
    left = right := by
  cases left with
  | mk leftExponent =>
      cases right with
      | mk rightExponent =>
          congr
          funext candidate
          exact equal candidate

@[simp]
theorem dimensionless_exponent (candidate : BaseDimension) :
    dimensionless.exponent candidate = 0 := rfl

@[simp]
theorem base_same_exponent (target : BaseDimension) :
    (base target).exponent target = 1 := by
  simp [base]

@[simp]
theorem multiply_dimensionless_left (dimension : Dimension) :
    multiply dimensionless dimension = dimension := by
  apply ext
  intro candidate
  exact Rat.zero_add _

@[simp]
theorem multiply_dimensionless_right (dimension : Dimension) :
    multiply dimension dimensionless = dimension := by
  apply ext
  intro candidate
  exact Rat.add_zero _

/-- Dimension multiplication is associative. -/
theorem multiply_assoc (first second third : Dimension) :
    multiply (multiply first second) third = multiply first (multiply second third) := by
  apply ext
  intro candidate
  exact Rat.add_assoc _ _ _

/-- Dimension multiplication is commutative. -/
theorem multiply_comm (left right : Dimension) :
    multiply left right = multiply right left := by
  apply ext
  intro candidate
  exact Rat.add_comm _ _

@[simp]
theorem divide_self (dimension : Dimension) :
    divide dimension dimension = dimensionless := by
  apply ext
  intro candidate
  exact Rat.sub_self

@[simp]
theorem pow_zero (dimension : Dimension) :
    pow dimension 0 = dimensionless := by
  apply ext
  intro candidate
  exact Rat.mul_zero _

@[simp]
theorem pow_one (dimension : Dimension) :
    pow dimension 1 = dimension := by
  apply ext
  intro candidate
  exact Rat.mul_one _

end Dimension

/-- Resolved dimension syntax before interpretation into exponent vectors. -/
inductive DimensionExpr where
  | dimensionless
  | base (dimension : BaseDimension)
  | multiply (left right : DimensionExpr)
  | divide (left right : DimensionExpr)
  | pow (dimension : DimensionExpr) (power : Rat)
  deriving Repr

namespace DimensionExpr

/-- Interpret resolved dimension syntax in the extensional semantic model. -/
def interpret : DimensionExpr → Dimension
  | .dimensionless => .dimensionless
  | .base dimension => .base dimension
  | .multiply left right => .multiply left.interpret right.interpret
  | .divide left right => .divide left.interpret right.interpret
  | .pow dimension power => .pow dimension.interpret power

/-- Semantic equivalence of dimension expressions. -/
def Equivalent (left right : DimensionExpr) : Prop :=
  left.interpret = right.interpret

end DimensionExpr

end Graphcal.Static
