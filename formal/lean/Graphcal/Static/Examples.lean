import Graphcal.Static.Metatheory

namespace Graphcal.Static.Examples

open Graphcal.Static

private def lengthBase : BaseDimension :=
  .prelude .length

private def timeBase : BaseDimension :=
  .prelude .time

private def lengthSyntax : DimensionExpr :=
  .base lengthBase

private def timeSyntax : DimensionExpr :=
  .base timeBase

private def length : Dimension := lengthSyntax.interpret

private def time : Dimension := timeSyntax.interpret

/-- The declarative former rules derive `Quantity(Length) : Type`. -/
example :
    HasTypeLevelKind
      (.quantity (.dimension length))
      .valueType :=
  .quantity (.dimension length)

/-- The declarative former rules derive `Key(Fin(3)) : Type`. -/
example :
    HasTypeLevelKind
      (.key (.fin 3))
      .valueType :=
  .key (.fin (by decide))

/-- A `Complex` former cannot consume an index argument. -/
example :
    ¬HasTypeLevelKind (.complex (.fin 3)) .valueType :=
  HasTypeLevelKind.complex_fin_unclassified 3 .valueType

/-- `Length / Length` is semantically dimensionless. -/
example :
    DimensionExpr.Equivalent
      (.divide lengthSyntax lengthSyntax)
      .dimensionless := by
  exact Dimension.divide_self length

/-- `(Length / Time) * Time` is semantically `Length`. -/
example :
    DimensionExpr.Equivalent
      (.multiply (.divide lengthSyntax timeSyntax) timeSyntax)
      lengthSyntax := by
  apply Dimension.ext
  intro candidate
  exact Rat.sub_add_cancel

/-- In an annotation, source `Length` means semantic `Quantity(Length)`. -/
example :
    elaborateValueType (.dimension lengthSyntax) =
      .ok (.quantity length) := by
  rfl

/-- The same syntax remains a dimension when a generic expects `Dim`. -/
example :
    elaborateGenericArgument .dim (.dimension lengthSyntax) =
      .ok (.dim length) := by
  rfl

/-- The same syntax becomes a quantity when a generic expects `Type`. -/
example :
    elaborateGenericArgument .valueType (.dimension lengthSyntax) =
      .ok (.valueType (.quantity length)) := by
  rfl

/-- An index argument cannot be supplied to a `Nat` generic parameter. -/
example :
    elaborateGenericArgument .nat (.index (.fin (.literal 3))) =
      .error (.wrongGenericKind .nat .index) := by
  rfl

/-- `Fin(0)` is rejected before a semantic `Index` can be constructed. -/
example :
    IndexSyntax.elaborate (.fin (.literal 0)) =
      .error (.nonPositiveFin 0) := by
  rfl

private def three : PositiveNat := ⟨3, by decide⟩

private def vectorConstructor : NominalTypeConstructor :=
  { identity := { owner := { serial := 0 }, ordinal := 0 }
    parameters := [.nat, .dim] }

/-- `Vector<3, Length>` is constructible because both arguments match. -/
private def vector3Length : ValueType :=
  .nominal vectorConstructor
    (.cons (.nat 3) (.cons (.dim length) .nil))

example : HasKind (.valueType vector3Length) .valueType :=
  .valueType vector3Length

/-- An indexed declaration always carries at least one semantic axis. -/
example :
    elaborateDeclType
      { element := .dimension lengthSyntax
        axes := .indexed [.fin (.literal 3)] } =
      .ok
        (.indexed
          (.unconstrained (.quantity length))
          ⟨.fin three, []⟩) := by
  rfl

/-- Quantity bounds carry the dimension in their Lean type. -/
private def nonnegativeLength : ConstrainedType :=
  .constrained (.quantity length)
    (.lower (.quantity 0))

example : nonnegativeLength.base = .quantity length := by
  rfl

end Graphcal.Static.Examples
