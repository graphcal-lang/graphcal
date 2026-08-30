import Graphcal.Static.ExternalSurface.Model

namespace Graphcal.Static.ExternalSurface

open Graphcal.Static
open Graphcal.Static.NamespaceResolution

/-- Typed failures from direct-import capability validation. -/
inductive ImportError where
  | declarationNotImportable (entity : Entity)
  | unresolvedStaticDependency
      (declaration : Entity)
      (dependency : StaticEntity)
  deriving DecidableEq, Repr

/-- Executable category-only import validation. -/
def validateBlueprintImportCategory (entity : Entity) : Except ImportError Unit :=
  match entity with
  | .static ⟨_, .nominalType .fixed⟩ => .ok ()
  | .static ⟨_, .nominalType .optionalInput⟩ => .ok ()
  | .static ⟨_, .dimension .fixed⟩ => .ok ()
  | .static ⟨_, .dimension .optionalInput⟩ => .ok ()
  | .static ⟨_, .index .fixed⟩ => .ok ()
  | .static ⟨_, .index .optionalInput⟩ => .ok ()
  | .term ⟨_, .constNode⟩ => .ok ()
  | .term ⟨_, .constructor _⟩ => .ok ()
  | .term ⟨_, .dag _⟩ => .ok ()
  | .term ⟨_, .moduleAlias _⟩ => .ok ()
  | .term ⟨_, .publicDagAlias _⟩ => .ok ()
  | .unit ⟨_, .prelude⟩ => .ok ()
  | .unit ⟨_, .base⟩ => .ok ()
  | .unit ⟨_, .constScaled⟩ => .ok ()
  | entity => .error (.declarationNotImportable entity)

/-- Reject the first unresolved required input in dependency-closure order. -/
def validateStaticDependenciesClosed
    (declaration : Entity) : List StaticEntity → Except ImportError Unit
  | [] => .ok ()
  | dependency :: rest =>
      match dependency.requiredInputKind with
      | some _ => .error (.unresolvedStaticDependency declaration dependency)
      | none => validateStaticDependenciesClosed declaration rest

/-- Executable direct-import capability check. -/
def validateImport
    (declaration : ExportedDeclaration) : Except ImportError Unit :=
  match validateBlueprintImportCategory declaration.entity with
  | .ok () =>
      validateStaticDependenciesClosed declaration.entity declaration.staticDependencies
  | .error failure => .error failure

/-- Construct the blueprint-stable semantic identity introduced by an import. -/
def importTarget
    (declaration : ExportedDeclaration) : Except ImportError ProjectionTarget :=
  match validateImport declaration with
  | .ok () => .ok (.staticSpecialization ⟨declaration.entity, []⟩)
  | .error failure => .error failure

/-- Typed failures from include projection construction. -/
inductive ProjectionError where
  | missingStaticBinding (input : StaticEntity)
  | invalidStaticBinding (input target : StaticEntity)
  | constructorOwnerRebound (constructor : TermEntity) (target : StaticEntity)
  | declarationNotProjectable (entity : Entity)
  deriving DecidableEq, Repr

/-- Resolve one required input to one concrete target of the exact same kind. -/
def resolveStaticBinding
    (bindings : StaticBindings)
    (input : StaticEntity) : Except ProjectionError StaticEntity :=
  match bindings input.id with
  | none => .error (.missingStaticBinding input)
  | some target =>
      if StaticBindingValid input target then
        .ok target
      else
        .error (.invalidStaticBinding input target)

/-- Normalize supplied bindable entries in deterministic dependency-closure order. -/
def resolveStaticSubstitutions
    (bindings : StaticBindings) :
    List StaticEntity → Except ProjectionError (List StaticSubstitutionEntry)
  | [] => .ok []
  | dependency :: rest =>
      match dependency.inputRole with
      | some (_, .optionalInput) =>
          match bindings dependency.id with
          | none => resolveStaticSubstitutions bindings rest
          | some _ =>
              match resolveStaticBinding bindings dependency with
              | .error failure => .error failure
              | .ok target =>
                  match resolveStaticSubstitutions bindings rest with
                  | .error failure => .error failure
                  | .ok tail => .ok ({ input := dependency.id, target } :: tail)
      | some (_, .requiredInput) =>
          match resolveStaticBinding bindings dependency with
          | .error failure => .error failure
          | .ok target =>
              match resolveStaticSubstitutions bindings rest with
              | .error failure => .error failure
              | .ok tail => .ok ({ input := dependency.id, target } :: tail)
      | _ => resolveStaticSubstitutions bindings rest

/-- Construct the semantic target for one already-resolved declaration. -/
def projectTarget
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (entity : Entity)
    (substitutions : List StaticSubstitutionEntry) :
    Except ProjectionError ProjectionTarget :=
  match includeProjection? entity with
  | none => .error (.declarationNotProjectable entity)
  | some (.staticDeclaration staticEntity) =>
      match staticEntity.inputRole with
      | some (_, .requiredInput) =>
          match resolveStaticBinding bindings staticEntity with
          | .ok effective =>
              .ok (.staticSpecialization ⟨.static effective, []⟩)
          | .error failure => .error failure
      | some (_, .optionalInput) =>
          match bindings staticEntity.id with
          | none => .ok (.staticSpecialization ⟨.static staticEntity, substitutions⟩)
          | some _ =>
              match resolveStaticBinding bindings staticEntity with
              | .ok effective =>
                  .ok (.staticSpecialization ⟨.static effective, []⟩)
              | .error failure => .error failure
      | some (_, .fixed) =>
          .ok (.staticSpecialization ⟨.static staticEntity, substitutions⟩)
      | none => .error (.declarationNotProjectable entity)
  | some (.constNode termEntity) =>
      .ok (.staticSpecialization ⟨.term termEntity, substitutions⟩)
  | some (.constructor termEntity owner) =>
      match bindings owner with
      | none => .ok (.staticSpecialization ⟨.term termEntity, substitutions⟩)
      | some target => .error (.constructorOwnerRebound termEntity target)
  | some (.runtimeTerm termEntity) =>
      .ok (.runtimeTerm ⟨.term termEntity, instanceId, substitutions⟩)
  | some (.assertion termEntity) =>
      .ok (.assertion ⟨.term termEntity, instanceId, substitutions⟩)
  | some (.visualization termEntity) =>
      .ok (.visualization ⟨.term termEntity, instanceId, substitutions⟩)
  | some (.staticUnit unitEntity) =>
      .ok (.staticSpecialization ⟨.unit unitEntity, substitutions⟩)
  | some (.runtimeUnit unitEntity) =>
      .ok (.runtimeUnit ⟨unitEntity.id, instanceId⟩ substitutions)

/-- Executable include projection after namespace and visibility resolution. -/
def project
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (declaration : ExportedDeclaration) :
    Except ProjectionError ProjectionTarget :=
  match resolveStaticSubstitutions bindings declaration.staticDependencies with
  | .ok substitutions =>
      projectTarget bindings instanceId declaration.entity substitutions
  | .error failure => .error failure

/-- Construct one source-visible projection binding. -/
def projectBinding
    (bindings : StaticBindings)
    (instanceId : InstanceId)
    (localName : NameAtom)
    (declaration : ExportedDeclaration) :
    Except ProjectionError ProjectionBinding :=
  match project bindings instanceId declaration with
  | .ok target =>
      .ok { instanceId, name := localName, source := entityId declaration.entity, target }
  | .error failure => .error failure

end Graphcal.Static.ExternalSurface
