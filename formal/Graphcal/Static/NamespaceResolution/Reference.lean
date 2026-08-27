import Graphcal.Static.NamespaceResolution.Model

namespace Graphcal.Static.NamespaceResolution

/-- Executable cardinality summary for one exact lookup query. -/
inductive LookupSummary where
  | none
  | one (binding : Binding)
  | many
  deriving DecidableEq, Repr

/-- Add one binding to a lookup summary without choosing a winner. -/
def LookupSummary.add
    (query : Query)
    (binding : Binding)
    (summary : LookupSummary) : LookupSummary :=
  if binding.Matches query then
    match summary with
    | .none => .one binding
    | .one _ | .many => .many
  else
    summary

/-- Count matching bindings while retaining the sole binding when unique. -/
def summarizeLookup : Environment → Query → LookupSummary
  | [], _ => .none
  | binding :: rest, query => (summarizeLookup rest query).add query binding

/-- Typed failures produced by the executable namespace resolver. -/
inductive ResolutionError where
  | unknown (query : Query)
  | ambiguous (query : Query)
  | notNamespaceOwner (entity : Entity)
  | nonDagPathSegment (entity : Entity)
  | cannotTraverse (target : NamespaceTarget) (child : NameAtom)
  | wrongNamespace (expected : Namespace) (actual : Entity)
  | invalidStaticUse (use : StaticUse) (entity : StaticEntity)
  | invalidTermUse (use : TermUse) (entity : TermEntity)
  | labelOwnerNotIndex (entity : StaticEntity)
  | labelOwnerMismatch (expected : StaticId) (actual : TermEntity)
  deriving DecidableEq, Repr

/-- Executable exact lookup. Ambiguity is an error rather than first-wins. -/
def lookup (environment : Environment) (query : Query) : Except ResolutionError Entity :=
  match summarizeLookup environment query with
  | .none => .error (.unknown query)
  | .one binding => .ok binding.entity
  | .many => .error (.ambiguous query)

/-- Extract the namespace target carried by a namespace-bearing Term. -/
def namespaceTarget (entity : Entity) : Except ResolutionError NamespaceTarget :=
  match entity with
  | .term ⟨_, .dag target⟩ => .ok (.dag target)
  | .term ⟨_, .moduleAlias target⟩ => .ok (.dag target)
  | .term ⟨_, .publicDagAlias target⟩ => .ok (.dag target)
  | .term ⟨_, .includeInstanceAlias target⟩ => .ok (.instance target)
  | .term ⟨_, .pluginAlias target⟩ => .ok (.plugin target)
  | other => .error (.notNamespaceOwner other)

/-- Dotted path children must themselves denote DAGs. -/
def dagTarget (entity : Entity) : Except ResolutionError DagId :=
  match entity with
  | .term ⟨_, .dag target⟩ => .ok target
  | .term ⟨_, .moduleAlias target⟩ => .ok target
  | .term ⟨_, .publicDagAlias target⟩ => .ok target
  | other => .error (.nonDagPathSegment other)

/-- Traverse the dotted child-DAG suffix before a `::` member boundary. -/
def resolveNamespaceSteps
    (environment : Environment) :
    NamespaceTarget → List NameAtom → Except ResolutionError NamespaceTarget
  | target, [] => .ok target
  | .dag current, child :: rest => do
      let entity ← lookup environment (Query.exact (.dag current) .term child)
      let next ← dagTarget entity
      resolveNamespaceSteps environment (.dag next) rest
  | target, child :: _ => .error (.cannotTraverse target child)

/-- Resolve the namespace-bearing root and its dotted child DAGs. -/
def resolveNamespacePath
    (environment : Environment)
    (origin : DagId)
    (path : NamespacePath) : Except ResolutionError NamespaceTarget := do
  let root ← lookup environment (Query.exact (.dag origin) .term path.root)
  let firstTarget ← namespaceTarget root
  resolveNamespaceSteps environment firstTarget path.children

/-- Resolve a visible name or a member selected after `::`. -/
def resolveHead
    (environment : Environment)
    (space : Namespace)
    (head : NameHead) : Except ResolutionError Entity :=
  match head with
  | .visible scopes name => lookup environment { scopes, space, name }
  | .member origin owner name => do
      let target ← resolveNamespacePath environment origin owner
      lookup environment (Query.exact target.scope space name)

/-- Executable Static kind validation, independent from `StaticPermits`. -/
def validateStatic
    (use : StaticUse)
    (entity : Entity) : Except ResolutionError StaticEntity :=
  match entity with
  | .static staticEntity =>
      match use, staticEntity.kind with
      | .type, .nominalType
      | .type, .genericTypeParam
      | .dimension, .dimension
      | .dimension, .genericDimParam
      | .index, .index
      | .index, .genericIndexParam
      | .nat, .genericNatParam
      | .timeScale, .timeScale => .ok staticEntity
      | _, _ => .error (.invalidStaticUse use staticEntity)
  | other => .error (.wrongNamespace .static other)

/-- Executable Term capability validation, independent from `TermPermits`. -/
def validateTerm
    (use : TermUse)
    (entity : Entity) : Except ResolutionError TermEntity :=
  match entity with
  | .term termEntity =>
      match use, termEntity.kind with
      | .bareValue, .constructor _
      | .bareValue, .builtinConstant
      | .bareValue, .localBinding
      | .call, .constructor _
      | .call, .builtinFunction
      | .call, .contextualCallable
      | .call, .externFunction
      | .localGraphRead, .param
      | .localGraphRead, .node
      | .localGraphRead, .constNode
      | .blueprintGraphRead, .constNode
      | .instanceGraphRead, .param
      | .instanceGraphRead, .node
      | .instanceGraphRead, .constNode
      | .includeOutcome, .param
      | .includeOutcome, .node
      | .includeOutcome, .assertion
      | .includeOutcome, .visualization => .ok termEntity
      | _, _ => .error (.invalidTermUse use termEntity)
  | other => .error (.wrongNamespace .term other)

/-- Unit positions accept exactly Unit entities. -/
def validateUnit (entity : Entity) : Except ResolutionError UnitEntity :=
  match entity with
  | .unit unitEntity => .ok unitEntity
  | other => .error (.wrongNamespace .unit other)

/-- Index-label owners must be concrete indexes, not generic index parameters. -/
def requireConcreteIndex
    (entity : StaticEntity) : Except ResolutionError StaticEntity :=
  match entity.kind with
  | .index => .ok entity
  | _ => .error (.labelOwnerNotIndex entity)

/-- Validate that a Term label retains the canonical owner selected by `#`. -/
def validateIndexLabel
    (expectedOwner : StaticId)
    (entity : Entity) : Except ResolutionError TermEntity :=
  match entity with
  | .term labelEntity =>
      match labelEntity.kind with
      | .indexLabel actualOwner =>
          if actualOwner = expectedOwner then
            .ok labelEntity
          else
            .error (.labelOwnerMismatch expectedOwner labelEntity)
      | _ => .error (.labelOwnerMismatch expectedOwner labelEntity)
  | other => .error (.wrongNamespace .term other)

/-- Resolve `Owner#Label` through the canonical Static index owner. -/
def resolveLabel
    (environment : Environment)
    (owner : NameHead)
    (label : NameAtom) : Except ResolutionError TermEntity := do
  let ownerCandidate ← resolveHead environment .static owner
  let indexCandidate ← validateStatic .index ownerCandidate
  let indexEntity ← requireConcreteIndex indexCandidate
  let labelCandidate ←
    lookup environment (Query.exact (.indexLabels indexEntity.id) .term label)
  validateIndexLabel indexEntity.id labelCandidate

/-- The executable reference resolver certified in `Proofs`. -/
def resolve
    (environment : Environment)
    (reference : Reference) : Except ResolutionError Entity :=
  match reference with
  | .static head use => do
      let candidate ← resolveHead environment .static head
      return .static (← validateStatic use candidate)
  | .term head use => do
      let candidate ← resolveHead environment .term head
      return .term (← validateTerm use candidate)
  | .unit head => do
      let candidate ← resolveHead environment .unit head
      return .unit (← validateUnit candidate)
  | .label owner label => do
      return .term (← resolveLabel environment owner label)
  | .call callee _ => do
      let candidate ← resolveHead environment .term callee
      return .term (← validateTerm .call candidate)

/-- A binder rejection carries the exact namespace/name query that was occupied. -/
inductive BinderError where
  | visibleNameOccupied (query : Query)
  deriving DecidableEq, Repr

/-- Enforce the approved no-shadowing rule for Static and Term binders. -/
def validateBinder
    (environment : Environment)
    (request : BinderRequest) : Except BinderError Unit :=
  let query : Query := {
    scopes := request.scope :: request.visibleScopes
    space := request.kind.space
    name := request.name
  }
  match summarizeLookup environment query with
  | .none => .ok ()
  | .one _ | .many => .error (.visibleNameOccupied query)

/-- Typed duplicate-slot failure from executable scope construction. -/
inductive ScopeError where
  | duplicate (slot : BindingSlot)
  deriving DecidableEq, Repr

/-- Insert one binding only when its exact namespace slot is empty. -/
def insertBinding
    (binding : Binding)
    (environment : Environment) : Except ScopeError Environment :=
  let query := Query.exact binding.scope binding.entity.space binding.name
  match summarizeLookup environment query with
  | .none => .ok (binding :: environment)
  | .one _ | .many => .error (.duplicate binding.slot)

/-- Executable, order-insensitive scope acceptance with structured bindings. -/
def buildScope : List Binding → Except ScopeError Environment
  | [] => .ok []
  | binding :: rest => do
      let environment ← buildScope rest
      insertBinding binding environment

end Graphcal.Static.NamespaceResolution
