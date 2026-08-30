import Graphcal.Static.NamespaceResolution.Model

namespace Graphcal.Static

open Graphcal.Static.NamespaceResolution

/--
The one shared classification of entities that may cross an `include` projection
boundary. Namespace resolution and external-surface construction consume this
value rather than maintaining separate capability tables.

DAGs and namespace aliases deliberately have no projection: dotted DAG paths
address reusable blueprints directly. Constructors are projected as part of
their owning nominal type API and retain that canonical owner.
-/
inductive IncludeProjection where
  | staticDeclaration (entity : StaticEntity)
  | constNode (entity : TermEntity)
  | constructor (entity : TermEntity) (owner : StaticId)
  | runtimeTerm (entity : TermEntity)
  | assertion (entity : TermEntity)
  | visualization (entity : TermEntity)
  | staticUnit (entity : UnitEntity)
  | runtimeUnit (entity : UnitEntity)
  deriving DecidableEq, Repr

/-- Classify one semantic entity at the include-projection boundary. -/
def includeProjection? : Entity → Option IncludeProjection
  | .static entity@⟨_, .nominalType _⟩ => some (.staticDeclaration entity)
  | .static entity@⟨_, .dimension _⟩ => some (.staticDeclaration entity)
  | .static entity@⟨_, .index _⟩ => some (.staticDeclaration entity)
  | .term entity@⟨_, .constNode⟩ => some (.constNode entity)
  | .term entity@⟨_, .constructor owner⟩ => some (.constructor entity owner)
  | .term entity@⟨_, .param⟩ => some (.runtimeTerm entity)
  | .term entity@⟨_, .node⟩ => some (.runtimeTerm entity)
  | .term entity@⟨_, .assertion⟩ => some (.assertion entity)
  | .term entity@⟨_, .visualization⟩ => some (.visualization entity)
  | .unit entity@⟨_, .prelude⟩ => some (.staticUnit entity)
  | .unit entity@⟨_, .base⟩ => some (.staticUnit entity)
  | .unit entity@⟨_, .constScaled⟩ => some (.staticUnit entity)
  | .unit entity@⟨_, .runtimeScaled⟩ => some (.runtimeUnit entity)
  | _ => none

/-- Normative include-projectability, derived only from `includeProjection?`. -/
def IncludeProjectable (entity : Entity) : Prop :=
  includeProjection? entity ≠ none

instance (entity : Entity) : Decidable (IncludeProjectable entity) := by
  unfold IncludeProjectable
  infer_instance

end Graphcal.Static
