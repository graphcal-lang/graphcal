import Graphcal.Static.Interface

namespace Graphcal.Static.NamespaceResolution

abbrev StaticInputKind := Interface.StaticInputKind
abbrev StaticRole := Interface.StaticRole

/--
An atomic source spelling. Structured names are represented by the owner/path
structures below rather than by concatenating atoms into one string.
-/
abbrev NameAtom := String

/-- Canonical identity of a DAG, independent of source aliases. -/
structure DagId where
  package : Nat
  serial : Nat
  deriving DecidableEq, Repr

/-- Canonical identity of a Static entity. -/
structure StaticId where
  owner : DagId
  serial : Nat
  deriving DecidableEq, Repr

/-- Canonical identity of a Term entity. -/
structure TermId where
  owner : DagId
  serial : Nat
  deriving DecidableEq, Repr

/-- Canonical identity of a Unit entity. -/
structure UnitId where
  owner : DagId
  serial : Nat
  deriving DecidableEq, Repr

/-- A configured DAG instance has its own member scope but retains its blueprint. -/
structure InstanceId where
  blueprint : DagId
  serial : Nat
  deriving DecidableEq, Repr

/-- Canonical identity of a plugin namespace. -/
structure PluginId where
  package : Nat
  serial : Nat
  deriving DecidableEq, Repr

/-- Identity of one lexical scope in a DAG body. -/
structure LexicalScopeId where
  owner : DagId
  serial : Nat
  deriving DecidableEq, Repr

/-- The three user-bindable namespaces approved for Graphcal. -/
inductive Namespace where
  | static
  | term
  | unit
  deriving DecidableEq, Repr

/--
Every lookup happens in an explicit scope. Index labels use an associated Term
scope owned by the canonical Static index; this is not a fourth namespace.
-/
inductive ScopeId where
  | dag (id : DagId)
  | lexical (id : LexicalScopeId)
  | instance (id : InstanceId)
  | plugin (id : PluginId)
  | indexLabels (owner : StaticId)
  deriving DecidableEq, Repr

/-- A namespace-bearing Term resolves to one of these structured targets. -/
inductive NamespaceTarget where
  | dag (id : DagId)
  | instance (id : InstanceId)
  | plugin (id : PluginId)
  deriving DecidableEq, Repr

/-- The lookup scope exposed by a namespace-bearing target. -/
def NamespaceTarget.scope : NamespaceTarget → ScopeId
  | .dag id => .dag id
  | .instance id => .instance id
  | .plugin id => .plugin id

/--
Closed Static categories. Input-capable declarations retain whether they are
concrete definitions or required static input ports; generic constraints remain
explicit categories.
-/
inductive StaticKind where
  | nominalType (role : StaticRole)
  | dimension (role : StaticRole)
  | index (role : StaticRole)
  | genericTypeParam
  | genericDimParam
  | genericIndexParam
  | genericNatParam
  | timeScale
  deriving DecidableEq, Repr

/-- Build the semantic Static kind for one shared input category. -/
def StaticInputKind.toStaticKind
    (kind : StaticInputKind)
    (role : StaticRole) : StaticKind :=
  match kind with
  | .nominalType => .nominalType role
  | .dimension => .dimension role
  | .index => .index role

/-- The shared typed input category, when this Static kind can be bound. -/
def StaticKind.inputKind : StaticKind → Option StaticInputKind
  | .nominalType _ => some .nominalType
  | .dimension _ => some .dimension
  | .index _ => some .index
  | _ => none

/-- Whether an input-capable Static declaration is a required input port. -/
def StaticKind.isRequiredInput : StaticKind → Bool
  | .nominalType .requiredInput
  | .dimension .requiredInput
  | .index .requiredInput => true
  | _ => false

/--
Closed Term categories. Constructors are flat Terms while index labels retain a
canonical Static owner and are installed only in that owner's associated scope.
-/
inductive TermKind where
  | param
  | node
  | constNode
  | constructor (owner : StaticId)
  | builtinConstant
  | builtinFunction
  | contextualCallable
  | externFunction
  | dag (target : DagId)
  | moduleAlias (target : DagId)
  | publicDagAlias (target : DagId)
  | includeInstanceAlias (target : InstanceId)
  | pluginAlias (target : PluginId)
  | assertion
  | visualization
  | indexLabel (owner : StaticId)
  | localBinding
  deriving DecidableEq, Repr

/-- Closed Unit categories. -/
inductive UnitKind where
  | prelude
  | base
  | constScaled
  | runtimeScaled
  deriving DecidableEq, Repr

structure StaticEntity where
  id : StaticId
  kind : StaticKind
  deriving DecidableEq, Repr

/-- The typed binding role carried by a Static entity, when any. -/
def StaticEntity.inputRole
    (entity : StaticEntity) : Option (StaticInputKind × StaticRole) :=
  match entity.kind with
  | .nominalType role => some (.nominalType, role)
  | .dimension role => some (.dimension, role)
  | .index role => some (.index, role)
  | _ => none

/-- The required-input category carried by a Static entity, when any. -/
def StaticEntity.requiredInputKind (entity : StaticEntity) : Option StaticInputKind :=
  match entity.inputRole with
  | some (kind, .requiredInput) => some kind
  | _ => none

/-- The optional-input category carried by a Static entity, when any. -/
def StaticEntity.optionalInputKind (entity : StaticEntity) : Option StaticInputKind :=
  match entity.inputRole with
  | some (kind, .optionalInput) => some kind
  | _ => none

/-- Any caller-bindable Static input category carried by this entity. -/
def StaticEntity.bindableInputKind (entity : StaticEntity) : Option StaticInputKind :=
  match entity.inputRole with
  | some (kind, .optionalInput) | some (kind, .requiredInput) => some kind
  | _ => none

/-- Any non-required concrete category that may be an effective binding target. -/
def StaticEntity.concreteInputKind (entity : StaticEntity) : Option StaticInputKind :=
  match entity.inputRole with
  | some (kind, .fixed) | some (kind, .optionalInput) => some kind
  | _ => none

structure TermEntity where
  id : TermId
  kind : TermKind
  deriving DecidableEq, Repr

structure UnitEntity where
  id : UnitId
  kind : UnitKind
  deriving DecidableEq, Repr

/--
The semantic sum makes it impossible for one entity value to inhabit several
namespaces. Aliases bind the same canonical entity value under another slot.
-/
inductive Entity where
  | static (entity : StaticEntity)
  | term (entity : TermEntity)
  | unit (entity : UnitEntity)
  deriving DecidableEq, Repr

/-- The unique namespace selected by an entity's constructor. -/
def Entity.space : Entity → Namespace
  | .static _ => .static
  | .term _ => .term
  | .unit _ => .unit

/-- One source-visible binding of a canonical entity. -/
structure Binding where
  scope : ScopeId
  name : NameAtom
  entity : Entity
  deriving DecidableEq, Repr

/-- A binding's sole collision slot. -/
structure BindingSlot where
  scope : ScopeId
  space : Namespace
  name : NameAtom
  deriving DecidableEq, Repr

/-- Binding aliases may change scope and spelling but not canonical entity data. -/
def Binding.slot (binding : Binding) : BindingSlot := {
  scope := binding.scope
  space := binding.entity.space
  name := binding.name
}

/-- The finite environment consumed by the executable reference resolver. -/
abbrev Environment := List Binding

/--
A query can search one exact scope or an explicitly supplied visible scope
chain. No hidden fallback scopes are added by the resolver.
-/
structure Query where
  scopes : List ScopeId
  space : Namespace
  name : NameAtom
  deriving DecidableEq, Repr

/-- Construct a query against exactly one scope. -/
def Query.exact (scope : ScopeId) (space : Namespace) (name : NameAtom) : Query := {
  scopes := [scope]
  space
  name
}

/-- Declarative statement that a binding occupies one of a query's slots. -/
def Binding.Matches (binding : Binding) (query : Query) : Prop :=
  binding.scope ∈ query.scopes ∧
    binding.entity.space = query.space ∧
    binding.name = query.name

instance (binding : Binding) (query : Query) : Decidable (binding.Matches query) := by
  unfold Binding.Matches
  infer_instance

/--
A structured namespace path. Its root is a flat Term in the current DAG and
its child atoms are traversed as DAGs before the eventual `::` boundary.
-/
structure NamespacePath where
  root : NameAtom
  children : List NameAtom
  deriving DecidableEq, Repr

/--
A source name head is either looked up in an explicit visible scope chain or
selected after crossing a structured namespace path with `::`.
-/
inductive NameHead where
  | visible (scopes : List ScopeId) (name : NameAtom)
  | member (origin : DagId) (owner : NamespacePath) (name : NameAtom)
  deriving DecidableEq, Repr

/-- The Static category required by a source position. -/
inductive StaticUse where
  | type
  | dimension
  | index
  | nat
  | timeScale
  deriving DecidableEq, Repr

/-- The operation requested of an already resolved Term. -/
inductive TermUse where
  | bareValue
  | call
  | localGraphRead
  | blueprintGraphRead
  | instanceGraphRead
  | includeProjection
  deriving DecidableEq, Repr

/-- Preserved call syntax; it must not choose the callee category. -/
inductive ArgumentShape where
  | empty
  | positional
  | named
  | generic
  | closure
  | mixed
  deriving DecidableEq, Repr

/--
The punctuation-preserving reference forms modeled by the namespace kernel.
`label` corresponds to `Owner#Label`; `member` heads contain the `::` boundary.
-/
inductive Reference where
  | static (head : NameHead) (use : StaticUse)
  | term (head : NameHead) (use : TermUse)
  | unit (head : NameHead)
  | label (owner : NameHead) (label : NameAtom)
  | call (callee : NameHead) (arguments : ArgumentShape)
  deriving DecidableEq, Repr

/-- The one result namespace selected by each punctuation-preserving form. -/
def Reference.resultSpace : Reference → Namespace
  | .static _ _ => .static
  | .term _ _ => .term
  | .unit _ => .unit
  | .label _ _ => .term
  | .call _ _ => .term

/--
The source forms for a DAG input binding. `unmarked` means exactly a parameter;
marked selectors reuse the one shared Static input category inventory.
-/
inductive InputBindingCategory where
  | unmarked
  | marked (kind : StaticInputKind)
  deriving DecidableEq, Repr

/-- Namespace selected before an input target is looked up. -/
def InputBindingCategory.space : InputBindingCategory → Namespace
  | .unmarked => .term
  | .marked _ => .static

/-- One parsed DAG input selector, shared by `include` and direct DAG calls. -/
structure InputBindingSelector where
  category : InputBindingCategory
  target : NameHead
  deriving DecidableEq, Repr

/-- Exact semantic target produced by a categorized DAG input selector. -/
inductive InputBindingTarget where
  | param (entity : TermEntity)
  | static (kind : StaticInputKind) (entity : StaticEntity)
  deriving DecidableEq, Repr

/-- Only Static and Term introduce lexical/generic binders. -/
inductive BinderKind where
  | static
  | term
  deriving DecidableEq, Repr

/-- Namespace selected by a binder declaration. -/
def BinderKind.space : BinderKind → Namespace
  | .static => .static
  | .term => .term

/-- One proposed binder and the scopes visible at its declaration point. -/
structure BinderRequest where
  kind : BinderKind
  scope : ScopeId
  visibleScopes : List ScopeId
  name : NameAtom
  deriving DecidableEq, Repr

/-- Data needed to introduce one flat constructor. -/
structure ConstructorIntro where
  name : NameAtom
  id : TermId
  deriving DecidableEq, Repr

/-- A nominal declaration introduces one Static type and flat Term constructors. -/
structure NominalTypeDecl where
  owner : DagId
  name : NameAtom
  id : StaticId
  constructors : List ConstructorIntro
  deriving DecidableEq, Repr

/-- Data needed to introduce one owner-scoped index label. -/
structure LabelIntro where
  name : NameAtom
  id : TermId
  deriving DecidableEq, Repr

/-- An index declaration introduces one Static axis and associated Term labels. -/
structure IndexDecl where
  owner : DagId
  name : NameAtom
  id : StaticId
  labels : List LabelIntro
  deriving DecidableEq, Repr

/-- Expand a nominal declaration into its distinct namespace bindings. -/
def NominalTypeDecl.bindings (decl : NominalTypeDecl) : List Binding :=
  {
    scope := .dag decl.owner
    name := decl.name
    entity := .static { id := decl.id, kind := .nominalType .fixed }
  } :: decl.constructors.map fun constructor => {
    scope := .dag decl.owner
    name := constructor.name
    entity := .term {
      id := constructor.id
      kind := .constructor decl.id
    }
  }

/-- Expand an index declaration into its Static axis and owner-scoped labels. -/
def IndexDecl.bindings (decl : IndexDecl) : List Binding :=
  {
    scope := .dag decl.owner
    name := decl.name
    entity := .static { id := decl.id, kind := .index .fixed }
  } :: decl.labels.map fun label => {
    scope := .indexLabels decl.id
    name := label.name
    entity := .term {
      id := label.id
      kind := .indexLabel decl.id
    }
  }

end Graphcal.Static.NamespaceResolution
