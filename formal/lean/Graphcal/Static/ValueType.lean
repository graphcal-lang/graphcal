import Graphcal.Static.Dimension
import Graphcal.Static.Index
import Graphcal.Static.Kind
import Graphcal.Static.TimeScale

namespace Graphcal.Static

/-- Owner-qualified identity of a nominal Graphcal type declaration. -/
structure TypeDeclId where
  owner : ModuleId
  ordinal : Nat
  deriving DecidableEq, Repr

/-- A nominal type identity together with its declared generic signature. -/
structure NominalTypeConstructor where
  identity : TypeDeclId
  parameters : List GenericKind
  deriving Repr

mutual
  /-- Semantic types inhabited by one unindexed Graphcal value. -/
  inductive ValueType where
    | quantity (dimension : Dimension)
    | complex (dimension : Dimension)
    | key (index : Index)
    | int
    | bool
    | datetime (scale : TimeScale)
    | nominal
        (constructor : NominalTypeConstructor)
        (arguments : GenericArguments constructor.parameters)

  /-- A semantic generic argument indexed by its declared generic kind. -/
  inductive GenericArgument : GenericKind → Type where
    | dim (dimension : Dimension) : GenericArgument .dim
    | valueType (type : ValueType) : GenericArgument .valueType
    | index (axis : Index) : GenericArgument .index
    | nat (value : Nat) : GenericArgument .nat

  /--
  Generic arguments whose kind sequence exactly matches a nominal type's
  signature. A cross-kind application has no constructor.
  -/
  inductive GenericArguments : List GenericKind → Type where
    | nil : GenericArguments []
    | cons
        {kind : GenericKind}
        {remaining : List GenericKind}
        (argument : GenericArgument kind)
        (rest : GenericArguments remaining) :
        GenericArguments (kind :: remaining)
end

end Graphcal.Static
