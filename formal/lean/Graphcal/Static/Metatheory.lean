import Graphcal.Static.Elaboration
import Graphcal.Static.Generic
import Graphcal.Static.TypeFormer

namespace Graphcal.Static

private def one : PositiveNat := ⟨1, by decide⟩

/--
Every semantic kind has a concrete inhabitant in the model. Combined with
`HasKind.unique`, this witnesses that the classification rules describe
separate, nonempty domains rather than contradictory empty specifications.
-/
theorem semantic_kind_model_nonempty (kind : Kind) :
    ∃ entity : ResolvedEntity, HasKind entity kind := by
  cases kind with
  | dim =>
      exact ⟨.dimension .dimensionless, .dimension .dimensionless⟩
  | timeScale =>
      exact ⟨.timeScale .utc, .timeScale .utc⟩
  | valueType =>
      exact ⟨.valueType .bool, .valueType .bool⟩
  | index =>
      exact ⟨.index (.fin one), .index (.fin one)⟩
  | nat =>
      exact ⟨.nat 0, .nat 0⟩

/-- `Quantity` maps every semantic dimension into the `Type` kind. -/
theorem quantity_former_kind_correct (dimension : Dimension) :
    HasKind (.valueType (.quantity dimension)) .valueType :=
  .valueType (.quantity dimension)

/-- `Complex` maps every semantic dimension into the `Type` kind. -/
theorem complex_former_kind_correct (dimension : Dimension) :
    HasKind (.valueType (.complex dimension)) .valueType :=
  .valueType (.complex dimension)

/-- `Key` maps every valid semantic index into the `Type` kind. -/
theorem key_former_kind_correct (index : Index) :
    HasKind (.valueType (.key index)) .valueType :=
  .valueType (.key index)

end Graphcal.Static
