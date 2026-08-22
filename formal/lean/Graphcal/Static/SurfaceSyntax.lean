import Graphcal.Static.DeclType

namespace Graphcal.Static

/--
Resolved source type syntax before contextual dimension notation is elaborated.
The `dimension` constructor corresponds to `hir::TypeExprKind::DimExpr`.
-/
inductive ValueTypeSyntax where
  | dimension (expression : DimensionExpr)
  | complex (dimension : DimensionExpr)
  | key (index : IndexSyntax)
  | int
  | bool
  | datetime (scale : TimeScale)
  | resolvedNominal (type : ValueType)

/-- The syntactic distinction between no axes and explicit indexed syntax. -/
inductive AxesSyntax where
  | scalar
  | indexed (axes : List IndexSyntax)

/-- Resolved declaration-type syntax for the initial unconstrained fragment. -/
structure DeclTypeSyntax where
  element : ValueTypeSyntax
  axes : AxesSyntax

/--
Resolved generic-argument syntax before its expected generic kind is applied.
Dimension syntax is shared by `Dim` and `Type` positions intentionally.
-/
inductive GenericArgumentSyntax where
  | dimension (expression : DimensionExpr)
  | valueType (type : ValueTypeSyntax)
  | index (axis : IndexSyntax)
  | nat (value : NatExpr)

end Graphcal.Static
