import Graphcal.Static.IncludeProjection

namespace Graphcal.Static.NamespaceResolution

open Graphcal.Static

/-- No binding in an environment answers the given query. -/
def NoMatch (environment : Environment) (query : Query) : Prop :=
  ∀ binding, binding ∈ environment → ¬ binding.Matches query

/--
Declarative exact lookup. `head` requires every remaining binding to miss, so
this relation holds only when exactly one visible binding answers the query.
-/
inductive Lookup (query : Query) : Environment → Entity → Prop where
  | head
      {binding : Binding}
      {rest : Environment}
      (doesMatch : binding.Matches query)
      (restMisses : NoMatch rest query) :
      Lookup query (binding :: rest) binding.entity
  | tail
      {binding : Binding}
      {rest : Environment}
      {entity : Entity}
      (misses : ¬ binding.Matches query)
      (found : Lookup query rest entity) :
      Lookup query (binding :: rest) entity

/-- A Term entity exposes a structured namespace target. -/
inductive OwnsNamespace : Entity → NamespaceTarget → Prop where
  | dag (id : TermId) (target : DagId) :
      OwnsNamespace (.term ⟨id, .dag target⟩) (.dag target)
  | moduleAlias (id : TermId) (target : DagId) :
      OwnsNamespace (.term ⟨id, .moduleAlias target⟩) (.dag target)
  | publicDagAlias (id : TermId) (target : DagId) :
      OwnsNamespace (.term ⟨id, .publicDagAlias target⟩) (.dag target)
  | includeInstanceAlias (id : TermId) (target : InstanceId) :
      OwnsNamespace (.term ⟨id, .includeInstanceAlias target⟩) (.instance target)
  | pluginAlias (id : TermId) (target : PluginId) :
      OwnsNamespace (.term ⟨id, .pluginAlias target⟩) (.plugin target)

/-- Only DAG-bearing Terms may extend a dotted DAG path. -/
inductive OwnsDag : Entity → DagId → Prop where
  | dag (id : TermId) (target : DagId) :
      OwnsDag (.term ⟨id, .dag target⟩) target
  | moduleAlias (id : TermId) (target : DagId) :
      OwnsDag (.term ⟨id, .moduleAlias target⟩) target
  | publicDagAlias (id : TermId) (target : DagId) :
      OwnsDag (.term ⟨id, .publicDagAlias target⟩) target

/-- Declarative traversal of the dotted child-DAG portion of a path. -/
inductive NamespaceSteps (environment : Environment) :
    NamespaceTarget → List NameAtom → NamespaceTarget → Prop where
  | done (target : NamespaceTarget) : NamespaceSteps environment target [] target
  | child
      {current next : DagId}
      {name : NameAtom}
      {rest : List NameAtom}
      {finalTarget : NamespaceTarget}
      {entity : Entity}
      (member : Lookup (Query.exact (.dag current) .term name) environment entity)
      (ownsDag : OwnsDag entity next)
      (remaining : NamespaceSteps environment (.dag next) rest finalTarget) :
      NamespaceSteps environment (.dag current) (name :: rest) finalTarget

/-- Resolve a namespace root and every dotted child DAG before a `::` boundary. -/
def NamespacePathResolves
    (environment : Environment)
    (origin : DagId)
    (path : NamespacePath)
    (target : NamespaceTarget) : Prop :=
  ∃ entity firstTarget,
    Lookup (Query.exact (.dag origin) .term path.root) environment entity ∧
      OwnsNamespace entity firstTarget ∧
      NamespaceSteps environment firstTarget path.children target

/-- Resolve a bare/visible head or a member selected after a namespace path. -/
def HeadResolves
    (environment : Environment)
    (space : Namespace)
    (head : NameHead)
    (entity : Entity) : Prop :=
  match head with
  | .visible scopes name =>
      Lookup { scopes, space, name } environment entity
  | .member origin owner name =>
      ∃ target,
        NamespacePathResolves environment origin owner target ∧
          Lookup (Query.exact target.scope space name) environment entity

/-- Static lookup is followed by explicit kind validation. -/
def StaticPermits : StaticUse → StaticEntity → Prop
  | .type, ⟨_, .nominalType _⟩ => True
  | .type, ⟨_, .dimension _⟩ => True
  | .type, ⟨_, .genericTypeParam⟩ => True
  | .type, ⟨_, .genericDimParam⟩ => True
  | .dimension, ⟨_, .dimension _⟩ => True
  | .dimension, ⟨_, .genericDimParam⟩ => True
  | .index, ⟨_, .index _⟩ => True
  | .index, ⟨_, .genericIndexParam⟩ => True
  | .nat, ⟨_, .genericNatParam⟩ => True
  | .timeScale, ⟨_, .timeScale⟩ => True
  | _, _ => False

/-- Term lookup is followed by explicit operation/capability validation. -/
def TermPermits : TermUse → TermEntity → Prop
  | .bareValue, ⟨_, .constructor _⟩ => True
  | .bareValue, ⟨_, .builtinConstant⟩ => True
  | .bareValue, ⟨_, .localBinding⟩ => True
  | .call, ⟨_, .constructor _⟩ => True
  | .call, ⟨_, .builtinFunction⟩ => True
  | .call, ⟨_, .contextualCallable⟩ => True
  | .call, ⟨_, .externFunction⟩ => True
  | .localGraphRead, ⟨_, .param⟩ => True
  | .localGraphRead, ⟨_, .node⟩ => True
  | .localGraphRead, ⟨_, .constNode⟩ => True
  | .blueprintGraphRead, ⟨_, .constNode⟩ => True
  | .instanceGraphRead, ⟨_, .param⟩ => True
  | .instanceGraphRead, ⟨_, .node⟩ => True
  | .instanceGraphRead, ⟨_, .constNode⟩ => True
  | .includeProjection, entity => IncludeProjectable (.term entity)
  | _, _ => False

/-- Exact category and required-input relation for DAG input bindings. -/
inductive InputTargetMatches :
    InputBindingCategory → Entity → InputBindingTarget → Prop where
  | param (id : TermId) :
      InputTargetMatches .unmarked (.term ⟨id, .param⟩)
        (.param ⟨id, .param⟩)
  | optionalStatic (kind : StaticInputKind) (id : StaticId) :
      InputTargetMatches (.marked kind)
        (.static ⟨id, kind.toStaticKind .optionalInput⟩)
        (.static kind ⟨id, kind.toStaticKind .optionalInput⟩)
  | requiredStatic (kind : StaticInputKind) (id : StaticId) :
      InputTargetMatches (.marked kind)
        (.static ⟨id, kind.toStaticKind .requiredInput⟩)
        (.static kind ⟨id, kind.toStaticKind .requiredInput⟩)

/--
An unmarked input performs one Term lookup and requires `param`; marked inputs
perform one Static lookup and require the exact marker category.
-/
def InputBindingResolves
    (environment : Environment)
    (selector : InputBindingSelector)
    (target : InputBindingTarget) : Prop :=
  ∃ entity,
    HeadResolves environment selector.category.space selector.target entity ∧
      InputTargetMatches selector.category entity target

/--
Normative reference resolution. Every constructor performs one namespace lookup
and then validates the requested category; no constructor describes fallback.
-/
inductive Resolves (environment : Environment) : Reference → Entity → Prop where
  | static
      {head : NameHead}
      {use : StaticUse}
      {entity : StaticEntity}
      (resolved : HeadResolves environment .static head (.static entity))
      (permitted : StaticPermits use entity) :
      Resolves environment (.static head use) (.static entity)
  | term
      {head : NameHead}
      {use : TermUse}
      {entity : TermEntity}
      (resolved : HeadResolves environment .term head (.term entity))
      (permitted : TermPermits use entity) :
      Resolves environment (.term head use) (.term entity)
  | unit
      {head : NameHead}
      {entity : UnitEntity}
      (resolved : HeadResolves environment .unit head (.unit entity)) :
      Resolves environment (.unit head) (.unit entity)
  | label
      {owner : NameHead}
      {label : NameAtom}
      {indexEntity : StaticEntity}
      {labelEntity : TermEntity}
      (ownerResolved : HeadResolves environment .static owner (.static indexEntity))
      (ownerIsIndex : indexEntity.concreteInputKind = some .index)
      (labelResolved :
        Lookup
          (Query.exact (.indexLabels indexEntity.id) .term label)
          environment
          (.term labelEntity))
      (ownerMatches : labelEntity.kind = .indexLabel indexEntity.id) :
      Resolves environment (.label owner label) (.term labelEntity)
  | call
      {callee : NameHead}
      {arguments : ArgumentShape}
      {entity : TermEntity}
      (resolved : HeadResolves environment .term callee (.term entity))
      (permitted : TermPermits .call entity) :
      Resolves environment (.call callee arguments) (.term entity)

/-- A slot is absent from an environment. -/
def SlotAbsent (slot : BindingSlot) (environment : Environment) : Prop :=
  ∀ binding, binding ∈ environment → binding.slot ≠ slot

/-- Declarative scope validity: every `(scope, namespace, name)` slot is unique. -/
def ScopeWellFormed : Environment → Prop
  | [] => True
  | binding :: rest => SlotAbsent binding.slot rest ∧ ScopeWellFormed rest

/--
No-shadowing for Static and Term binders. The binder's own scope is checked in
addition to every enclosing visible scope supplied by the caller.
-/
def BinderFresh (environment : Environment) (request : BinderRequest) : Prop :=
  NoMatch environment {
    scopes := request.scope :: request.visibleScopes
    space := request.kind.space
    name := request.name
  }

end Graphcal.Static.NamespaceResolution
