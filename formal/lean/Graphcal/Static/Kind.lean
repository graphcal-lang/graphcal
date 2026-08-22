namespace Graphcal.Static

/--
The kinds in Graphcal's semantic type-level ontology.

These are object-language kinds represented as ordinary Lean data. In
particular, `valueType` is not Lean's `Type` universe.
-/
inductive Kind where
  | dim
  | timeScale
  | valueType
  | index
  | nat
  deriving DecidableEq, Repr

/--
Kinds that a Graphcal generic parameter may declare. `TimeScale` is
intentionally absent: it is a closed semantic domain, not a source-level
generic kind.
-/
inductive GenericKind where
  | dim
  | valueType
  | index
  | nat
  deriving DecidableEq, Repr

namespace GenericKind

/-- Embed a source-level generic kind into the complete semantic kind set. -/
def toKind : GenericKind → Kind
  | .dim => .dim
  | .valueType => .valueType
  | .index => .index
  | .nat => .nat

/-- No legal generic parameter denotes Graphcal's closed `TimeScale` domain. -/
theorem toKind_ne_timeScale (kind : GenericKind) : kind.toKind ≠ .timeScale := by
  cases kind <;> decide

/-- The embedding of generic kinds into semantic kinds is injective. -/
theorem toKind_injective : Function.Injective toKind := by
  intro left right equal
  cases left <;> cases right <;> simp_all [toKind]

end GenericKind

end Graphcal.Static
