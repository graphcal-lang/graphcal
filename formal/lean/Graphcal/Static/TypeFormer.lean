import Graphcal.Static.Kinding

namespace Graphcal.Static

/--
Untyped semantic type-level former expressions. Unlike the intrinsically typed
semantic representations, this syntax can express malformed applications, so
its declarative kinding relation carries substantive rejection information.
-/
inductive TypeLevelExpr where
  | dimension (value : Dimension)
  | timeScale (value : TimeScale)
  | nat (value : Nat)
  | index (value : Index)
  | int
  | bool
  | quantity (argument : TypeLevelExpr)
  | complex (argument : TypeLevelExpr)
  | key (argument : TypeLevelExpr)
  | datetime (argument : TypeLevelExpr)
  | fin (cardinality : Nat)

/-- Declarative kinding rules for Graphcal's built-in type-level formers. -/
inductive HasTypeLevelKind : TypeLevelExpr → Kind → Prop where
  | dimension (value : Dimension) : HasTypeLevelKind (.dimension value) .dim
  | timeScale (value : TimeScale) : HasTypeLevelKind (.timeScale value) .timeScale
  | nat (value : Nat) : HasTypeLevelKind (.nat value) .nat
  | index (value : Index) : HasTypeLevelKind (.index value) .index
  | int : HasTypeLevelKind .int .valueType
  | bool : HasTypeLevelKind .bool .valueType
  | quantity {argument : TypeLevelExpr} :
      HasTypeLevelKind argument .dim →
      HasTypeLevelKind (.quantity argument) .valueType
  | complex {argument : TypeLevelExpr} :
      HasTypeLevelKind argument .dim →
      HasTypeLevelKind (.complex argument) .valueType
  | key {argument : TypeLevelExpr} :
      HasTypeLevelKind argument .index →
      HasTypeLevelKind (.key argument) .valueType
  | datetime {argument : TypeLevelExpr} :
      HasTypeLevelKind argument .timeScale →
      HasTypeLevelKind (.datetime argument) .valueType
  | fin {cardinality : Nat} :
      0 < cardinality → HasTypeLevelKind (.fin cardinality) .index

namespace HasTypeLevelKind

/-- An untyped type-level expression has at most one kind. -/
theorem unique {expression : TypeLevelExpr} {left right : Kind}
    (leftDerivation : HasTypeLevelKind expression left)
    (rightDerivation : HasTypeLevelKind expression right) : left = right := by
  cases leftDerivation <;> cases rightDerivation <;> rfl

/-- The malformed semantic application `Complex(Fin(n))` has no kind. -/
theorem complex_fin_unclassified (cardinality : Nat) (kind : Kind) :
    ¬HasTypeLevelKind (.complex (.fin cardinality)) kind := by
  intro derivation
  cases derivation with
  | complex argumentDerivation => cases argumentDerivation

/-- The malformed semantic application `Key(n)` has no kind. -/
theorem key_nat_unclassified (value : Nat) (kind : Kind) :
    ¬HasTypeLevelKind (.key (.nat value)) kind := by
  intro derivation
  cases derivation with
  | key argumentDerivation => cases argumentDerivation

/-- `Fin(0)` cannot be classified as an index or any other kind. -/
theorem fin_zero_unclassified (kind : Kind) :
    ¬HasTypeLevelKind (.fin 0) kind := by
  intro derivation
  cases derivation with
  | fin positive => exact (Nat.lt_irrefl 0) positive

end HasTypeLevelKind

end Graphcal.Static
