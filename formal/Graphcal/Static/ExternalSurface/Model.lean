import Graphcal.Static.IncludeProjection

namespace Graphcal.Static.ExternalSurface

open Graphcal.Static
open Graphcal.Static.NamespaceResolution

/--
A semantic declaration already admitted to an external surface. Namespace and
visibility checking happen upstream; this layer decides whether the resolved
entity may cross an `import` or `include` boundary.

`staticDependencies` is the upstream-resolved, deterministic transitive closure
of Static dependencies. Keeping entities rather than spellings preserves exact
category, requirement, and canonical identity.
-/
structure ExportedDeclaration where
  name : NameAtom
  entity : Entity
  staticDependencies : List StaticEntity
  deriving DecidableEq, Repr

/-- A canonical key for any semantic entity. -/
inductive EntityId where
  | static (id : StaticId)
  | term (id : TermId)
  | unit (id : UnitId)
  deriving DecidableEq, Repr

/-- Canonical identity selected by an entity constructor. -/
def entityId : Entity → EntityId
  | .static entity => .static entity.id
  | .term entity => .term entity.id
  | .unit entity => .unit entity.id

/--
One required or optional Static input may bind only to a concrete target of the
same exact category. This shared predicate is the single source of truth used by
the normative composition relation and executable binding lookup.
-/
def StaticBindingValid (input target : StaticEntity) : Prop :=
  input.bindableInputKind = target.concreteInputKind ∧
    input.bindableInputKind ≠ none

instance (input target : StaticEntity) : Decidable (StaticBindingValid input target) := by
  unfold StaticBindingValid
  infer_instance

/--
A canonical binding environment for required and optional Static inputs.
Function shape makes one input ID select at most one target by construction;
finite source binding validation belongs to the environment-construction shell.
-/
abbrev StaticBindings := StaticId → Option StaticEntity

/-- One normalized entry in an applicative Static specialization. -/
structure StaticSubstitutionEntry where
  input : StaticId
  target : StaticEntity
  deriving DecidableEq, Repr

/--
Applicative identity of a blueprint-stable entity. The dependency closure fixes
entry order, so runtime include occurrence and source binding order cannot
change this identity.
-/
structure StaticSpecialization where
  declaration : Entity
  substitutions : List StaticSubstitutionEntry
  deriving DecidableEq, Repr

/-- An entity or outcome whose identity genuinely includes one DAG instance. -/
structure InstantiatedProjection where
  declaration : Entity
  instanceId : InstanceId
  substitutions : List StaticSubstitutionEntry
  deriving DecidableEq, Repr

/-- Runtime scale identity of a plain `unit`. -/
structure RuntimeUnitScaleKey where
  declaration : UnitId
  instanceId : InstanceId
  deriving DecidableEq, Repr

/--
Semantic target of an include projection. Every resulting binding records its
instance separately, while only genuinely runtime targets include the instance
in semantic identity.
-/
inductive ProjectionTarget where
  | staticSpecialization (specialization : StaticSpecialization)
  | runtimeTerm (projection : InstantiatedProjection)
  | runtimeUnit
      (key : RuntimeUnitScaleKey)
      (substitutions : List StaticSubstitutionEntry)
  | assertion (projection : InstantiatedProjection)
  | visualization (projection : InstantiatedProjection)
  deriving DecidableEq, Repr

/-- One source-visible include projection and its semantic target. -/
structure ProjectionBinding where
  instanceId : InstanceId
  name : NameAtom
  source : EntityId
  target : ProjectionTarget
  deriving DecidableEq, Repr

end Graphcal.Static.ExternalSurface
