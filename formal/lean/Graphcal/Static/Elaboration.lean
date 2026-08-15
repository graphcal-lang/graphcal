import Graphcal.Static.SurfaceSyntax
import Graphcal.Static.TypeFormer

namespace Graphcal.Static

/-- The top-level syntactic category supplied to generic-argument elaboration. -/
inductive GenericSyntaxCategory where
  | dimension
  | valueType
  | index
  | nat
  deriving DecidableEq, Repr

namespace GenericArgumentSyntax

/-- Classify source syntax without assigning it a Graphcal semantic kind. -/
def category : GenericArgumentSyntax → GenericSyntaxCategory
  | .dimension _ => .dimension
  | .valueType _ => .valueType
  | .index _ => .index
  | .nat _ => .nat

end GenericArgumentSyntax

inductive ElaborationError where
  | invalidIndex (cause : IndexElaborationError)
  | emptyIndexList
  | wrongGenericKind
      (expected : GenericKind)
      (found : GenericSyntaxCategory)
  deriving DecidableEq, Repr

private def elaborateIndex (source : IndexSyntax) : Except ElaborationError Index :=
  match source.elaborate with
  | .ok index => .ok index
  | .error cause => .error (.invalidIndex cause)

private def elaborateIndexList : List IndexSyntax → Except ElaborationError (List Index)
  | [] => .ok []
  | source :: remaining => do
      let index ← elaborateIndex source
      let indexes ← elaborateIndexList remaining
      pure (index :: indexes)

/--
Elaborate a source value-type position. A dimension expression is contextual
surface notation for the semantic former `Quantity : Dim -> Type`.
-/
def elaborateValueType : ValueTypeSyntax → Except ElaborationError ValueType
  | .dimension expression => .ok (.quantity expression.interpret)
  | .complex dimension => .ok (.complex dimension.interpret)
  | .key index => do
      let resolved ← elaborateIndex index
      pure (.key resolved)
  | .int => .ok .int
  | .bool => .ok .bool
  | .datetime scale => .ok (.datetime scale)
  | .resolvedNominal type => .ok type

/-- Elaborate the initial unconstrained declaration-type fragment. -/
def elaborateDeclType (source : DeclTypeSyntax) : Except ElaborationError DeclType := do
  let valueType ← elaborateValueType source.element
  let element := ConstrainedType.unconstrained valueType
  match source.axes with
  | .scalar => pure (.scalar element)
  | .indexed [] => .error .emptyIndexList
  | .indexed (first :: remaining) => do
      let head ← elaborateIndex first
      let tail ← elaborateIndexList remaining
      pure (.indexed element ⟨head, tail⟩)

/--
Elaborate one generic argument under the generic parameter's expected kind.
The shared dimension syntax has two deliberate interpretations:

* expected `Dim`  -> a dimension argument;
* expected `Type` -> a `Quantity` value-type argument.
-/
def elaborateGenericArgument
    (expected : GenericKind)
    (source : GenericArgumentSyntax) :
    Except ElaborationError (GenericArgument expected) :=
  match expected, source with
  | .dim, .dimension expression => .ok (.dim expression.interpret)
  | .valueType, .dimension expression =>
      .ok (.valueType (.quantity expression.interpret))
  | .valueType, .valueType type => do
      let elaborated ← elaborateValueType type
      pure (.valueType elaborated)
  | .index, .index axis => do
      let elaborated ← elaborateIndex axis
      pure (.index elaborated)
  | .nat, .nat value => .ok (.nat value.interpret)
  | expected, source => .error (.wrongGenericKind expected source.category)

/-- The contextual quantity notation elaborates exactly to `Quantity(D)`. -/
@[simp]
theorem elaborateValueType_dimension (expression : DimensionExpr) :
    elaborateValueType (.dimension expression) =
      .ok (.quantity expression.interpret) := by
  rfl

/-- Contextual quantity elaboration is licensed by the declarative former rules. -/
theorem elaborateValueType_dimension_has_declarative_kind
    (expression : DimensionExpr) :
    HasTypeLevelKind
      (.quantity (.dimension expression.interpret))
      .valueType :=
  .quantity (.dimension expression.interpret)

/-- In a `Dim` generic position, dimension syntax remains a dimension. -/
@[simp]
theorem elaborateGenericArgument_dimension (expression : DimensionExpr) :
    elaborateGenericArgument .dim (.dimension expression) =
      .ok (.dim expression.interpret) := by
  rfl

/-- In a `Type` generic position, the same syntax denotes a quantity type. -/
@[simp]
theorem elaborateGenericArgument_quantity (expression : DimensionExpr) :
    elaborateGenericArgument .valueType (.dimension expression) =
      .ok (.valueType (.quantity expression.interpret)) := by
  rfl

/-- Value-type elaboration is deterministic. -/
theorem elaborateValueType_deterministic
    {source : ValueTypeSyntax} {left right : ValueType}
    (leftResult : elaborateValueType source = .ok left)
    (rightResult : elaborateValueType source = .ok right) : left = right := by
  have same : Except.ok left = Except.ok right := leftResult.symm.trans rightResult
  injection same

/-- Generic elaboration is deterministic for a fixed expected kind. -/
theorem elaborateGenericArgument_deterministic
    {expected : GenericKind}
    {source : GenericArgumentSyntax}
    {left right : GenericArgument expected}
    (leftResult : elaborateGenericArgument expected source = .ok left)
    (rightResult : elaborateGenericArgument expected source = .ok right) : left = right := by
  have same : Except.ok left = Except.ok right := leftResult.symm.trans rightResult
  injection same

/-- Successful value-type elaboration produces an entity classified as `Type`. -/
theorem elaborateValueType_preserves_classification
    {source : ValueTypeSyntax} {result : ValueType}
    (_accepted : elaborateValueType source = .ok result) :
    HasKind (.valueType result) .valueType :=
  .valueType result

/-- Explicit indexed syntax cannot elaborate with an empty axis list. -/
@[simp]
theorem elaborateDeclType_empty_axes (expression : DimensionExpr) :
    elaborateDeclType ⟨.dimension expression, .indexed []⟩ =
      .error .emptyIndexList := by
  rfl

end Graphcal.Static
