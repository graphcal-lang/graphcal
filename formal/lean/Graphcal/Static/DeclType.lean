import Graphcal.Static.ConstrainedType

namespace Graphcal.Static

/-- A list whose nonemptiness is structural rather than a caller convention. -/
structure NonEmptyList (α : Type) where
  head : α
  tail : List α

namespace NonEmptyList

/-- Forget the structural witness and obtain an ordinary list. -/
def toList (values : NonEmptyList α) : List α := values.head :: values.tail

@[simp]
theorem toList_ne_nil (values : NonEmptyList α) : values.toList ≠ [] := by
  simp [toList]

end NonEmptyList

/--
The type of a declaration: either one constrained value or a total map over a
nonempty ordered sequence of axes.
-/
inductive DeclType where
  | scalar (element : ConstrainedType)
  | indexed (element : ConstrainedType) (axes : NonEmptyList Index)

namespace DeclType

/-- Recover the constrained element type from either declaration shape. -/
def element : DeclType → ConstrainedType
  | .scalar value => value
  | .indexed value _ => value

end DeclType

end Graphcal.Static
