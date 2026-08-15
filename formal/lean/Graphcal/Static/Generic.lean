import Graphcal.Static.ValueType

namespace Graphcal.Static

/-- A de Bruijn variable whose type index records its declared generic kind. -/
inductive GenericVar : List GenericKind → GenericKind → Type where
  | here : GenericVar (kind :: context) kind
  | there : GenericVar context kind → GenericVar (other :: context) kind

/--
A small intrinsically kinded language of generic type-level expressions. The
constructors encode the signatures of Graphcal's type formers.
-/
inductive GenericExpr (context : List GenericKind) : GenericKind → Type where
  | var (entry : GenericVar context kind) : GenericExpr context kind
  | dimension (value : Dimension) : GenericExpr context .dim
  | valueType (value : ValueType) : GenericExpr context .valueType
  | index (value : Index) : GenericExpr context .index
  | nat (value : Nat) : GenericExpr context .nat
  | quantity (dimension : GenericExpr context .dim) : GenericExpr context .valueType
  | complex (dimension : GenericExpr context .dim) : GenericExpr context .valueType
  | key (index : GenericExpr context .index) : GenericExpr context .valueType
  | natAdd
      (left right : GenericExpr context .nat) : GenericExpr context .nat
  | natMultiply
      (left right : GenericExpr context .nat) : GenericExpr context .nat

namespace GenericExpr

/--
Substitute an argument for the newest generic parameter. The result kind is
preserved in the function's return type, so a cross-kind substitution cannot
be implemented.
-/
def substituteTop
    {context : List GenericKind}
    {parameter result : GenericKind}
    (argument : GenericExpr context parameter) :
    GenericExpr (parameter :: context) result → GenericExpr context result
  | .var .here => argument
  | .var (.there entry) => .var entry
  | .dimension value => .dimension value
  | .valueType value => .valueType value
  | .index value => .index value
  | .nat value => .nat value
  | .quantity inner => .quantity (substituteTop argument inner)
  | .complex inner => .complex (substituteTop argument inner)
  | .key inner => .key (substituteTop argument inner)
  | .natAdd left right =>
      .natAdd (substituteTop argument left) (substituteTop argument right)
  | .natMultiply left right =>
      .natMultiply (substituteTop argument left) (substituteTop argument right)

/-- Existential packaging used only to state kind preservation as an equality. -/
structure SomeGenericExpr (context : List GenericKind) where
  kind : GenericKind
  expression : GenericExpr context kind

/-- Package an intrinsically kinded expression. -/
def SomeGenericExpr.of {kind : GenericKind}
    (expression : GenericExpr context kind) : SomeGenericExpr context :=
  ⟨kind, expression⟩

/-- Generic substitution leaves the body's result kind unchanged. -/
theorem substituteTop_preserves_kind
    {context : List GenericKind}
    {parameter result : GenericKind}
    (argument : GenericExpr context parameter)
    (body : GenericExpr (parameter :: context) result) :
    (SomeGenericExpr.of (substituteTop argument body)).kind = result := by
  rfl

@[simp]
theorem substituteTop_here
    (argument : GenericExpr context parameter) :
    substituteTop argument (.var (.here : GenericVar (parameter :: context) parameter)) =
      argument := by
  simp [substituteTop]

end GenericExpr

end Graphcal.Static
