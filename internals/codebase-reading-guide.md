# Graphcal Codebase Reading Guide

This guide is a map for reading the Graphcal source code as it exists today.
It focuses on the compiler/evaluator pipeline, the crate boundaries, and the
typed data structures that carry language semantics through the system.

## 1. Pipeline

### Representation Names

Compiler code commonly names data shapes by how far they are from source text
and how many semantic invariants they carry:

- **AST** means **Abstract Syntax Tree**. It is a tree-shaped representation of
  source syntax after parsing. An AST is still source-oriented: it preserves
  spans, syntactic forms, and source paths so tools can point back to what the
  user wrote.
- **IR** means **Intermediate Representation**. It is the general compiler term
  for any representation between syntax and execution; Graphcal does not use a
  bare `IR` phase type.
- **HIR** means **High-level Intermediate Representation**. `HirDag` is one
  canonically identified DAG body, and `HirProject` is the complete pre-check
  project. HIR remains close to source expression/type structure, but replaces
  source paths with canonical owner-qualified names, lexical IDs, and typed
  built-in variants.
- **TIR** means **Typed Intermediate Representation**. It is the type- and
  dimension-checked representation used by the evaluator. TIR combines HIR's
  declaration/DAG structure with resolved type expressions, dimension facts,
  and per-DAG compilation state.

These names are relative, not universal. Another compiler may use "HIR" or
"TIR" differently; in this codebase, read them as the boundaries above.

### Pipeline Rationale

A `.gcl` file cannot safely go straight from text to evaluation. Early stages
need to remember exactly what the user wrote, while later stages need to know
what those words mean in the whole project.

For example, this source reference:

```gcl
import helpers.physics
node force = helpers.physics.mass * acceleration
```

starts life as text with spans, then as a dotted path, and eventually as a
reference to one specific declaration owned by one specific `DagId`. Keeping
those steps separate prevents bugs where two files define the same leaf name, an
import alias changes, or a runtime map accidentally treats `a.mass` and
`b.mass` as the same value.

Read the pipeline as a sequence of practical questions:

1. **Parser / AST: what did the user write?**
   The AST keeps source spans, punctuation-sensitive shapes, and surface syntax
   needed by diagnostics, formatting, and the LSP. At this point `foo.bar` is
   still just a path written in the file.
2. **Desugared AST: what is the simpler form of that syntax?**
   Parser conveniences such as multi-declarations and table literals are
   expanded so later passes do not each need to understand every shortcut.
3. **Loader and `ModuleResolver`: which file or DAG does a module path mean?**
   The loader performs filesystem work and turns import/include paths into
   canonical `DagId`s, but it does not merge dependency ASTs into the root
   `File`. Each loaded file keeps its own `File<Desugared>`. The compiler-side
   `ModuleResolver` then answers semantic lookup questions without doing I/O.
   This keeps disk paths and import aliases out of the core compiler data.
4. **Unfrozen IR: what declarations does this DAG body contain?**
   The IR assembly stage groups the body into consts, params, nodes, asserts,
   and registry entries, still carrying syntactic bodies. This is where checks
   such as duplicate names, visibility, bindability, and declaration categories
   are easiest because they require seeing the whole DAG body. Include bodies
   are not copied or rewritten here: the project compiler records typed
   template-to-instance edges and importer-context bindings separately.
5. **HIR at the freeze boundary: what does each reference point to?**
   `UnfrozenIR::freeze` is the single resolution stage of the pipeline. It
   lowers every assembled declaration body to HIR, classifying and resolving
   each reference path in one pass: declarations, dimensions, indexes,
   constructors, locals, generic params, and built-ins. After this boundary,
   code should not need to ask whether the string `"sum"`, `"PI"`, or
   `"helpers.physics.mass"` has a special meaning; it should pattern-match on
   typed values instead. The frozen `HirDag` carries its canonical `DagId` and
   no syntax-AST expression. Omitted dimension and unit exponents are normalized
   to exact `Rational::ONE` values at this boundary; only syntax AST retains
   `Option<Rational>` so source-aware tools can distinguish `m` from `m^1`.
6. **TIR: is the program type- and dimension-correct?**
   TIR combines the declaration structure from HIR with its bodies, resolved
   type expressions, dimension facts, domain constraints, inline DAG bodies, and
   dependency DAGs. The dependency graph is derived from the HIR bodies. This
   is the checked program representation used to prepare execution. The
   checker orders it once: one constant schedule for the file's local DAGs and
   one runtime schedule per callable DAG and its include closure.
7. **ExecPlan: what exact work should runtime evaluation do?**
   Execution checking evaluates constants in the checked constant schedule,
   resolves domain constraints, and seals the `CheckedTir` together with those
   facts into a `CheckedProgram`; sealing is its only construction. The
   `ExecPlan` borrows that program, and execution selects each DAG with its
   own facts as one `SealedDag` from it, so a body and its facts can never
   come from different checks.

Some names in this pipeline can sound misleading if read too literally:

- `UnfrozenIR` is syntactic declaration assembly, not another copy of the AST.
- HIR is "high-level" because it keeps expression/type tree shapes. Each
  `HirDag` owns one canonically identified DAG module, while `HirProject` is the
  complete project-level phase value that owns those modules before checking.
- TIR is not just HIR with types; it is the checked, per-DAG program model.

`DagTIR` keeps the declaration records (with their HIR bodies) and
`DagSemanticBody`, which holds facts derived from them: dependencies, domain
bounds, dynamic units, type definitions, and declaration bindings, keyed by
canonical `ResolvedName`/`DagId` identities. A `CheckedDag` pairs one `DagTIR`
with everything its check published: the checked typed tree (`TExpr`) of
every expression root, presentation facts, and its runtime schedule.

Every `.gcl` file moves forward through the same core stages:

```text
Source text
  |
  |  crates/graphcal-compiler/src/syntax/lexer.rs
  v
Tokens with Span
  |
  |  crates/graphcal-compiler/src/syntax/parser/
  v
File<Raw>
  |
  |  crates/graphcal-compiler/src/desugar/
  v
File<Desugared>  (the final syntax-AST phase)
  |
  |  crates/graphcal-compiler/src/ir/          (declaration-shell checks,
  |  crates/graphcal-compiler/src/resolve/     assembly)
  v
UnfrozenIR + ModuleResolver
  |
  |  UnfrozenIR::freeze — the single resolution stage
  |  crates/graphcal-compiler/src/hir/
  v
HirDag  (one canonically identified DAG module)
  |
  |  crates/graphcal-project/src/project_compiler/pipeline.rs
  v
HirProject  (every file-root and inline `HirDag`; no checked/runtime facts)
  |
  |  crates/graphcal-project/src/project_compiler/checking.rs
  |  crates/graphcal-compiler/src/tir/
  v
CheckedTir  (TirDraft -> InstantiatedTir -> CheckedTir; CheckedDag per body)
  |
  |  crates/graphcal-eval/src/execution_check.rs
  v
CheckedProgram  (CheckedTir + constant pools + resolved constraints;
  |              retained by graphcal-project's CheckedProject)
  |
  |  crates/graphcal-eval/src/exec_plan.rs
  v
ExecPlan
  |
  |  crates/graphcal-eval/src/eval/runtime.rs
  v
RuntimeEvaluation  (SI values + contained errors + root result)
  |
  |  crates/graphcal-project/src/prepare/prepared.rs
  v
EvalResult  (public project output assembly)
```

The pipeline is forward-only: parser sugar is removed before IR assembly,
reference paths are lowered through HIR to canonical owners at the freeze
boundary before TIR/eval, and runtime maps use owner-qualified declaration
identities. Source `ScopedName`s
and spans are kept at diagnostics and formatting boundaries; semantic
compile/eval decisions use HIR and `ResolvedName`-based data.

### 1.1 AST Phases

The AST is parameterized by a `Phase` marker in
`crates/graphcal-compiler/src/syntax/phase.rs`.

```text
File<Raw> -> File<Desugared>
```

The marker controls only the slots that actually vary by phase:

| Slot        | Raw             | Desugared       |
| ----------- | --------------- | --------------- |
| `DeclSugar` | `RawDeclSugar`  | `Infallible`    |
| `ExprSugar` | `RawExprSugar`  | `Infallible`    |

References are `UnresolvedRef` paths in both phases, and type-level syntactic
references such as type applications, dimension terms, and index expressions
are `NamePath` in every phase, so they are represented directly as fields
rather than as `Phase` associated types.

`File<Raw>` is produced by the parser and consumed by surface-aware tooling such
as the formatter. `File<Desugared>` has no multi-decl or table-literal sugar
and is the final syntax-AST phase: expression references stay syntactic
(`ExprKind::UnresolvedRef` paths, plus parser-produced `GraphRef`s), and
type-level paths, index paths, and match patterns are intentionally preserved
as structured paths. HIR lowering is the single stage that classifies and
resolves all of them against the lexical scope and the module resolver.

The alias module keeps signatures readable:

- `desugar/desugared_ast.rs` pins AST aliases to `Desugared`.

When a post-desugar match sees an impossible payload, use
`syntax::phase::never(x)` rather than a runtime `unreachable!()`.

### 1.2 Lexing and Parsing

The lexer is `logos`-based and produces tokens with byte-accurate `Span`s.
The parser is recursive descent with Pratt-style expression precedence.

Key files:

- `syntax/token.rs` defines the token vocabulary.
- `syntax/lexer.rs` wraps tokenization and lookahead.
- `syntax/parser/expr.rs` parses expressions.
- `syntax/parser/type_expr.rs` parses type and dimension expressions.
- `syntax/parser/decl/` contains per-declaration parsers.
- `syntax/parser/table.rs` parses table-literal surface syntax. Its cardinality
  diagnostics retain declared and parsed counts as `u64`, including on 32-bit
  wasm targets.

The parser emits `File<Raw>`. Declarations carry typed definition leaves where
possible (`DeclName`, `DimName`, `UnitName`, etc.), a `Visibility`, attributes,
a `DeclKind<Raw>`, and a source span. Reference positions that may be qualified
are parsed as `IdentPath`/`NamePath`, not as namespace-specific leaf names.
Match arms start as `MatchPattern::Path`; semantic categorization into
constructor patterns versus index-label patterns happens only when a resolver
has enough information to prove the kind. Quoted datetime arguments remain
string syntax until `datetime_literal.rs` classifies them; civil coordinates
must carry an explicit time, so the semantic boundary never synthesizes
midnight.

### 1.3 Desugaring

Desugaring converts parser-only constructs into canonical AST forms:

- Multi-declarations expand into ordinary `param`, `node`, or `const node`
  declarations.
- Table literals become `ExprKind::MapLiteral`.

The `Raw` to `Desugared` lift is a set of `From` impls (and a derived
`TryFrom` for `DeclKind`, which hands declaration sugar back). Structural
pass-throughs are generated by `#[derive(PhaseLift)]` from the
`graphcal-ast-derive` proc-macro crate; `desugar/convert.rs` holds the
conversions with real logic (declaration expansion, expression sugar
lowering, and the stack-guarded `Expr` conversion).
The multi-declaration expander lives in `desugar/multi.rs`.

### 1.4 Module Resolver and HIR Boundary

`resolve/` builds project-wide, owner-qualified symbol tables. It consumes the
desugared AST, so it sits after both `syntax/` and `desugar/`. It stores, per canonical `DagId`, one `ModuleSymbols` table and one
`ModuleScope` for import/include aliases and selective imports. It resolves
syntactic `NamePath` / `IdentPath` values into canonical
`ResolvedName<namespace::...>` values or `ResolvedIndexVariant` values
(`resolved_name.rs`, beside `dag_id.rs`). Source
qualifier text is used only to find a scoped module/DAG binding; successful
lookups carry the canonical `DagId` owner. For inline-call paths, an imported
alias resolves to its exact file-root or inline-DAG target and is itself
callable; additional segments descend through child DAG modules.

`graphcal-project/src/loader.rs` remains the only layer that resolves import paths
to files/DAGs. This is physical project loading and path resolution, not
semantic AST merging: each loaded file owns its own `File<Desugared>`, and inline
`dag` bodies are lifted into `LoadedDag`s. `LoadedProject::build_module_resolver()`
hands those already resolved edges to the pure compiler resolver, including
inline DAGs and instantiated include owners.

`crates/graphcal-compiler/src/hir/` is the semantic boundary after syntax and
the single resolution stage of the compiler:

- `hir/types.rs` models type expressions using `BuiltinType`,
  `ResolvedName`, normalized Nat forms, and lexical `GenericParamId`s.
  Type-level Nat expressions are normalized to `NatPolyForm` during HIR
  lowering, so later phases never see Nat syntax.
- `hir/expr/` models value expressions using canonical declaration refs,
  constructor refs, `ResolvedIndexVariant`s, `LocalId`s, typed built-ins, and
  semantic `MatchPattern` variants (`model.rs`), with structural traversal
  (`visit.rs`), finished bodies (`checked.rs`), and the evaluation-time local
  environment (`local_env.rs`). Trees are parameterized by completeness
  (the `Completeness` trait, also in `model.rs`): `Expr<Strict>` (the default) cannot represent an error
  node and carries an `ExprId` on every node; `Expr<Draft>` is complete but
  unnumbered (strict lowering and synthesized trees); `Expr<Tolerant>`
  (`hir/expr_lower/tolerant.rs`) keeps each unresolved reference as an error
  node carrying its diagnostic for the LSP. Strict lowering refines tolerant
  into draft, and finishing a body numbers draft into strict (`refine.rs`),
  so no consumer handles an unresolved node or a missing identity. The lowerer in
  `hir/expr_lower/` (walk in
  `lower.rs`, name resolution in `resolve.rs`, call and datetime-literal
  lowering in `call.rs`) consumes desugared expressions
  directly and classifies every reference path in one pass: lexical locals,
  built-in constants and time scales, constructors, type-system names, and
  declarations — resolving each to its canonical identity at
  the same time. Power operators also retain the parser's typed
  `PowerExponent` classification, backed by `ExactRational`, so checking and
  evaluation never infer an exact exponent from binary64 or source text.
  Strict expression/assertion lowering seals occurrence IDs and a separate
  source map before returning its read-only body. `expression_id.rs` provides
  allocation-scoped revision identity and body-local ordinals;
  `expression_source.rs` rejects duplicate IDs and cross-revision lookups.
  Expression semantics are private behind read-only access; consuming and
  reconstructing a node cannot retain its old identity. Cloning an immutable
  expression preserves its identity; fresh lowering creates a fresh revision
  even when all source coordinates agree. Finished expressions/assertions share
  immutable bodies and source maps; expression handles box their variant payload.
  Occurrence ordinals are `usize` positions, not language Nat values or wire IDs.
  A single exhaustive child inventory
  serves identity assignment, dependency collection, and structural inspection.
  DAG root enumeration distinguishes bounds owned by the current semantic body
  from referenced foreign nominal bounds, while dependency inspection still
  visits both.
  Presentation selections travel with concrete values, not static call-site keys.
  Checking turns each expression root into one typed tree (`tir/texpr/`,
  `TExpr`): every node carries its `CheckedType`, a constructor its
  `ConstructorApplication`, a match arm its constructor target, and a
  statically checked position its range proof. `CheckedBodies` publishes each
  root as executable or as deferred until Static or generic bindings fix its
  axes, and a semantic instance specializes its template's trees instead of
  re-inferring them. Runtime consumes these trees instead of resolving
  constructor generics again. Generic Nat discharge belongs only to checking and
  uses canonical parameter owners, never same-leaf matching. Interpretation has
  no Nat-binding store or source-Nat evaluation service.
- `hir/closed_expr.rs` validates the complete syntactically closed input-literal
  subset, rejects nonfinite/unresolved/computational nodes, and finishes a fresh
  source revision before parameter binding checks. `ClosedExpr` preserves this
  stronger invariant; it is not a type proof or a claim that referenced units
  require no runtime services.
- `hir/const_expr.rs` is the resolved form of the compile-time constant
  positions (unit scales, coordinate bounds, `linspace` point counts). A
  position marker makes rejected constructs unrepresentable, and one pure
  evaluator yields `PositiveFiniteScale` and validated `CoordinateIndexData`
  directly. `hir/const_lower.rs` lowers those positions from the desugared AST
  against the static unit registry and classifies a unit scale as static or
  graph-dependent.
- `hir/lower.rs` lowers syntax AST type references into HIR with a
  `ModuleResolver`, a `GenericScope`, and an optional prelude scope.

Expression lowering is diagnostic-accumulating: `lower_expr_tolerant` turns an
unresolvable reference into an explicit `hir::ExprKind::Error` node and records
the diagnostic, so IDE consumers keep working on incomplete code. The strict
`lower_expr` entry point returns `CheckedExpr`, and strict assertion lowering
returns `CheckedAssertBody`. These wrappers cannot be constructed from tolerant
HIR and expose no production mutation path, so batch declarations, bounds, and
visualization bodies cannot represent an error-node tree.

TIR stores HIR expressions and HIR-derived semantic metadata in
`DagSemanticBody`. Frozen declaration shells retain source-facing names, spans,
and provenance for diagnostics and editor presentation. Value annotations,
declaration bounds, nominal generic defaults, and nominal field signatures are
all already HIR.

### 1.5 IR Assembly and the Freeze Boundary

`ir/entry.rs`, `ir/model.rs`, `ir/lower.rs`, `ir/include.rs`,
`ir/static_definitions.rs`, `ir/extern_fns.rs`, and `ir/resolve/` assemble a
desugared AST into an `UnfrozenIR`. `UnfrozenIR::freeze` (`ir/freeze.rs`) then
lowers it into a `HirDag`. The files
separate the IR model/freeze boundary, semantic include-edge recording,
canonical Static definition evaluation, and extern signature resolution even
though they cooperate during assembly.

The assembly stage (syntactic, pre-resolution):

- Checks duplicate names, declaration naming rules, visibility, bindability,
  and attribute placement on declaration shells.
- Evaluates the module's dimensions, units, and indexes with the
  project-wide `StaticDefinitionEvaluator`: every name in a `dim`, `unit`, or
  `index` declaration resolves through the module resolver (prelude as
  fallback), and each canonical identity is evaluated once on demand, so no
  source-name keyed table is merged across modules.
- Carries import metadata and canonical `ResolvedDeclName` targets across
  file/DAG boundaries; checked types and optional constant values do not exist
  at this phase.
- Records semantic include edges with typed Static substitutions, explicit
  output/assertion/plot projections, and importer-context value bindings. The
  independently checked template body is not inserted into the importer.

The freeze boundary (`UnfrozenIR::freeze(owner, definitions, src)`):

- Lowers every `type` declaration of the module once (`hir/nominal_lower.rs`,
  the single validation point of generic parameter lists, constructors, and
  payload fields). A type that a selective include projects under Static
  bindings is lowered from the template's declaration in the template's scope
  and specialized through that include's canonical substitution.
- Lowers every nominal generic default, nominal field annotation/bound,
  const/param/node type annotation, declaration bound, and
  const/param/node/assert body to HIR (strict — an unresolvable reference fails
  the compile with a spanned definition-source diagnostic).
- Strictly lowers every plot/figure/layer expression; tolerant lowering is
  reserved for editor-facing incomplete buffers.
- Emits the owner-qualified `ModuleDefinitions` (dimensions, units, indexes,
  base-dimension metadata, and the canonical `NominalTypeRegistry`).
  Canonical definitions and typed child-DAG identities are the only
  authorities available after this point.

One `HirDag` represents one DAG body and stores its own canonical identity:
either a file root or an inline `dag` block. `HirProject` owns exactly one such
frozen module for every loaded file root and inline DAG, stored as a
`DependencyOrdered<HirFile>` (dependencies first, root last by construction),
plus a narrow borrow of loader-owned plugin verification inputs; the loaded source AST arena does not cross this boundary.
Entry names stay source-shaped `ScopedName`s for presentation, but
value-declaration signatures, nominal definitions, and bodies are HIR;
owner-qualified declaration dependencies are collected from those HIR bodies
during TIR construction. TIR resolves and validates HIR nominal signatures; it
never lowers their syntax AST. Scope policies that need resolved
references (no runtime `@` in `const node` bodies, no `@assert` references, A10
variant literal rules) run when `HirProject` is consumed by checking.

### 1.6 TIR and Dimension Checking

`tir/typed.rs` resolves type annotations into semantic type expressions.
`tir/materialized_shape.rs` defines the checked total-cardinality policy carried
by concrete indexed expressions. `tir/texpr/` defines the checked typed trees
and `tir/static_index.rs` the static membership proofs they rely on.
`tir/presentation.rs` retains checked plot-channel shapes;
there is no second tree containing presentation selector HIR. `tir/dim_check/`
infers and installs concrete value types, eager shape facts, and plot shapes
before evaluation. The interpreter records selected value-shaped presentation
evidence while consuming these checked expressions. `expression_axes.rs` derives axes from
checked declared types. `concrete_obligations.rs` specializes published nominal
bound products, including scalar-result obligations, without raw-bound inference.
Constructor preparation uses the complete owned-root inventory, including
nominal bounds in their canonical defining (possibly imported) scope.

In the module-aware project path, TIR resolution receives both a
`ModuleResolver` and the project-wide `ProjectTypeStore`. The resolver maps
source aliases to canonical identities; the store is the authoritative lookup
for owner-qualified dimensions, units, declared and structural finite indexes,
nominal types, and constructors. Value-declaration bodies, type annotations,
and domain bounds arrive already lowered to HIR.

TIR construction is explicitly two-pass within each physical file. The first
pass consumes every root and inline `HirDag` into a `SignatureResolvedHirDag`,
which owns both the HIR body and its one resolved declaration interface. Once
all local interfaces exist, the second pass attaches each checked
`ImportedBinding` and resolves bodies from those exact signature values. A
signature therefore cannot be paired with another body, and declaration type
expressions are not resolved again during body construction. TIR validates that
no lexical key or canonical import target changed across the boundary;
declaration bounds then move into checked semantic storage without another
lowering pass.

The TIR is a typestate, and it is not flat:

```text
TirDraft -> InstantiatedTir -> CheckedTir   // tir/typed/program.rs, checked.rs

CheckedTir
  core
    registry: FormattingRegistry  // diagnostics + timezone validation only
    project_types                 // canonical owner-qualified definitions
    runtime_units, extern_functions
  dags: CheckedDagRegistry        // root, local, then imported DAGs, each at a DagPosition
  const_schedule
```

Each file root and inline `dag` body is represented by a `DagTIR`. Dependency
files are not merged at the AST stage; dependency DAG TIRs are registered in
the same project TIR during lowering/finalization using their canonical
`DagId`s. Each reusable template is checked once. V007 checks executable bodies
with bindable Static ports rigid, while V005 separately records reconciliation
obligations for parameter defaults. A semantic include then materializes a
checked instance DAG with a concrete owner, typed Static substitution, value
bindings, and explicit value/assertion/plot projections; it never copies or
rewrites syntax expressions. A defaulted dimension port
(`pub(bind) dim Q = Length;`) resolves to its default in the template's own
signatures and facts, so an instance that rebinds it specializes the
template's port-generic view instead: static definitions keep each dimension's
value with the module's defaulted ports opaque, and
`ProjectTypeStore::with_rigid_dimensions` rebuilds signatures, facts, and plot
shapes with the bound ports rigid (the same view V007 checks).

Each category-specific declaration record on `DagTIR` owns its strictly lowered
HIR body exactly once (`ConstEntry.expr`, `ParamEntry.default`, `NodeEntry.expr`,
`AssertEntry.body`, and plot/figure/layer bodies). A parameter default is an
optional `CheckedExpr` on its entry, so the default cannot drift from the
declaration across include rewrites.
A private declaration index maps canonical IDs to those records; no semantic
side map clones bodies.
`DagSemanticBody` contains derived facts only:

- `semantic.dependencies`: owner-qualified declaration dependency maps.
- `semantic.domain_bounds`: checked, unevaluated HIR bound expressions, a
  non-empty list per constrained declaration.
- `semantic.dynamic_unit_scales`: runtime-dependent unit definitions with
  their validated dimensions and HIR scale expressions.
- `semantic.type_defs`: resolved field/default semantics plus shared handles to
  canonical nominal definitions.
- `semantic.decl_bindings`: declaration records and visible imported values
  mapped from source-facing `ScopedName` keys to authoritative
  `ResolvedName<Decl>` identities. The local part is the HIR `DeclTable`
  spelling index, taken over rather than rebuilt. Unknown-name diagnostics use
  `DeclarationIdentityLookup::DiagnosticProbe`; they never synthesize a
  resolved identity from the current DAG owner.

The checked typed trees are not part of `DagSemanticBody`. Checking publishes
them per body as `CheckedBodies` (`tir/texpr/checked_bodies.rs`) on the
`CheckedDag`, next to the body's presentation facts and runtime schedule. A
semantic instance's trees are its template's, specialized with the instance's
Static substitution (`tir/dim_check/body_specialization.rs`); only its own
replacement bindings are inferred afresh.

Collection and index semantics call the TIR's `index_def`, which resolves both
owner-qualified declared indexes and concrete `Fin(N)` identities through the
project type store. `DagSemanticBody` has no duplicate per-DAG index-definition
cache or expression walk.

Dimensions are exact exponent maps:

```text
Dimension = BTreeMap<BaseDimId, Rational>
```

Dimension inference is split by expression families under
`tir/dim_check/infer/` and operates on HIR expressions. Every
`DimCheckContext` carries its concrete `DagTIR`; checking cannot run in a
context that lacks canonical index/constructor/inline-DAG ownership.
Function-signature checking records each dimension-variable binding together
with the parameter that bound it, so mismatch help names that parameter and
never fabricates one. Index-access checking classifies structural `Fin` identities separately
from declared axes: structural cardinality forms need no registry entry, while
a declared axis missing its semantic definition is an internal error instead of
disabling `Fin(N) <= Fin(M)` widening.

### 1.7 Execution and Runtime Evaluation

Execution checking (`graphcal-eval/src/execution_check.rs`) evaluates
`const node` declarations in the checker's constant schedule and resolves
top-level and struct-field domain constraints once, sealing those immutable
stores with the `CheckedTir` into the `CheckedProgram` that the checked
project retains. Runtime preparation reuses the same stores and the checker's
topological `param`/`node` schedule of each callable.

Runtime values are keyed by canonical `ResolvedDeclName`
(`ResolvedName<Decl>`) identities so same-leaf declarations from different DAGs
do not collide. Checked bodies, however, name declarations and units by
DAG-relative handles (`LocalDecl`, `LocalUnit`), because a template and all of
its instances share one body. The evaluator never chooses the DAG a handle
resolves in: `CheckedTir::declaration_body`, `unit_scale_body`, and
`nominal_type_body` hand out each evaluation unit already bound to the scope of
the DAG that owns it (`Scoped` / `ScopedTree`,
`tir/typed/evaluation_unit.rs`), and the kernel walks a tree only as
`ScopedNode`s, whose children and references come out resolved in that scope
(`tir/typed/scoped_node.rs`). `EvalSession` (`eval_expr/context.rs`) carries
everything else evaluation needs and has no scope of its own. The kernel
(`eval_expr/hir_eval.rs` and its operation families) executes the checked
`TExpr` trees directly; there is no separate execution IR. Assertions and
visualization declarations are read from the checked DAGs, not copied into
`ExecPlan`.

`eval/runtime.rs` runs the root's plan through the shared frame machine
(`execution_frame.rs`). A failed node is contained as a typed `NodeUnavailable`
reason; independent nodes can still evaluate. A violated evaluator invariant is
a `Failure::Invariant` (`invariant.rs`) and cancellation is
`Outcome::Cancelled`; neither is an ordinary evaluation failure. One
`RuntimeEvaluation` retains the SI-normalized value map, contained-error map,
and display-aware root result together. `PreparedProject` (in
`graphcal-project`) then assembles the normal project-level `EvalResult`.
Reasons in the output name declarations as the root's source does
(`eval/output_decl_name.rs`), never by internal runtime identity.

`graphcal dump` is deliberately only a debugging shell: each stage stops at an
existing pipeline boundary and pretty-prints that boundary's Rust `Debug`
representation. It does not define a serialization schema or a parallel
inspection model. `dump hir` prints the production `HirProject` continuation;
static checking consumes that same value to produce `CheckedProject`.

## 2. Workspace Map

The workspace contains eighteen Rust crates:

```text
graphcal-cli           binary/library: CLI shell
graphcal-lsp           binary/library: Language Server Protocol
graphcal-project       loader, project compiler, runtime preparation, output
graphcal-eval          interpreter: sealing, execution plans, evaluation
graphcal-compiler      syntax, semantic core, HIR, IR, TIR
graphcal-ast-derive    AST derive proc-macros for the compiler
graphcal-ratio         reduced rational numbers (dimension exponents)
graphcal-fmt           formatter
graphcal-io            filesystem abstraction
graphcal-package       pure package manifest/lockfile domain model
graphcal-report        report rendering and browser assets
graphcal-wasm          browser/Wasm project adapter
graphcal-tenax         Tenax model adapter
graphcal-plugin-abi    plugin signatures, manifests, and wire contract
graphcal-plugin-macros plugin authoring proc-macros
graphcal-plugin        plugin authoring SDK
graphcal-plugin-host   plugin acquisition and Wasm execution adapter
graphcal-test-support  shared testing utilities
```

Some principal production dependencies are shown below (not an exhaustive
Cargo graph). `A -> B` means that A consumes B:

```text
graphcal-cli
  -> graphcal-project
  -> graphcal-eval
  -> graphcal-fmt
  -> graphcal-io
  -> graphcal-package

graphcal-project
  -> graphcal-eval
  -> graphcal-compiler
  -> graphcal-io
  -> graphcal-package

graphcal-eval
  -> graphcal-compiler

graphcal-lsp
  -> graphcal-project
  -> graphcal-eval
  -> graphcal-compiler

graphcal-fmt
  -> graphcal-compiler

graphcal-compiler
  -> graphcal-ast-derive   # compile-time only: AST derive proc-macros
  -> graphcal-ratio

graphcal-io
  -> graphcal-compiler     # cancellation token and `Outcome`

graphcal-plugin-macros
  -> graphcal-plugin-abi
  -> graphcal-ratio        # shared with the compiler; no compiler dependency

graphcal-package   # no Graphcal-internal crate dependencies
```

### 2.1 `graphcal-compiler`

The compiler crate owns the functional core through TIR.

| Path                          | Purpose                                                       |
| ----------------------------- | ------------------------------------------------------------- |
| `syntax/format_equivalent.rs` | `FormatEquivalent` trait (formatter's AST-preservation check), leaves, containers |
| `syntax/ast.rs`               | Phase-parameterized AST aggregate and re-exports              |
| `syntax/ast/common.rs`        | Shared AST nodes and typed common fields                      |
| `syntax/ast/value.rs`         | Expression/value AST definitions, with their non-structural format-equivalence impls |
| `syntax/ast/decl.rs`          | Declaration AST definitions                                   |
| `syntax/ast/plot_props.rs`    | Syntax-level plot/figure/layer property names                 |
| `plot_props.rs`               | Semantic plot/mark/composition property registry              |
| `plot_visibility.rs`          | Standalone vs composition-only plot output                    |
| `syntax/phase.rs`             | `Raw`, `Desugared`, sugar/path slots, `never`                 |
| `syntax/names.rs`             | `NameAtom`, typed name newtypes, and source paths             |
| `generic_param.rs`            | Owner-qualified `GenericParamId` of lexical generic parameters |
| `nat.rs`                      | Normalized type-level Nat polynomials over `GenericParamId`s  |
| `dag_id.rs`                   | Filesystem-independent DAG identity                           |
| `resolved_name.rs`            | `ResolvedName<Ns>` (canonical owner + leaf) and its aliases   |
| `import_cycle.rs`             | Typed import chain reported for circular imports              |
| `declaration_category.rs`     | Source-order declaration categories independent of collection |
| `dependency_graph.rs`         | `DependencyGraph<K>` → deterministic `TopoOrder<K>` or `Cycle<K>` |
| `assertion_expectation.rs`    | Phase-parameterized assertion selectors and semantic key matching |
| `syntax/parser/`              | Parser for declarations, expressions, types, tables           |
| `resolve/`                    | Owner-qualified module symbol tables and path resolution      |
| `desugar/`                    | Sugar-lowering phase conversions and the `Desugared` AST alias |
| `hir/`                        | Canonical semantic type/value expressions and lowering boundary |
| `hir/source_interface.rs`     | Direct parameter/node/index provenance retained through HIR     |
| `hir/nominal_lower.rs`        | `type` declaration lowering, validation, and specialization     |
| `ir/static_substitution.rs`   | Canonical Static substitution and specialization identity      |
| `ir/instance.rs`              | Typed template-instance edges and binding environments         |
| `ir/override_reconciliation.rs` | Canonical include-override obligations of unrebound defaults |
| `ir/entry.rs`                 | Phase-indexed value/sink declaration entries (`Syntax` bodies) |
| `ir/decl_table.rs`            | `ResolvedDeclName`-keyed declaration table with spelling index |
| `ir/model.rs`                 | IR model: lowered body phase, `HirDag`, and `UnfrozenIR`       |
| `ir/lower.rs`                 | Per-DAG lowering and assembly coordination                     |
| `ir/include.rs`               | Include assembly and typed pre-freeze substitution             |
| `ir/freeze.rs`                | Strict HIR freeze boundary of an assembled `UnfrozenIR`        |
| `ir/module_definitions.rs`    | Owner-qualified definitions one module contributes             |
| `ir/static_definitions.rs`    | Demand-driven canonical dimension/unit/index evaluation        |
| `ir/extern_fns.rs`            | External plugin signature resolution                           |
| `ir/required_bindability.rs`  | Pure V002 required-interface validation                       |
| `ir/resolve/`                 | Declaration-shell collection and validation                   |
| `semantic/`                   | Checked types, indexes, unit scales, time, built-in catalog   |
| `display/`                    | Number and unit-label formatting, `FormattingRegistry`, include-scope names |
| `diagnostic.rs`               | Source-located diagnostics as plain core data (`Diagnostic<K>`) |
| `semantic_error/`             | `SemanticError`: typed diagnostic families, or an internal error |
| `internal_error.rs`           | `InternalError` (`X001`); `InternalError::new` is its only constructor |
| `source_id.rs` / `source_registry.rs` | Opaque source identity in the core; shell-side registry that renders it |
| `cancellation.rs` / `outcome.rs` | `CancellationToken` and `Outcome<E>`, which keeps cancellation out of error types |
| `node_unavailable.rs`         | Typed reasons a declaration has no evaluated value            |
| `hir/expr/local_decl.rs`      | `LocalDecl`: DAG-relative declaration handles in checked bodies |
| `ir/instance/frame.rs`        | `InstanceFrame`: how the DAG running a shared body names its declarations |
| `tir/materialized_shape.rs`   | Checked total cardinality for eagerly materialized indexed values |
| `tir/texpr.rs` / `tir/texpr/` | Checked typed trees (`TExpr`), `CheckedBodies`, and DAG-relative `CallSlot`s |
| `tir/static_index.rs`         | Static membership proofs, independent of result shape          |
| `tir/dim_check/expression_axes.rs` | Cardinalities and shapes from checked declared types |
| `tir/dim_check/body_specialization.rs` | Specializing checked typed trees without re-inferring their bodies |
| `tir/dim_check/concrete_obligations.rs` | Concrete application validation using published bound facts |
| `tir/presentation.rs`         | Checked plot-channel shape tables                            |
| `tir/schedule.rs`             | Checker-built constant and per-callable runtime schedules     |
| `tir/dim_check/schedules.rs`  | Declaration scheduling and cyclic-dependency diagnostics      |
| `tir/typed.rs`                | Typed semantic bodies, including atomic dynamic-unit entries   |
| `tir/typed/model.rs`          | Per-DAG typed model (`DagTIR`) and the project type store      |
| `tir/typed/module_type_context.rs` | Module-aware type-resolution context for one DAG body |
| `tir/typed/program.rs`        | DAG registry and the draft/unchecked/instantiated TIR states    |
| `tir/typed/checked_dag.rs`    | A checked DAG body with the facts its check published          |
| `tir/typed/checked.rs`        | The checked project TIR (`CheckedTir`) and its call-closed `CheckedDagRegistry` |
| `tir/typed/dag_position.rs`   | `DagPosition`: a DAG's deterministic position in its program's registry |
| `tir/typed/declaration_view.rs` | Declarations of a checked DAG as seen outside the compiler, without HIR bodies |
| `tir/typed/evaluation_unit.rs` | Evaluation units handed out bound to their owner's scope (`Scoped`, `ScopedTree`) |
| `tir/typed/scoped_node.rs`    | Scoped traversal of executable trees (`ScopedNode`)            |
| `tir/typed/dag_store.rs`      | The immutable store of checked bodies one module publishes     |
| `tir/typed/freeze.rs`         | Freezing a checked TIR's local bodies into a `DagStore`        |
| `tir/typed/substitution.rs`   | `Substitution` of generic parameters and its single type fold  |
| `tir/dim_check/`              | Dimension/type inference, including scalar unit-scale checks   |
| `tir/dim_check/plot.rs`       | Plot/figure/layer dimension validation                        |
| `tir/dim_check/presentation.rs` | Publication of checked plot-channel shapes                 |

### 2.2 `graphcal-eval`

The interpreter crate. It seals a checked TIR into a `CheckedProgram` by
evaluating its constants and resolving its domain constraints, prepares the
indexed `ExecPlan`, and executes it. It depends only on `graphcal-compiler`;
project loading and compilation live in `graphcal-project`, whose tests enable
the `test-internals` feature to reach interpreter test hooks. Single-file
interpreter tests live in this crate (`interpreter_tests.rs`), over checked
TIRs built by `test_tir.rs`.

| Path                              | Purpose                                                        |
| --------------------------------- | -------------------------------------------------------------- |
| `execution_check.rs` / `execution_check/` | Constant evaluation, domain resolution, and sealing into a `CheckedProgram` |
| `checked_program.rs`              | `CheckedProgram` and `SealedDag`: a checked TIR sealed with its per-DAG constants, constraints, and source |
| `runtime_value.rs` / `runtime_value/` | Internal `RuntimeValue`, index axes, keys, dense arrays, and struct values |
| `invariant.rs`                    | `Invariant` / `Failure<E>`: violated invariants kept apart from user-facing failures |
| `host_abi.rs` / `host_abi/`       | Validated host-function arguments and marshalling across the host ABI |
| `static_incompleteness.rs`        | Value-independent TODO reachability across call outputs        |
| `presentation_evidence.rs`        | Selected value-shaped display data and typed presentation diagnostics |
| `runtime_presentation.rs`         | Atomic interpreter transport of values and selected evidence |
| `constant_pools.rs` | Shared constant-pool views and validated imported references |
| `execution_plan.rs`     | Immutable indexed callable plans and steps borrowing a sealed program |
| `execution_frame.rs` | Shared binding, dependency, domain, and insertion machine with fixed frame policy |
| `assertion_eval.rs` | Assertion semantics over an expression callback, independent of root/call reporting |
| `exec_plan.rs`          | Checked-fact validation and execution-plan preparation       |
| `domain_constraint.rs`  | Family-preserving evaluated bounds and validated same-scale instants |
| `domain_check.rs`       | Runtime and compile-time value validation against those contracts |
| `eval/runtime.rs`       | Root evaluation and staged public result assembly             |
| `eval/runtime/root_loop.rs` | The root's plan run by the shared frame machine (the evaluation loop) |
| `eval/runtime/evaluated_root.rs` | What one root run evaluated, borrowed by the assembly stages |
| `eval/runtime/declaration_body.rs` | Checked-body lookup of a declaration identity            |
| `eval/runtime/dependency_failures.rs` | `dependency failed: ...` reports for failed reads     |
| `eval/output_decl_name.rs` | The names an evaluation's output gives the declarations it reports |
| `eval_expr/context.rs`  | Immutable phase-selected environments and checked scope transitions |
| `pipeline_metrics.rs`  | Test-only observations of copying, planning, resolution, and presentation work |
| `eval_expr/presentation.rs` | Deliberate selected display computations in live owning frames |
| `eval/plot_data.rs`     | Runtime plot/figure/layer data extraction                     |
| `eval/public_projection.rs` | One-walk presented-value-to-public projection, preserving SI on display failure |
| `eval/types.rs`         | Public `EvalResult`, `Value`, plot/assert result types        |
| `eval_expr/`            | HIR expression evaluation kernels by expression family        |
| `eval_expr/numeric.rs`  | Shared checked numeric helpers for expression evaluation      |
| `eval_expr/work_budget.rs` | Declaration-scoped kernel budgets and cancellation cadence  |
| `eval_expr/unit_scale.rs` | Dynamic unit-scale resolution (re-entering the kernel it is handed) and finite-quantity validation |
| `eval_expr/aggregations.rs` | Aggregation built-ins such as sum/mean/min/max/count     |
| `eval_expr/conversions.rs` | Unit/type conversion helpers                              |
| `eval_expr/hir_eval.rs` | Expression kernel over checked typed trees, walked as `ScopedNode`s |
| `eval_expr/operations.rs` | Checked operations, each evaluated by its operand family's kernel |

`eval/public_projection.rs` projects a presented value (value and
presentation in one tree) together with its `CheckedType` in one walk. Struct
values carry their constructor's instantiated field types
(`semantic/applied_constructor.rs`), so nothing is looked up; the checked type
only supplies what runtime scalars do not carry (dimension, time scale), and a
runtime variant that differs from it is an internal error.

### 2.3 `graphcal-project`

The project crate contains the loader shell, the compiler-facing pure project
checking layer, runtime preparation, and public output assembly over the
`graphcal-eval` interpreter. The crate root only declares modules: `CompileError`
lives in `compile_error.rs`, `ProjectCompiler` and `CheckedProject` in
`project_compiler/`, and `PreparedProject` with the `compile_and_eval*` entry
points in `prepare/`.

| Path                              | Purpose                                                        |
| --------------------------------- | -------------------------------------------------------------- |
| `compile_error.rs`                | `CompileError`: parse, load, binding, rendered semantic, and external-binding failures |
| `load_error.rs` / `binding_error.rs` | Typed loader and parameter-binding failures                 |
| `dependency_ordered.rs`           | `DependencyOrdered<T>`: dependency-first modules plus the root  |
| `loader.rs`                       | The IO shell that fetches a `SourceSnapshot` and builds a `LoadedProject` |
| `loader/module_path.rs`           | `ModulePathKey` and the module targets the loader resolves them to |
| `loader/loaded_file.rs`           | `LoadedFile` / `LoadedDag` / `LoadedModule` loaded-source records |
| `loader/loaded_project.rs`        | `LoadedProject` snapshot: loaded files, plugin artifacts, package closure |
| `loader/budget.rs`                | Per-load byte and file budgets and shared loader errors          |
| `loader/budget_violation.rs`      | Typed report of one exhausted loader budget                      |
| `loader/source_snapshot.rs`       | `SourceSnapshot`: parsed files plus span-free recorded path resolutions |
| `loader/build.rs`                 | Pure `build_loaded_files(snapshot)`: load order, cycles, loader-resolved module edges |
| `loader/inline_dags.rs`           | Inline-DAG lifting shared by every loader mode                  |
| `project_compiler/session.rs`     | Configurable `ProjectCompiler` builder                          |
| `project_compiler/checked_project.rs` | `CheckedProject`: the reusable checked semantic boundary     |
| `project_compiler/imports.rs`     | Compile-time imports and typed instance requests                |
| `project_compiler/including_module.rs` | Importer side of one include: interface, source, static scope |
| `project_compiler/binding_values.rs` | Importer-side values of type-level include bindings           |
| `project_compiler/generic_leakage.rs` | Include-boundary generic visibility checks                  |
| `project_compiler/template.rs`    | Canonical shared pre-HIR template store                         |
| `project_compiler/entry_interface.rs` | Checked syntax-independent entry-DAG runtime interface      |
| `project_compiler/model.rs`       | Internal typed pass artifacts and instance requests             |
| `project_compiler/hir_project.rs` | Complete resolved-project phase value                            |
| `project_compiler/lowering.rs`    | Pure per-file and inline-DAG HIR elaboration                     |
| `project_compiler/module_resolve_errors.rs` | Project diagnostics for module-resolution failures     |
| `project_compiler/checking.rs`    | HIR-to-TIR interface resolution and mandatory static checks      |
| `project_compiler/pipeline.rs`    | Dependency-ordered HIR lowering and checking continuations       |
| `prepare/model_schema.rs`    | Finite typed arena for recursive model-value definitions        |
| `prepare/prepared.rs`        | Checked-to-prepared transition and reusable evaluation surface  |
| `prepare/binding_compile.rs` | Closed external-value lowering and binding-row validation       |
| `prepare/tenax_model.rs`     | Generic model and strict Tenax-v2 projection                    |
| `prepare/output.rs`          | Presentation-only output projection                            |
| `inline_dag.rs`                   | Inline-DAG self-import preprocessing                            |
| `graph_ir/`             | Dependency-graph export model and DOT rendering               |
| `prepare/mod.rs`                  | `prepare_from_project` and the `compile_and_eval*` entry points |

### 2.4 `graphcal-report`

The functional core for rendering evaluated projects into shareable documents.
Pure projections from evaluated plot specs to Vega-Lite JSON (`vega.rs`), plus
self-contained HTML plot pages (`plot_page.rs`) that inline vendored
vega/vega-lite/vega-embed bundles (`vega_assets.rs`, `assets/`) so output works
offline and from `file://` paths. The auto-report pipeline (#1410) lives here
too: `value_display.rs` projects runtime values into scalar/entry/grid bodies,
`report_ir.rs` derives the typed `ReportDocument` (cards, figures, checks,
provenance, `///` captions), and `report_html.rs` / `report_markdown.rs` render
it deterministically. `report_hydrate.rs` packages the interactive layer —
the no-modules wasm engine, project sources, and baseline bindings.
`report_standalone.js` adapts that embedded engine to the transport-neutral
`window.GraphcalReport.mount` API. `report_form_state.js` owns pure recursive
schema/value-to-draft projection and completeness checks. `report_outline_state.js`
provides pure path/search/tree projection. `report_results.js` owns result tabs,
output pinning and disclosure; `report_workspace.js` composes the independent
panes and adaptive input outline over explicitly registered controls (not DOM
schema discovery). `report_runtime.js` owns atomic auto-run/Apply/Discard
transitions, typed-control registration, DOM rendering, debounce, and timeout
restart lifecycle. Outline edits reuse the same native draft handlers; pins
include constructor identity, separately from diagnostic binding paths. Algebraic and indexed form submissions
remain tagged data until the evaluator constructs and validates their AST;
JavaScript never assembles container syntax. Wasm declaration outcomes serialize
the same `value_display::ValueBody` projection consumed by native HTML plus a
lossless typed editable-value projection; JavaScript creates DOM elements and
does not reinterpret tensor rank or leaf formatting.
Consumed by the CLI (`--plot`, `report build`)
and the browser WASM adapter (playground figures). No file, process, or
network I/O.

### 2.5 `graphcal-fmt`

The formatter parses `File<Raw>` and prints it with the `pretty` crate.
Formatting modules are split by syntax family under `src/format/`.
After printing, it re-parses its output and requires the two trees to be
`FormatEquivalent` (equal modulo spans). The trait lives in
`graphcal-compiler/src/syntax/format_equivalent.rs` rather than here because
AST types derive it (`#[derive(FormatEquivalent)]`, spans marked
`#[fe(skip)]`) at their definitions, and this crate depends on the compiler.

### 2.6 `graphcal-io`

`graphcal-io` isolates filesystem access behind `FileSystemReader`.
Implementations include real, in-memory, and overlay filesystems. The loader
uses this crate so tests and editor integrations can run deterministically
without direct disk coupling. Cancellable reads use the compiler's
`CancellationToken` and report cancellation as `Outcome::Cancelled`, not as a
read error.

### 2.7 `graphcal-package`

`graphcal-package` is a pure package-management domain crate. It has no Git,
filesystem, cache, or CLI I/O; callers provide manifest text, lockfile text,
source metadata, and materialized dependency manifests.

The crate owns typed package identifiers (`PackageName`, `DependencyName`,
`PackageInstanceId`, `GitCommitHash`, `GitUrl`, `GitSourceId`), manifest parsing for
`graphcal.toml`, lockfile parsing/serialization for `graphcal.lock`, and
validation of the locked package graph. Keep credentials, cache paths, and Git
commands outside this crate; it should remain the functional core for package
resolution.

### 2.8 `graphcal-cli`

The CLI is the imperative shell around the library pipeline and package
management commands. The package has both a binary target (`main.rs`) and a small
library target (`lib.rs`) so tests and the binary can share pure format-discovery
helpers.

Subcommands:

| Command  | Purpose                                      |
| -------- | -------------------------------------------- |
| `eval`   | Compile and evaluate a `.gcl` file           |
| `check`  | Parse and type-check without evaluation      |
| `format` | Format files or check formatting             |
| `dump`   | Debug-print pipeline artifacts (experimental) |
| `graph`  | Export a dependency graph                    |
| `deps`   | Manage package dependencies / lockfiles      |
| `report` | Build shareable HTML report artifacts (experimental) |
| `lsp`    | Start the language server                    |

Key files:

- `main.rs` owns command dispatch and exit-code behavior.
- `lib.rs` exposes the reusable CLI library surface.
- `format.rs` discovers `.gcl` files and classifies format status without printing.
- `deps.rs` is the imperative shell for `graphcal deps lock`: root discovery,
  validated-cache reuse, per-source writer locking, staged Git publication,
  tree hashing, and writing `graphcal.lock` around `graphcal-package`'s pure
  model.
- `graphcal-project/src/package_cache.rs` centralizes the absolute cache root and
  typed source/generation path derivation shared by the lock producer and
  read-only loader.
- `json_input.rs` converts JSON parameter documents into typed binding expressions.
- `overrides.rs` parses direct and JSON CLI parameter bindings.
- `display.rs` renders normal eval text output.
- `dump.rs` selects one pipeline boundary and pretty-prints its unstable Rust
  `Debug` representation.
- `build_support/bundle.rs` defines pure engine identity/integrity checks;
  `build_support/engine.rs` discovers source inputs and orchestrates cached Wasm
  generation for `build.rs`. `examples/export_report_engine.rs` exports that exact
  embedded bundle for Node tests and release packaging. Generated files live in
  `OUT_DIR`, not Git; this build-time dependency does not create a Rust dependency
  from the Wasm engine back to the CLI.
- `report.rs` is the imperative shell for `graphcal report build`: project
  loading, override binding, source digests, and artifact writes around the
  pure document assembly in `graphcal-report`.
- Plot/figure/layer rendering lives in `graphcal-report` (Vega-Lite
  projection plus self-contained HTML pages with vendored Vega bundles);
  the CLI consumes it for `--plot` output.

### 2.9 `graphcal-lsp`

The LSP consumes compiler/evaluator APIs and adds editor-facing analysis:

| Path                        | Feature                                           |
| --------------------------- | ------------------------------------------------- |
| `analysis_schedule_state.rs` | Pure per-document scheduling and cancellation state |
| `workspace_revision.rs`     | Typed revisions, freshness, and reverse dependency graph |
| `client_capabilities.rs`    | Typed interpretation of optional client capabilities |
| `filesystem_events.rs`      | Watched artifact classification, registration, and load-input tracking |
| `analysis.rs`               | Cached per-document `AnalysisResult` consumed by features |
| `analysis_pipeline.rs`      | Synchronous `run_analysis()`: load, check, evaluate, index |
| `imported_definitions.rs`   | Definitions and export surfaces reachable through imports |
| `fn_signatures.rs`          | Builtin and plugin signatures for Signature Help  |
| `value_format.rs`           | Compact value rendering for hover and inlay hints |
| `file_identity.rs`          | Canonical document identities of files and URIs   |
| `server.rs`                 | Server lifecycle, scheduling, and protocol handlers |
| `diagnostics.rs`            | Compiler/evaluator diagnostics to LSP diagnostics |
| `symbol_table.rs`           | Typed `SymbolKey` index from decl shells + tolerantly lowered HIR |
| `completion.rs`             | Completion                                        |
| `hover.rs`                  | Hover                                             |
| `goto_definition.rs`        | Go to definition                                  |
| `references.rs`             | Find references                                   |
| `rename.rs`                 | Rename                                            |
| `inlay_hints.rs`            | Computed value hints                              |
| `formatting.rs`             | Pure formatting-to-edit adapter                   |
| `formatting_scheduler.rs`   | Size, concurrency, cancellation, and timeout shell |
| `document_symbols.rs`       | Outline symbols                                   |
| `document_links.rs`         | Import/include links                              |
| `signature_help.rs`         | Function signatures                               |
| `code_actions.rs`           | Quick fixes                                       |
| `cursor_context.rs`         | Cursor-sensitive context                          |
| `resolve.rs` / `convert.rs` | Shared resolution and protocol conversion helpers |

`run_analysis()` treats library files specially. A file with required params or
required indexes is not evaluated standalone, so diagnostics avoid surfacing
unbound input errors for files intended to be consumed through parameterized
includes. Deliberate empty-symbol-table and empty-resolver fallbacks leave the
synchronous core as typed `AnalysisDegradation` values; the async server shell
renders them through `window/logMessage`.

### 2.10 Editors and Grammars

Syntax/editor surfaces live outside the Rust workspace:

- `grammar.ebnf` is the formal grammar source of truth.
- `graphcal-lang/tree-sitter-graphcal` contains the tree-sitter grammar and
  highlight queries.
- `graphcal-lang/vscode-graphcal` contains the VS Code extension and TextMate
  grammar.
- `graphcal-lang/zed-graphcal` contains the Zed extension and bundled grammar
  artifact.

When syntax changes, update these together with the compiler/parser and docs.

## 3. Core Data Structures

### 3.1 Typed Names

Identifier leaf segments are `NameAtom`s. A parsed AST `Ident` holds the
narrower `SourceIdentifier` (`syntax/token.rs`), proven by the lexer to be one
`IDENT` token, so compiler-generated names cannot pose as written identifiers.
Definition-site names are
`NameDef<Ns>` aliases in `syntax/names.rs`: `DeclName`, `DimName`, `UnitName`,
`StructTypeName`, `IndexName`, `FnName`, `FieldName`, `IndexVariantName`,
`ConstructorName`, `GenericParamName`, `LocalName`, `ModuleAliasName`, and
`PlotPropertyName`.

Use these newtypes for actual definition leaves only. Reference positions that
may be qualified stay as `IdentPath`/`NamePath` until module-aware resolution
produces `ResolvedName<Ns>` or `ResolvedIndexVariant`. `ResolvedName<Ns>` is the
core owner-qualified identity: a canonical `DagId` owner plus a namespace-typed
leaf atom. `DagId` keeps package identity plus one sequence of typed
`DagSegment`s (a prefix of `File` path components, then `InlineDag`
declaration names and `Instance` scopes, where the scope of a selective
include is the spelling-free `IncludeInstance`) structurally; its dotted
`Display` output is not canonical identity. A file submodule is therefore
never the child of the file its path extends, and a file submodule and an
inline DAG with one module-path spelling are rejected as ambiguous.
`ResolvedIndexVariant`
stores the resolved index identity plus the variant leaf. Diagnostics for
unresolved qualified dimensions also retain `NamePath`; dotted rendering occurs
only at the diagnostic boundary and is never smuggled through `DimName`.

The graph IR preserves these typed identities through projection. Its DOT
renderer assigns deterministic opaque local ids to node and cluster statements,
then renders semantic names only as labels; display collisions can therefore
never merge vertices or edge endpoints.

`ScopedName` carries legacy declaration lookup/display paths structurally as
qualifier segments plus a member. A qualifier `ScopeSegment` is either a
source-visible `Named` module alias or the opaque `IncludeInstance` namespace
of a selective include, which has no source path. Its dotted `Display` form is
a boundary representation for diagnostics and protocols, not a string to split
in the functional core.

### 3.2 Module Resolver and HIR

`ModuleResolver` is a pure, project-wide symbol resolver:

```text
ModuleResolver
  modules: HashMap<DagId, ModuleEntry { symbols, scope }>

ModuleSymbols
  owner: DagId
  decls, dimensions, units, struct_types, indexes, constructors

ModuleScope
  module_aliases
  selected_decls, selected_dimensions, selected_units
  selected_struct_types, selected_indexes, selected_constructors
```

It is built from loader-resolved import/include edges, not from filesystem I/O,
through a typestate (`resolve/builder.rs`): `SymbolTables` collects every
source module's declarations, `SymbolTables::scopes` connects each module's
`import` / `include` declarations to the modules the loader names
(`ModuleTargets`) and expands includes into instance modules, and
`ScopeBuilder::freeze` completes every scope (dependencies first; an instance
inherits its template's scope) into the immutable `ModuleResolver`. Callers
never order registrations.
It enforces visibility at module boundaries and returns `ResolvedName` /
`ResolvedIndexVariant` values for successful lookups.

HIR is the first layer where references are intended to be truly semantic:

- Module-owned names carry `ResolvedName<Ns>`.
- Index labels carry `ResolvedIndexVariant`.
- Generic parameters carry `GenericParamId`, not module names.
- Expression locals carry `LocalId`.
- Built-ins use closed enums such as `BuiltinType`, `BuiltinConst`, and
  `BuiltinFn`.
- Match patterns are semantic (`Constructor` or `IndexLabel`) after HIR
  lowering; syntax-only `MatchPattern::Path` does not cross the HIR boundary.

### 3.3 DAG Identity

`dag_id.rs` defines `DagId`, the canonical identity for file roots and
inline DAGs. It is an opaque package identity plus a non-empty sequence of
typed segments, not a path string: file-path components and inline DAG names
are source-module segments, and include instances are named or anonymous
instance segments. Virtual single-file projects,
manifest-backed packages, locked dependency instances, and synthetic test
contexts all receive package ids at the loader/test boundary; there is no
package-less DAG, and the compiler core does not inspect the package id's
origin.

Examples:

- `helpers/math.gcl` in package `math` becomes
  `DagId(package = "math", segments = ["helpers", "math"])`.
- `dag burn { ... }` inside that file becomes
  `DagId(package = "math", segments = ["helpers", "math", "burn"])`.

Filesystem paths are converted to `DagId` at loader boundaries. Compiler and
evaluator internals should use `DagId` rather than `PathBuf` when referring to
compiled modules or DAG bodies.

### 3.4 Loader Types

`graphcal-project/src/loader.rs` parses and resolves a project into:

```text
LoadedProject
  files: LoadedFiles
    ordered: DependencyOrdered<LoadedFile>  // { deps, root }: dependencies first; root last
    owners: HashMap<DagId, usize>  // file-root and inline DAG -> owning file position
  plugins: HashMap<PluginIdentity, PluginFileEntry>
  plugin_call_policy: PluginCallPolicy
  package_closure: Option<LoadedPackageClosure>

LoadedFile
  path: PathBuf
  dag_id: DagId
  source: Arc<String>
  ast: File<Desugared>
  named_source: NamedSource<Arc<String>>
  resolved_imports: HashMap<ModulePathKey, ResolvedModuleTarget>
  inline_dags: Vec<LoadedDag>  // source preorder

LoadedDag
  dag_id: DagId
  parent_dag_id: DagId
  body_locator: DagBodyLocator  // path into the owning file's AST
  resolved_imports: HashMap<ModulePathKey, InlineBodyImportResolution>
```

Loading is split into an IO shell and a pure builder. The shell walks the
project in load order and records a `SourceSnapshot`: each reachable file's
parsed AST plus the span-free outcome of resolving each of its import/include
paths (reads, `canonicalize` probes, and package lookups all happen here, through
the caller's filesystem capability, so LSP overlays apply). The pure
`build_loaded_files(snapshot)` then orders files, detects import cycles as a
typed `ImportCycle`, assigns `DagId`s, and connects every path to its target,
reporting the first failure in load order.

An inline DAG does not own a copy of its body. `LoadedDag::body(file)` borrows
it from the owning `LoadedFile::ast` through the validated `DagBodyLocator`,
so the file AST remains the single body owner.

`ModulePathKey` stores import/include path segments as a vector. It avoids using
joined strings as map keys inside the loader. `LoadedProject::build_module_resolver()`
then turns the loaded files, inline DAGs, and pre-resolved edges into the
compiler's `ModuleResolver`.

The loader's `ast: File<Desugared>` field is per file. It means "this source
file has been parsed, desugared, and connected to loader-side import/include
edges"; it does not mean imported declarations have been copied into the root
file AST, and reference resolution has not happened yet — that is the freeze
boundary's job.

### 3.5 IR

There is no type named `IR`. The IR stage is the pair `UnfrozenIR` (assembly)
and `HirDag` (its frozen output), both in `ir/model.rs`. `HirDag` holds the
semantic declaration lists for one DAG body, with bodies already lowered to HIR
at `UnfrozenIR::freeze`. Both stages use the same per-kind entry types from
`ir/entry.rs` (`ConstEntry<P>`, `ParamEntry<P>`, …), indexed by a
`BodyPhase`: `Syntax` bodies pair desugared AST with their resolution scope
(`InScope`), and `Lowered` bodies are strict HIR. Every entry carries its type
annotation and attribute-derived policy (plot visibility) from collection on.
`HirDag` keeps its declarations in a `DeclTable` (`ir/decl_table.rs`) keyed by
`ResolvedDeclName`, the only semantic key: source order is a list of identities,
and the source spelling is an index built once at the freeze boundary.
`#[assumes]` targets and `#[expected_fail]` keys are resolved there as well.

```text
UnfrozenIR             // assembly stage: entries in the `Syntax` phase
  decls: Vec<Decl<Syntax>>  // source order, desugared AST bodies
  + plugin imports, dynamic unit scales, unit bindings
  + typed semantic-instance edges and importer-context bindings

HirDag = UnfrozenIR::freeze(types, owner, definitions, src)
  dag_id: DagId
  definitions: ModuleDefinitions       // owner-qualified dims/units/indexes + NominalTypeRegistry
  display_dimensions                   // dimension spellings diagnostics may prefer
  decls: DeclTable<Lowered>
    order: Vec<ResolvedDeclName>        // category from Decl; contract: declaration_category.rs
    decls: HashMap<ResolvedDeclName, HirDecl>  // hir::Expr / LoweredPlotBody bodies
    spelling: HashMap<ScopedName, ResolvedDeclName>
  source_declarations, static_ports    // interface provenance authored in this DAG
  assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>
  expected_fail: HashMap<ResolvedDeclName, ResolvedExpectedFailMetadata>
    keys resolved in the authoring scope + source provenance
  dynamic_unit_scales
  imported_bindings: HashMap<ScopedName, ResolvedDeclName>
    canonical target only; checked type facts are attached at TIR construction
  extern_functions: HashMap<ExternFnKey, ExternFunctionEntry>
  external_surface: ExternalDeclSurface
    explicit_exports  // declarations carrying `pub` / `pub(bind)`
    input_ports       // bindable and projectable annotation-free params
  semantic_instances: Vec<HirInstanceRecord>
```

Imported compile-time values are never stored on a binding: they are facts of
the defining body's checked constant pool.

There are no dependency maps on `HirDag`: the owner-qualified dependency graph is
collected from the HIR bodies during TIR construction and stored on
`DagTIR::semantic.dependencies` (value sets use `BTreeSet` so DAG construction
is deterministic).

### 3.6 Semantic core and display services

`semantic/` holds the value types of the type system (base-dimension
metadata, unit scales, index definitions, checked types, applied
constructors), the scalar built-in catalog, the prelude catalog, and time
scales and zones. `display/` holds number and unit-label formatting and the
post-resolution `FormattingRegistry`. There is no source-name keyed registry.

`tir/typed/model.rs` also defines `ProjectTypeStore`, the authoritative
owner-qualified semantic store used during module-aware TIR resolution. It keys
dimensions, units, declared indexes, nominal types, and constructors by
`ResolvedName`, and keys structural finite indexes by typed `FiniteIndex`
identity. The project compiler creates it once, installs the synthetic Graphcal
prelude, and inserts each HIR module's owner-qualified `ModuleDefinitions`
directly. Imported spellings remain resolver bindings; they are not copied into
the store under importer owners.

Checked TIR retains only a `FormattingRegistry` with dimension display metadata
and reproducible timezone validation; semantic consumers must use
`ProjectTypeStore`.

### 3.7 TIR and `DagTIR`

The project TIR wraps post-resolution formatting services and all DAGs
reachable from one file. It moves through a typestate: `TirDraft` (assembly) is
consumed by `instantiate` into `InstantiatedTir`, whose only transition,
`check`, produces the immutable `CheckedTir` that evaluation and the language
server see.

```text
TirDraft / InstantiatedTir             // tir/typed/program.rs
  core: TirCore
  dags: DagRegistry
    root: DagTIR                       // mutable only during local assembly
    other_dags: BTreeMap<DagId, DagTIR>  // local inline/instance bodies
    shared_dags: BTreeMap<DagId, Arc<CheckedDag>>  // immutable imports

CheckedTir                             // tir/typed/checked.rs
  core: TirCore
    registry: FormattingRegistry       // display + timezone boundary
    project_types: Arc<ProjectTypeStore>  // one frozen project-wide store
    runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>
    extern_functions
  dags: CheckedDagRegistry
    root: CheckedDag, other_dags, shared_dags  // each at a deterministic DagPosition
    callees  // the callee position of every call slot of every body
  const_schedule: ConstSchedule

CheckedDag                             // tir/typed/checked_dag.rs
  body: DagTIR
  bodies: CheckedBodies                // checked TExpr tree per expression root
  presentation: DagPresentationFacts
  runtime_schedule: RuntimeSchedule

DagStore  // published by `tir/typed/freeze.rs` consuming local assembly, never by cloning its closure
  dags: HashMap<DagId, Arc<CheckedDag>>  // only this module's own bodies
  runtime_units: HashMap<ResolvedUnitName, Arc<UnitInfo>>  // only locally owned overlays
  external_callees  // DAGs outside the store that its bodies call

DagTIR
  dag_id: DagId
  decls: DeclTable<Typed>  // identity-keyed records in source order
    value records carry CheckedTypeAnnotation { decl_type, span, checked }
    CheckedTir::decl_type(&ResolvedDeclName) finds any value declaration's type
    instances rebase every record to the instance owner
  included_plots
  semantic: DagSemanticBody
  static_ports
  assumes_map: HashMap<ResolvedDeclName, Vec<ResolvedDeclName>>
  expected_fail: HashMap<ResolvedDeclName, ResolvedExpectedFailMetadata>
    canonical keys + authored diagnostic source; instances rekey to runtime identities
  imported_bindings: HashMap<ScopedName, ImportedBinding>  // imported constants
  semantic_instances: Vec<HirInstanceRecord>  // typed include edges
  frame: InstanceFrame  // how this DAG's bodies name its declarations
  projectable_outputs  // explicit node exports + param input ports
```

`CheckedTir::root()` borrows the file root; crate-internal `root_mut()` on the
draft is an assembly operation. Imported bodies have no mutable registry view.
Publication checks runtime-unit owners and never republishes imported unit
definitions. Inline DAG call routing is not re-derived from source paths: each
HIR `ExprKind::DagCall` carries its canonical `DagId` target, and its checked
`TExprKind::DagCall` node carries a `CallSlot` relative to the calling body.
`CheckedDagRegistry` resolves every slot of every body to the callee's
`DagPosition` once when it is built, so the registry is closed under calls by
construction.

### 3.8 ExecPlan

`ExecPlan<'p>` in `execution_plan.rs` borrows the sealed `CheckedProgram` it was
prepared from and retains one `CallablePlan` for every DAG of the program, by
registry position; `exec_plan.rs::PreparedPlan` owns the program together with
its plan (`self_cell`). Preparation and validation remain free functions in
checking `exec_plan.rs`; runtime consumers import data directly, without a
checking-layer re-export:

```text
ExecPlan<'p>
  program: &'p CheckedProgram
  declarations: HashMap<&ResolvedDeclName, PlannedDeclaration>  // every value declaration
  callables: IndexVec<DagPosition, CallablePlan>  // one per DAG, by registry position
  calls: IndexVec<DagPosition, Box<[DagPosition]>>  // callee of each call slot, per DAG

PlannedDeclaration<'p>
  scope: SealedDag  // physical body, not semantic owner
  body: Todo
      | Expression { root: Scoped<Expr>, tree: Result<ScopedTree<&TExpr>, _> }
      | Supplied
  reads: &[ResolvedDeclName]  // the checker's reads, incl. constants and imports
  domain: Option<&ResolvedDomainConstraint>

CallablePlan<'p>
  scope: SealedDag
  execution_dags: Vec<SealedDag>  // prepared semantic closure
  instances, closure_instances: PlannedInstance  // included instances and their sealed DAGs
  imports: Vec<PreparedConstantImport>  // retained imported constants
  steps: IndexVec<StepIdx, Step { declaration, deps: Vec<StepIdx> }>
```

Preparation plans every value declaration of every body once from its
authoritative declaration index, including parameters without default
expressions, and rejects duplicate locations. Each callable indexes the checker's
runtime schedule as steps whose dependencies are earlier steps of the same
callable; it rejects schedule entries absent from the declaration table or
outside the selected semantic closure. Running a frame walks the steps by index:
it looks up neither declarations nor bodies nor DAGs, and never scans bodies.
An inline call finds its callee by indexing `calls` with the caller's position
and the `CallSlot` its checked node carries; no DAG is looked up by identity.
Modules import only constants, so a call never seeds imported runtime values.
Assertion expectations and `#[assumes]` tables are read from each
body's own records.

Preparation retains canonical constant pools for both singleton and multi-body
closures and resolves imported constants (the only values a module can import)
to validated pool references once. Frame initialization still copies selected
values into a mutable invocation map; this is not a zero-allocation evaluator.

`execution_frame.rs` owns one shared binding/default/dependency/domain/insertion
machine. A frame privately retains its selected callable, project plan, and fixed
failure policy, so individual operations cannot substitute another plan or switch
between containment and propagation. Root adapters contain ordinary failures;
call adapters propagate them to the containing expression. Internal errors and
cancellation are fatal in both policies. Prepared dependency order is validated
before publication. Expressions receive their defining body's diagnostic source.

`assertion_eval.rs` consumes an expression callback rather than depending on the
expression interpreter or root adapter. Root reporting and call assertion failure
adaptation remain separate; both use the same assertion and expected-failure
semantics. The frame and assertion modules import contract definitions directly,
including the public `tir::typed::model` data module, not checking re-exports.
Nested contexts retain their enclosing declaration's work budget. Aggregate
operation/resource limits remain separately gated D08 work. Presentation replay
and whole-call retention are removed: calls resolve their selected output's
display requests before dropping their frames. Caller-owned requests pass through
child calls without retaining the caller's environment. Conversion-only scales
are not computational dependency edges; literals keep their real prerequisites. Constructors
and matches consume retained expression applications; required field constraints
cannot silently disappear, and instance comprehensions no longer reconstruct
missing materialization facts. Key forms, maps, comprehensions, and unfold
consume retained axes; normalized Nat arithmetic is never replayed from source.
Dynamic positions and actual numeric/domain/owner/generic-argument checks remain.

It contains no cloned HIR bodies and no parser or registry-building work;
evaluation reads declaration/assertion/visualization records from the checked
DAGs. Runtime planning keeps assertion metadata under canonical
`ResolvedDeclName`s and converts back to source-facing `ScopedName`s only while
assembling public output. Per-DAG execution facts retain only the stores read in
that scope; project-wide struct-field constraints have a single authoritative
map. `ResolvedDomainConstraint` lives in `graphcal-eval/src/domain_constraint.rs`
and has separate quantity (`f64`), integer (`i64`), and same-scale datetime instant
representations, so constraint families cannot mix after resolution. The
`domain_check.rs` interpreter borrows its read-only typed view; checked facts
and public result records no longer depend on runtime validation algorithms.

### 3.9 Runtime Values

There are two value layers:

- `RuntimeValue` (`graphcal-eval/src/runtime_value.rs`) is internal and
  unit-normalized. It carries no display-unit metadata. Quantities are
  `FiniteQuantity`s, keys are `KeyValue`s (a position on a concrete
  `IndexAxis`), indexed values are `IndexedValue`s over their axes, and struct
  values are `StructValue`s carrying their applied constructor, so
  owner-qualified type/index identity is preserved during evaluation.
- `Value` is user-facing and appears in `EvalResult`. Quantity values carry a
  dimension and optional display-unit information; labels, structs, and indexed
  values keep public identity carriers for diagnostics/output.

`assertion_expectation.rs` owns generic expectation records and semantic key
selection without depending on declaration collection. Source-path aliases and
blanket-attribute spans remain in the collection/lowering shells; resolved keys
use canonical `IndexTypeRef` identities. Named selection ignores display aliases
but rejects equal leaf spellings from different owners. Finite positions bind to
their tuple's assertion axis during checking, not through a fabricated name.

`TypeNameRef<Ns>` in `semantic/checked_type.rs` is the shared identity carrier
for declared type-level runtime/public values. It stores both a display leaf and
a canonical `ResolvedName<Ns>`.

Index type references are split by semantic kind. Declared indexes use
`TypeNameRef<namespace::Index>` and have a canonical `ResolvedName<Index>`.
Compiler-generated structural indexes use `FiniteIndexRef`: concrete `Fin(N)`
forms carry a validated non-zero `FiniteIndex`, while symbolic generic forms
carry a normalized `NatPolyForm` from `nat.rs`. Finite indexes intentionally do
not have fake resolved names or registry-key strings; callers that need declared
index ownership use `declared_resolved()`, and callers that need finite-index
semantics use `finite_index_ref()` / `finite_index()` /
`finite_index_form()`.

## 4. Declarations, Visibility, and Evaluation

Ordinary declarations are private unless explicitly exported. `pub` means
exported at a module/include boundary, and `pub(bind)` means exported and
bindable. Required `dim`, `type`, and `index` declarations must be `pub(bind)`.
A `param` is separate from that visibility matrix: the declaration kind itself
creates an annotation-free named input port. A missing default makes the input
port required. Its effective value is projectable like an exported value, but
its role remains distinct from declarations carrying an explicit `pub` marker.

| Category          | Main syntax                                | Evaluation phase                                | Reference rules                     |
| ----------------- | ------------------------------------------ | ----------------------------------------------- | ----------------------------------- |
| Type system       | `base dim`, `dim`, `unit`, `type`, `index` | Static definitions (compile time)               | Other type-system declarations      |
| DAG               | `dag`                                      | Compiled per body, instantiated by include/call | Own declarations, imports, includes |
| Const node        | `const node`                               | Compile time                                    | Const nodes and built-ins           |
| Param             | `param`                                    | Runtime input/default                           | Consts, params, nodes               |
| Node              | `node`                                     | Runtime computed                                | Consts, params, nodes               |
| Assert            | `assert`                                   | After runtime values                            | Consts, params, nodes               |
| Plot/Figure/Layer | `plot`, `figure`, `layer`                  | After runtime values                            | Consts, params, nodes               |

`@name` references graph values: params, nodes, and const nodes. Built-in
constants such as `PI`, `E`, `TAU`, `SQRT2`, `LN2`, and `LN10` are bare names.

## 5. Import, Include, and Project Loading

Graphcal has separate mechanisms for compile-time names and runtime instances:

- `import` is compile-time-only. It brings constants, static units, dimensions,
  types, indexes, constructors, and callable DAG blueprints into scope. It
  never creates a default runtime instance, evaluates assertions, or exports a
  runtime-dependent unit scale.
- `include` creates an explicit DAG instance, with optional value/index/type/
  dimension bindings. Assertions and plain-unit scales belong to that concrete
  instance. Repeated and nested instances preserve distinct runtime-unit
  identities; plain units remain unavailable through `import`.
- A direct DAG call validates the same categorized Static binding batch as an
  include. HIR retains the call's canonical `StaticSubstitution`, and TIR
  specializes parameter and output signatures per occurrence with the same
  `specialize_type` an include instance uses.

Import/include paths are dot-separated module paths in source. Loader internals
drop spans and store path segments in `ModulePathKey`; compiled DAG identity is
stored in `DagId`.

Project loading:

1. Determine the project root from `graphcal.toml`, an explicit root, or loose
   single-file mode.
2. Parse and desugar each file.
3. Resolve import/include paths to `DagId`s.
4. Index inline `dag` blocks as `LoadedDag`s. Each entry holds a validated
   locator into its file AST; the AST remains the sole owner of the body.
5. Record every import/include edge in a `DependencyGraph`; its depth-first
   order becomes `DependencyOrdered { deps, root }`, or its `Cycle` becomes the
   circular-import error.

After project loading, the compiler performs several assembly steps. Do not read
these as repeated merges of one giant project AST. They merge different products
at the stage where each product first has the information it needs:

| Step                          | Merged Product                                                   | Stage                             | Why Here                                                                                                                       |
| ----------------------------- | ---------------------------------------------------------------- | --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| Import scope assembly         | canonical constant targets and module/source aliases             | before current-file HIR lowering  | HIR records canonical targets only; checked types and optional constant values are attached later.                             |
| Canonical type-store assembly | native owner-qualified dimensions, units, indexes, and types      | complete HIR -> TIR checking      | Every HIR module contributes before checking, so resolver aliases point to one canonical definition without checked dependencies feeding lowering. |
| Module-template elaboration   | one shared `UnfrozenIR` template per canonical `DagId`           | project-session template store    | Repeated include/call sites reuse import processing and body lowering rather than recompiling the source template.              |
| Instantiated include assembly | a typed instance edge plus its binding environment                | unfrozen IR builder             | The importer records the typed template/instance edge; checking materializes the instance as its own DAG. |
| Dependency DAG attachment     | already-checked dependency `DagStore`s of `CheckedDag`s keyed by canonical `DagId` | TIR finalization | Cross-file DAG calls need callable checked templates, but those templates remain separate owners.                              |

The invariant is that each source `File<Desugared>` AST owns its bodies once;
`LoadedDag` indexes those bodies rather than cloning them. `import` assembles
lexical bindings only. A canonical `ModuleTemplateStore` elaborates each file or
inline-DAG template once per project session and shares it through `Arc`.
Instantiated `include` or call sites clone that immutable template only at the
specialization boundary and record an `InstanceRecord`: its `InstanceId` pairs
the canonical template with a fresh concrete owner, while its
`StaticSpecializationId` carries the index, type, and dimension substitution
and its value ports are derived from the concrete owner. Checking materializes
each instance as its own DAG under that owner: it shares its template's HIR
bodies, names their declarations through its `InstanceFrame`, and specializes
the template's checked trees with its substitution. Semantic declaration
records carry the explicit concrete owner; source-facing prefixes are lookup/presentation names, not the source of semantic
identity. Runtime-dependent units are rebased onto that concrete owner and are
installed in the project type store under their instance-qualified identities.

Generic-leakage analysis carries bare signature references as typed
index/type/dimension categories. It checks importer visibility only after an
explicit include substitution; an unsubstituted name remains dependency-local
or builtin, while a missing required-port substitution is an internal error.

`ProjectCompiler` builds the `ModuleResolver` once and lowers every loaded
module into `HirProject` using HIR-only dependency interfaces. HIR import
bindings contain canonical targets but cannot express checked types or values.
Checking then freezes one shared `ProjectTypeStore` from the complete HIR project.
Non-root module publication consumes local bodies into an immutable `DagStore`;
importers install body/unit handles rather than copying dependency closures.
Imported values are constants by construction: checking builds each checked
binding by walking the HIR imports, and the constants are read from their
defining body's checked pool, with missing facts rejected;
no mutable imported-value injection or duplicate artifact value map remains.
Constructor references resolve through `ProjectTypeStore::lookup_constructor`
to a `hir::nominal::ResolvedConstructor` (shared definition handle plus member), so
independently lowered external values need no per-DAG constructor table. For
each physical file it resolves every root/inline declaration signature first,
then attaches checked imported interfaces and consumes the same
`SignatureResolvedHirDag` values into TIR bodies. Dependency aliases never
become importer-owned semantic definitions, and the checker never reconstructs
a local interface in a separate prepass. Each `HirDag` also retains an ordered
typed record of parameter,
node, and index declarations authored directly in that DAG, excluding merged
include declarations. Checking attaches declared types, runtime keys, defaults,
visibility, and required-index facts to that record. A successful
`CheckedProject` therefore retains the final TIR, constant pool, resolved
constraints, and complete checked entry interface; runtime preparation never
consults the desugared root AST.

Package locking is adjacent to, not part of, this compile/eval pipeline. The
`graphcal deps lock` shell materializes Git dependencies and writes
`graphcal.lock`; the pure package facts and lock graph validation live in
`graphcal-package`. `GitSourceId` binds the canonical URL and immutable commit,
while the shared `PackageCacheRoot` derives content-addressed checkout paths
from that identity and the validated tree digest. The loader/project compiler
should consume already-resolved filesystem/module inputs rather than run Git or
parse lockfile conventions in the compiler core.

## 6. Errors

Diagnostics are plain core data; shells render them with `miette` (source
snippets, spans, labels, and codes).

Common layers:

```text
CompileError                       // graphcal-project/src/compile_error.rs
  Parse(RenderableDiagnostic<ParseErrorKind>)
  Load(LoadError)
  Binding(BindingError)
  Eval(RenderedSemanticError)      // a SemanticError with its SourceRegistry
  ExternalBinding { .. }

SemanticError                      // graphcal-compiler/src/semantic_error/
  Located(Diagnostic<SemanticErrorKind>)  // SourceId + span + typed family payload
  Internal(InternalError)                 // X001: a violated compiler invariant

SemanticErrorKind
  Domain, Attribute, Graph, Struct, Visibility, Name, Index,
  Plugin, Dimension, Module, Evaluation   // one typed enum per family

ModuleResolveError
  DuplicateModule
  DuplicateSymbol
  UnknownName
  PrivateName
  UnknownIndexVariant
  UnexpectedDeclKind
  ...
```

`ModuleResolveError` is produced by the pure module resolver and mapped to a
`SemanticError` at project boundaries. A semantic diagnostic names its source
only by `SourceId`; the shell resolves the id through the `SourceRegistry` that
issued it when rendering (`semantic_error/rendered.rs`), so the core never
holds `NamedSource`s. `InternalError::new` is the only constructor of an
internal error. Cooperative cancellation is never an error variant: cancellable
operations return `Result<T, Outcome<E>>`, and only `Cancelled` produces
`Outcome::Cancelled`. Parse errors are already core data: a `ParseError` is a
typed `ParseErrorKind` (with `Expected` / `Found` payloads) plus a span, and the
shell that owns the source attaches it through `RenderableDiagnostic`. Error
codes such as `D001`, `V001`, and `M020` are searchable in the source.

## 7. Tests and Fixtures

Tests use a mix of inline unit tests, snapshot tests, integration tests, and
property tests.

Fixture categories are enforced by
`crates/graphcal-cli/tests/cli.rs`:

- `tests/fixtures/valid/` checks and evaluates cleanly.
- `tests/fixtures/valid_library/` checks cleanly but is not meant to evaluate
  standalone.
- `tests/fixtures/runtime_error/` checks cleanly and fails at runtime.
- `tests/fixtures/invalid/` fails during static checking.

Important test locations:

- `crates/graphcal-package/src/lib.rs` (manifest/lockfile/package-graph unit tests)
- `crates/graphcal-project/src/graph_ir/` (graph export unit tests)
- `crates/graphcal-eval/src/interpreter_tests.rs` (single-file interpreter tests)
- `crates/graphcal-cli/tests/output_golden.rs` (whole-output golden files)
- `crates/graphcal-project/tests/error_snapshots.rs`
- `crates/graphcal-project/tests/edge_case_bugs.rs`
- `crates/graphcal-project/tests/phase0_regressions.rs`
- `crates/graphcal-project/tests/declaration_order.rs`
- `crates/graphcal-fmt/tests/format_tests.rs`
- `crates/graphcal-cli/tests/cli.rs`
- `crates/graphcal-cli/tests/dump.rs`

Useful commands:

```bash
cargo test --workspace
cargo test -p graphcal-compiler
cargo test -p graphcal-package
cargo insta review
just lint
```

## 8. Conventions Worth Keeping in Mind

| Convention                       | Where                                  | Why                                                                 |
| -------------------------------- | -------------------------------------- | ------------------------------------------------------------------- |
| AST phases                       | `syntax/phase.rs`                      | Parser-only constructs are statically excluded downstream           |
| `NameAtom` / `NameDef<Ns>`       | `syntax/names.rs`                      | Definition leaves cannot accidentally contain dotted paths          |
| `NamePath` / `IdentPath`         | syntax AST                             | Preserve source qualification until a resolver has module context   |
| `ResolvedName` / variants        | HIR/TIR/eval                           | Carry canonical owner identity instead of source alias strings      |
| `ModuleResolver`                 | `resolve/`                             | Keep module lookup pure and owner-qualified                         |
| HIR                              | `hir/`                                 | The single resolution stage: consume syntax paths once              |
| `DagId`                          | `dag_id.rs`                            | Keep filesystem paths at loader boundaries                          |
| `ModulePathKey`                  | `loader/module_path.rs`                | Keep module paths structured instead of separator-joined            |
| `TypeNameRef` identity carriers  | `semantic/checked_type.rs`             | Preserve declared index/struct owners through runtime/public values |
| Finite-index identity carriers   | `nat.rs`, `semantic/checked_type.rs`  | Keep concrete/symbolic `Fin` axes typed, not fake resolved names    |
| Trait-based I/O                  | `graphcal-io`                          | Deterministic tests and editor integration                          |
| Package identifier newtypes      | `graphcal-package`                     | Keep package/alias/instance/Git identities typed                    |
| Visitor pattern                  | `syntax/visitor.rs`                    | Centralized AST traversal                                           |
| `BTreeSet` in dep values         | IR/TIR deps                            | Deterministic graph construction                                    |
| `IndexMap` in output-facing maps | eval/display output                    | Stable user-facing order                                            |
| Separate const/runtime phases    | `execution_check.rs` (eval)            | Compile-time values and runtime values have different failure modes |
| Display units outside dimensions | `eval/public_projection.rs`            | Compute in SI, display in requested units                           |
| Deliberate absent-state fallbacks | `clippy.toml`                           | Core `unwrap_or_default` calls require a reasoned lint expectation  |

`clippy::disallowed_methods` rejects unreviewed `Option::unwrap_or_default` and
`Result::unwrap_or_default` calls. Prefer a typed boundary default, explicit
error, or explicit control-flow branch. When absence is genuinely semantic in
the functional core, use `#[expect(clippy::disallowed_methods, reason = "…")]`
to record why; imperative shell crates carry documented crate-level allowances.

When adding a feature, update the grammar, parser, compiler stages, evaluator,
LSP/editor surfaces, docs, and fixtures together. The compiler core should carry
semantic distinctions as types, not string conventions.

## 9. Suggested Reading Order

All Rust files in library-consumer order. The numbered list is generated:
regenerate it with `./internals/reading-order.py --write-guide` after dependency
changes; `just reading-order` (part of `just lint` and CI) rejects a stale list.
Strongly connected components are condensed and kept together; edges outside
those components point backward. **Zero outside-SCC forward edges is not an
acyclicity proof.** The `reading_order_sccs` refactor metric counts the
heuristic's mutually dependent groups. The [pipeline-layer guard](pipeline-layers/README.md) separately checks
explicit roles and exact production/test dependency debt, including re-export
boundaries. Its documented analysis limits still apply.

Presentation data now precedes its producers and consumers: selected evidence
is a contract, resolution is interpreter work, and public display attachment is
a pure output adapter. Immutable constant selections are not invocation state.

### Standalone browser shell

The frontend is a consumer of `graphcal-wasm`, not an upstream compiler layer.
Read `web/playground/src/` in this dependency order: `document.ts`,
`example-catalog.ts`, `location.ts`, `protocol.ts`, `output-budget.ts`, `dom.ts`, `figures.ts`,
`share-codec.ts`, `examples.ts`, `graphcal-language.ts`, `editor.ts`, `output.ts`, `layout.ts`,
`evaluation-worker.ts`, `worker-client.ts`, then `app.ts`. The worker client loads
its worker at the I/O boundary. See [playground architecture](playground.md) for
build, sharing, and safety contracts. No compiler/LSP dependency edges changed.

For the shared report browser UI, read `crates/graphcal-report/src/` in this
order: `report_form_state.js`, `report_outline_state.js`, `report_results.js`,
`report_workspace.js`, `report_runtime.js`, then `report_standalone.js`.
`report_hydrate.rs` embeds these modules for offline reports;
`web/playground/src/report.ts` consumes the same modules for sandboxed reports.

### Library-consumer sequence

<!-- BEGIN generated by `./internals/reading-order.py --write-guide`; do not edit by hand -->

1. `crates/graphcal-ratio/src/ratio.rs`
2. `crates/graphcal-ratio/src/real_power.rs`
3. `crates/graphcal-ratio/src/lib.rs`
4. `crates/graphcal-ast-derive/src/lib.rs`
5. `crates/graphcal-ast-derive/src/common.rs`
6. `crates/graphcal-ast-derive/src/phase_lift.rs`
7. `crates/graphcal-ast-derive/src/format_equivalent.rs`
8. `crates/graphcal-compiler/src/syntax/attribute.rs`
9. `crates/graphcal-compiler/src/syntax/phase.rs`
10. `crates/graphcal-compiler/src/syntax/mod.rs`
11. `crates/graphcal-compiler/src/syntax/non_empty.rs`
12. `crates/graphcal-compiler/src/desugar/mod.rs`
13. `crates/graphcal-compiler/src/tir/mod.rs`
14. `crates/graphcal-compiler/src/lib.rs`
15. `crates/graphcal-compiler/src/source_line.rs`
16. `crates/graphcal-compiler/src/text_position.rs`
17. `crates/graphcal-compiler/src/syntax/names.rs`
18. `crates/graphcal-compiler/src/syntax/import_category.rs`
19. `crates/graphcal-compiler/src/stack.rs`
20. `crates/graphcal-compiler/src/syntax/ast/plot_props.rs`
21. `crates/graphcal-compiler/src/syntax/decl_name.rs`
22. `crates/graphcal-compiler/src/syntax/span.rs`
23. `crates/graphcal-compiler/src/syntax/token.rs`
24. `crates/graphcal-compiler/src/syntax/comments.rs`
25. `crates/graphcal-compiler/src/syntax/lexer.rs`
26. `crates/graphcal-compiler/src/syntax/function_name.rs`
27. `crates/graphcal-compiler/src/syntax/plugin.rs`
28. `crates/graphcal-compiler/src/syntax/index_name.rs`
29. `crates/graphcal-compiler/src/syntax/local_name.rs`
30. `crates/graphcal-compiler/src/syntax/module_name.rs`
31. `crates/graphcal-compiler/src/dag_id.rs`
32. `crates/graphcal-compiler/src/plugin_identity.rs` (resolved package-owned artifacts versus global host identities)
33. `crates/graphcal-compiler/src/syntax/dimension.rs`
34. `crates/graphcal-compiler/src/syntax/type_name.rs`
35. `crates/graphcal-compiler/src/sparse_monomial.rs`
36. `crates/graphcal-compiler/src/finite_value.rs`
37. `crates/graphcal-compiler/src/complex_value.rs`
38. `crates/graphcal-compiler/src/declaration_category.rs`
39. `crates/graphcal-compiler/src/fresh_identity.rs`
40. `crates/graphcal-compiler/src/expression_id.rs`
41. `crates/graphcal-compiler/src/expression_source.rs`
42. `crates/graphcal-compiler/src/exact_rational.rs`
43. `crates/graphcal-compiler/src/syntax/ast/common.rs`
44. `crates/graphcal-compiler/src/cancellation.rs`
45. `crates/graphcal-compiler/src/outcome.rs`
46. `crates/graphcal-compiler/src/dependency_graph.rs`
47. `crates/graphcal-compiler/src/syntax/parser/token_stream.rs`
48. `crates/graphcal-compiler/src/node_definition.rs`
49. `crates/graphcal-compiler/src/diagnostic_anchor.rs`
50. `crates/graphcal-compiler/src/source_id.rs`
51. `crates/graphcal-compiler/src/diagnostic.rs`
52. `crates/graphcal-compiler/src/source_registry.rs`
53. `crates/graphcal-compiler/src/plot_props.rs`
54. `crates/graphcal-compiler/src/plot_visibility.rs`
55. `crates/graphcal-compiler/src/import_cycle.rs`
56. `crates/graphcal-compiler/src/syntax/builtin_type_name.rs`
57. `crates/graphcal-compiler/src/syntax/fin_position.rs`
58. `crates/graphcal-compiler/src/hir/expr_lower/mod.rs`
59. `crates/graphcal-compiler/src/display/mod.rs`
60. `crates/graphcal-compiler/src/semantic/mod.rs`
61. `crates/graphcal-compiler/src/semantic/time_scale.rs`
62. `crates/graphcal-compiler/src/semantic/time_zone.rs`
63. `crates/graphcal-compiler/src/datetime_literal.rs`
64. `crates/graphcal-compiler/src/builtin.rs`
65. `crates/graphcal-compiler/src/semantic/aliased_table.rs`
66. `crates/graphcal-compiler/src/tir/texpr/operators.rs`
67. `crates/graphcal-compiler/src/display/number.rs`
68. `crates/graphcal-compiler/src/ir/mod.rs`
69. `crates/graphcal-compiler/src/ir/instance/mint.rs`
70. `crates/graphcal-compiler/src/resolve/mint.rs`
71. `crates/graphcal-compiler/src/tir/typed/frame_mint.rs`
72. `crates/graphcal-compiler/src/resolved_name.rs`
73. `crates/graphcal-compiler/src/dimension.rs`
74. `crates/graphcal-compiler/src/node_unavailable.rs`
75. `crates/graphcal-compiler/src/generic_param.rs`
76. `crates/graphcal-compiler/src/nat.rs`
77. `crates/graphcal-compiler/src/function_signature.rs`
78. `crates/graphcal-compiler/src/syntax/ast/value.rs`
79. `crates/graphcal-compiler/src/syntax/visitor.rs`
80. `crates/graphcal-compiler/src/syntax/ast/multi_decl.rs`
81. `crates/graphcal-compiler/src/syntax/ast/decl.rs`
82. `crates/graphcal-compiler/src/resolve/category.rs`
83. `crates/graphcal-compiler/src/resolve/namespace.rs`
84. `crates/graphcal-compiler/src/syntax/doc_attach.rs`
85. `crates/graphcal-compiler/src/syntax/parser/expected.rs`
86. `crates/graphcal-compiler/src/desugar/desugared_ast.rs`
87. `crates/graphcal-compiler/src/declaration_kind.rs`
88. `crates/graphcal-compiler/src/semantic/dimension_table.rs`
89. `crates/graphcal-compiler/src/hir/expr/local_decl.rs`
90. `crates/graphcal-compiler/src/display/unit_label.rs`
91. `crates/graphcal-compiler/src/semantic/unit_scale.rs`
92. `crates/graphcal-compiler/src/semantic/index_def.rs`
93. `crates/graphcal-compiler/src/tir/materialized_shape.rs`
94. `crates/graphcal-compiler/src/ir/static_substitution.rs`
95. `crates/graphcal-compiler/src/resolve/error.rs`
96. `crates/graphcal-compiler/src/hir/expr/local_unit.rs`
97. `crates/graphcal-compiler/src/semantic/prelude.rs`
98. `crates/graphcal-compiler/src/resolve/reserved_name.rs`
99. `crates/graphcal-compiler/src/display/formatting_registry.rs`
100. `crates/graphcal-compiler/src/resolve/prelude.rs`
101. `crates/graphcal-compiler/src/ir/instance/identity.rs`
102. `crates/graphcal-compiler/src/hir/types.rs`
103. `crates/graphcal-compiler/src/hir/expr/model.rs`
104. `crates/graphcal-compiler/src/hir/expr/local_env.rs`
105. `crates/graphcal-compiler/src/hir/expr/refine.rs`
106. `crates/graphcal-compiler/src/hir/expr/visit.rs`
107. `crates/graphcal-compiler/src/hir/expr/checked.rs`
108. `crates/graphcal-compiler/src/hir/node_definition.rs`
109. `crates/graphcal-compiler/src/resolve/symbols.rs`
110. `crates/graphcal-compiler/src/resolve/scope.rs`
111. `crates/graphcal-compiler/src/resolve/tables.rs`
112. `crates/graphcal-compiler/src/hir/const_expr.rs`
113. `crates/graphcal-compiler/src/semantic/checked_type.rs`
114. `crates/graphcal-compiler/src/ir/imported_binding.rs`
115. `crates/graphcal-compiler/src/tir/static_index.rs`
116. `crates/graphcal-compiler/src/assertion_expectation.rs`
117. `crates/graphcal-compiler/src/plot_shape.rs`
118. `crates/graphcal-compiler/src/tir/presentation.rs`
119. `crates/graphcal-compiler/src/semantic/applied_constructor.rs`
120. `crates/graphcal-compiler/src/ir/override_reconciliation.rs`
121. `crates/graphcal-compiler/src/extern_struct_result.rs`
122. `crates/graphcal-compiler/src/ir/entry.rs`
123. `crates/graphcal-compiler/src/ir/decl_table.rs`
124. `crates/graphcal-compiler/src/ir/instance/frame.rs`
125. `crates/graphcal-compiler/src/ir/instance.rs`
126. `crates/graphcal-compiler/src/resolve/mod.rs`
127. `crates/graphcal-compiler/src/resolve/builder.rs`
128. `crates/graphcal-compiler/src/hir/expr/mod.rs`
129. `crates/graphcal-compiler/src/semantic/scalar_function.rs`
130. `crates/graphcal-compiler/src/syntax/format_equivalent.rs`
131. `crates/graphcal-compiler/src/resolve/exports.rs`
132. `crates/graphcal-compiler/src/resolve/imports.rs`
133. `crates/graphcal-compiler/src/resolve/lookup.rs`
134. `crates/graphcal-compiler/src/resolve/projection.rs`
135. `crates/graphcal-compiler/src/hir/closed_expr.rs`
136. `crates/graphcal-compiler/src/ir/decl_table/tests.rs`
137. `crates/graphcal-compiler/src/tir/dim_check/infer/linear_algebra.rs`
138. `crates/graphcal-compiler/src/tir/dim_check/infer/complex.rs`
139. `crates/graphcal-compiler/src/hir/type_annotation.rs`
140. `crates/graphcal-compiler/src/hir/nominal.rs`
141. `crates/graphcal-compiler/src/tir/texpr/nominal.rs`
142. `crates/graphcal-compiler/src/ir/module_definitions.rs`
143. `crates/graphcal-compiler/src/ir/prelude_definitions.rs`
144. `crates/graphcal-compiler/src/syntax/comments/tests.rs`
145. `crates/graphcal-compiler/src/syntax/parser/nesting_limit.rs`
146. `crates/graphcal-compiler/src/syntax/parser/error.rs`
147. `crates/graphcal-compiler/src/syntax/parser/mod.rs`
148. `crates/graphcal-compiler/src/static_interface.rs`
149. `crates/graphcal-compiler/src/ir/required_bindability.rs`
150. `crates/graphcal-compiler/src/hir/source_interface.rs`
151. `crates/graphcal-compiler/src/syntax/ast/introduced_name.rs`
152. `crates/graphcal-compiler/src/ir/resolve/collected.rs`
153. `crates/graphcal-compiler/src/syntax/ast.rs`
154. `crates/graphcal-compiler/src/hir/const_lower.rs`
155. `crates/graphcal-compiler/src/syntax/parser/compound.rs`
156. `crates/graphcal-compiler/src/syntax/parser/expr.rs`
157. `crates/graphcal-compiler/src/syntax/parser/table.rs`
158. `crates/graphcal-compiler/src/syntax/parser/decl/dim_unit.rs`
159. `crates/graphcal-compiler/src/syntax/parser/decl/index.rs`
160. `crates/graphcal-compiler/src/syntax/parser/decl/type_decl.rs`
161. `crates/graphcal-compiler/src/syntax/parser/decl/dag.rs`
162. `crates/graphcal-compiler/src/syntax/parser/decl/layer.rs`
163. `crates/graphcal-compiler/src/syntax/parser/decl/plot.rs`
164. `crates/graphcal-compiler/src/syntax/parser/decl/figure.rs`
165. `crates/graphcal-compiler/src/syntax/parser/decl/tests.rs`
166. `crates/graphcal-compiler/src/diagnostic_render.rs`
167. `crates/graphcal-compiler/src/syntax/parser/decl/mod.rs`
168. `crates/graphcal-compiler/src/tir/template_closure.rs`
169. `crates/graphcal-compiler/src/tir/template_closure/formal_conformance.rs`
170. `crates/graphcal-compiler/src/desugar/multi.rs`
171. `crates/graphcal-compiler/src/desugar/convert.rs`
172. `crates/graphcal-compiler/src/syntax/parser/type_expr.rs`
173. `crates/graphcal-compiler/src/syntax/parser/decl/visibility.rs`
174. `crates/graphcal-compiler/src/syntax/parser/decl/import.rs`
175. `crates/graphcal-compiler/src/syntax/parser/decl/multi.rs`
176. `crates/graphcal-compiler/src/syntax/parser/decl/value.rs`
177. `crates/graphcal-compiler/src/ir/static_dependencies.rs`
178. `crates/graphcal-compiler/src/ir/static_external_surface_formal_conformance.rs`
179. `crates/graphcal-compiler/src/resolve/tests.rs`
180. `crates/graphcal-compiler/src/syntax/ast/value/format_equivalent_tests.rs`
181. `crates/graphcal-compiler/src/syntax/doc_attach/tests.rs`
182. `crates/graphcal-compiler/src/syntax/token/tests.rs`
183. `crates/graphcal-compiler/src/tir/texpr/call_targets.rs`
184. `crates/graphcal-compiler/src/tir/typed/dag_position.rs`
185. `crates/graphcal-compiler/src/semantic_error/attribute.rs`
186. `crates/graphcal-compiler/src/semantic_error/domain.rs`
187. `crates/graphcal-compiler/src/semantic_error/graph.rs`
188. `crates/graphcal-compiler/src/semantic_error/structure.rs`
189. `crates/graphcal-compiler/src/semantic_error/tests.rs`
190. `crates/graphcal-compiler/src/semantic_error/visibility.rs`
191. `crates/graphcal-compiler/src/semantic_error/index.rs`
192. `crates/graphcal-compiler/src/hir/lower.rs`
193. `crates/graphcal-compiler/src/hir/expr_lower/context.rs`
194. `crates/graphcal-compiler/src/hir/expr_lower/error.rs`
195. `crates/graphcal-compiler/src/hir/expr_lower/lowerer.rs`
196. `crates/graphcal-compiler/src/hir/expr_lower/tolerant.rs`
197. `crates/graphcal-compiler/src/hir/expr_lower/resolve.rs`
198. `crates/graphcal-compiler/src/hir/expr_lower/lower.rs`
199. `crates/graphcal-compiler/src/ir/node_definition.rs`
200. `crates/graphcal-compiler/src/hir/expr_lower/call.rs`
201. `crates/graphcal-compiler/src/hir/expr_lower/tests.rs`
202. `crates/graphcal-compiler/src/semantic_error/name.rs`
203. `crates/graphcal-compiler/src/semantic_error/plugin.rs`
204. `crates/graphcal-compiler/src/semantic_error/dimension.rs`
205. `crates/graphcal-compiler/src/semantic_error/module.rs`
206. `crates/graphcal-compiler/src/internal_error.rs`
207. `crates/graphcal-compiler/src/semantic_error/mod.rs`
208. `crates/graphcal-compiler/src/ir/resolve/names.rs`
209. `crates/graphcal-compiler/src/ir/resolve/attribute_validation.rs`
210. `crates/graphcal-compiler/src/ir/extern_function.rs`
211. `crates/graphcal-compiler/src/ir/model.rs`
212. `crates/graphcal-compiler/src/ir/include.rs`
213. `crates/graphcal-compiler/src/tir/dim_check/builtins.rs`
214. `crates/graphcal-compiler/src/ir/module_interface.rs`
215. `crates/graphcal-compiler/src/ir/resolve/include_selection.rs`
216. `crates/graphcal-compiler/src/semantic_error/evaluation.rs`
217. `crates/graphcal-compiler/src/tir/typed/resolved_type.rs`
218. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/nat_forms.rs`
219. `crates/graphcal-compiler/src/tir/typed/resolved_type/tests.rs`
220. `crates/graphcal-compiler/src/tir/typed/model.rs`
221. `crates/graphcal-compiler/src/tir/schedule.rs`
222. `crates/graphcal-compiler/src/tir/typed/module_type_context.rs`
223. `crates/graphcal-compiler/src/tir/typed/declaration_view.rs`
224. `crates/graphcal-compiler/src/tir/typed/substitution.rs`
225. `crates/graphcal-compiler/src/ir/static_definitions.rs`
226. `crates/graphcal-compiler/src/tir/typed/substitution/tests.rs`
227. `crates/graphcal-compiler/src/tir/dim_check/helpers.rs`
228. `crates/graphcal-compiler/src/tir/dim_check/infer/rules.rs`
229. `crates/graphcal-compiler/src/tir/dim_check/domain_bound_type.rs`
230. `crates/graphcal-compiler/src/tir/dim_check/generic_substitution.rs`
231. `crates/graphcal-compiler/src/tir/typed/type_expr.rs`
232. `crates/graphcal-compiler/src/tir/typed/collect.rs`
233. `crates/graphcal-compiler/src/ir/resolve/mod.rs`
234. `crates/graphcal-compiler/src/ir/resolve/formal_conformance.rs`
235. `crates/graphcal-compiler/src/hir/diagnostics.rs`
236. `crates/graphcal-compiler/src/hir/nominal_lower.rs`
237. `crates/graphcal-compiler/src/hir/mod.rs`
238. `crates/graphcal-compiler/src/ir/extern_fns.rs`
239. `crates/graphcal-compiler/src/ir/lower.rs`
240. `crates/graphcal-compiler/src/ir/extern_fns/tests.rs`
241. `crates/graphcal-compiler/src/ir/freeze.rs`
242. `crates/graphcal-compiler/src/semantic_error/rendered.rs`
243. `crates/graphcal-compiler/src/display/include_scope_names.rs`
244. `crates/graphcal-compiler/src/semantic/index_axis.rs`
245. `crates/graphcal-compiler/src/semantic/key_value.rs`
246. `crates/graphcal-compiler/src/semantic/struct_value.rs`
247. `crates/graphcal-compiler/src/tir/texpr/map_layout.rs`
248. `crates/graphcal-compiler/src/tir/texpr/model.rs`
249. `crates/graphcal-compiler/src/tir/texpr/map.rs`
250. `crates/graphcal-compiler/src/tir/texpr/assembly.rs`
251. `crates/graphcal-compiler/src/tir/texpr/checked_bodies.rs`
252. `crates/graphcal-compiler/src/tir/texpr.rs`
253. `crates/graphcal-compiler/src/tir/texpr/tests.rs`
254. `crates/graphcal-compiler/src/tir/typed/checked_dag.rs`
255. `crates/graphcal-compiler/src/tir/typed/dag_store.rs`
256. `crates/graphcal-compiler/src/tir/typed/body_scope.rs`
257. `crates/graphcal-compiler/src/tir/typed/scoped_node.rs`
258. `crates/graphcal-compiler/src/tir/typed/dag_slots.rs`
259. `crates/graphcal-compiler/src/tir/typed/program.rs`
260. `crates/graphcal-compiler/src/tir/typed/override_dependencies.rs`
261. `crates/graphcal-compiler/src/tir/dim_check/infer/mod.rs`
262. `crates/graphcal-compiler/src/tir/dim_check/expression_axes.rs`
263. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/observations.rs`
264. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/context.rs`
265. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/generics.rs`
266. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/calls.rs`
267. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/conversion_calls.rs`
268. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/extern_call.rs`
269. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/mod.rs`
270. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/operators.rs`
271. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/recurrence.rs`
272. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/override_deps.rs`
273. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/indexing.rs`
274. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/map_literal.rs`
275. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/match_expr.rs`
276. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/nominal.rs`
277. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/refs.rs`
278. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/dispatch.rs`
279. `crates/graphcal-compiler/src/tir/typed/checked.rs`
280. `crates/graphcal-compiler/src/tir/typed/checked_instance.rs`
281. `crates/graphcal-compiler/src/tir/dim_check/schedules.rs`
282. `crates/graphcal-compiler/src/tir/typed/evaluation_unit.rs`
283. `crates/graphcal-compiler/src/tir/typed/freeze.rs`
284. `crates/graphcal-compiler/src/tir/typed.rs`
285. `crates/graphcal-compiler/src/tir/typed/specialization.rs`
286. `crates/graphcal-compiler/src/tir/typed/tests.rs`
287. `crates/graphcal-compiler/src/tir/dim_check/body_specialization.rs`
288. `crates/graphcal-compiler/src/tir/dim_check/infer/hir/dag_call.rs`
289. `crates/graphcal-compiler/src/ir/resolve/tests.rs`
290. `crates/graphcal-compiler/src/tir/typed/checking_tir.rs`
291. `crates/graphcal-compiler/src/tir/dim_check/concrete_obligations.rs`
292. `crates/graphcal-compiler/src/tir/dim_check/model_schema.rs`
293. `crates/graphcal-compiler/src/tir/dim_check/mod.rs`
294. `crates/graphcal-compiler/src/tir/dim_check/plot.rs`
295. `crates/graphcal-compiler/src/tir/dim_check/template_closure.rs`
296. `crates/graphcal-compiler/src/tir/dim_check/instance_bodies.rs`
297. `crates/graphcal-compiler/src/tir/dim_check/presentation.rs`
298. `crates/graphcal-compiler/src/tir/dim_check/tests.rs`
299. `crates/graphcal-io/src/atomic_write.rs`
300. `crates/graphcal-io/src/limits.rs`
301. `crates/graphcal-io/src/reader.rs`
302. `crates/graphcal-io/src/virtual_path.rs`
303. `crates/graphcal-io/src/real_fs.rs`
304. `crates/graphcal-io/src/in_memory_fs.rs`
305. `crates/graphcal-io/src/source_tree.rs`
306. `crates/graphcal-io/src/ingestion.rs`
307. `crates/graphcal-io/src/overlay_fs.rs`
308. `crates/graphcal-io/src/lib.rs`
309. `crates/graphcal-package/src/lib.rs`
310. `crates/graphcal-plugin-abi/src/constants.rs`
311. `crates/graphcal-plugin-abi/src/section.rs`
312. `crates/graphcal-plugin-abi/src/manifest.rs`
313. `crates/graphcal-plugin-abi/src/lib.rs`
314. `crates/graphcal-plugin-macros/src/lib.rs`
315. `crates/graphcal-plugin-macros/src/dims.rs`
316. `crates/graphcal-plugin-macros/src/parse.rs`
317. `crates/graphcal-plugin-macros/src/lower.rs`
318. `crates/graphcal-plugin-macros/src/manifest.rs`
319. `crates/graphcal-plugin-macros/src/codegen.rs`
320. `crates/graphcal-plugin/src/lib.rs`
321. `crates/graphcal-eval/src/eval_expr/numeric.rs`
322. `crates/graphcal-eval/src/eval_expr/datetime.rs`
323. `crates/graphcal-eval/src/lib.rs`
324. `crates/graphcal-eval/src/domain_constraint.rs`
325. `crates/graphcal-eval/src/pipeline_metrics.rs`
326. `crates/graphcal-eval/src/invariant.rs`
327. `crates/graphcal-eval/src/runtime_value/indexed.rs`
328. `crates/graphcal-eval/src/presentation_evidence.rs`
329. `crates/graphcal-eval/src/runtime_value.rs`
330. `crates/graphcal-eval/src/runtime_value/dense_array.rs`
331. `crates/graphcal-eval/src/domain_check.rs`
332. `crates/graphcal-eval/src/runtime_presentation.rs`
333. `crates/graphcal-eval/src/eval/bindings.rs`
334. `crates/graphcal-eval/src/constant_pools.rs`
335. `crates/graphcal-eval/src/checked_program.rs`
336. `crates/graphcal-eval/src/execution_plan.rs`
337. `crates/graphcal-eval/src/static_incompleteness.rs`
338. `crates/graphcal-eval/src/execution_frame.rs`
339. `crates/graphcal-eval/src/eval_expr/work_budget.rs`
340. `crates/graphcal-eval/src/eval_expr/conversions.rs`
341. `crates/graphcal-eval/src/eval_expr/complex.rs`
342. `crates/graphcal-eval/src/eval_expr/aggregations.rs`
343. `crates/graphcal-eval/src/eval_expr/linear_algebra_lu.rs`
344. `crates/graphcal-eval/src/eval/runtime/declaration_body.rs`
345. `crates/graphcal-eval/src/test_tir.rs`
346. `crates/graphcal-eval/src/host_abi/scalar.rs`
347. `crates/graphcal-eval/src/host_abi/argument.rs`
348. `crates/graphcal-eval/src/host_fns.rs`
349. `crates/graphcal-eval/src/host_abi.rs`
350. `crates/graphcal-eval/src/eval_expr/context.rs`
351. `crates/graphcal-eval/src/eval_expr/arithmetic.rs`
352. `crates/graphcal-eval/src/eval_expr/linear_algebra.rs`
353. `crates/graphcal-eval/src/host_abi/marshal.rs`
354. `crates/graphcal-eval/src/eval_expr/unit_scale.rs`
355. `crates/graphcal-eval/src/eval_expr/presentation.rs`
356. `crates/graphcal-eval/src/eval_expr/operations.rs`
357. `crates/graphcal-eval/src/eval/output_decl_name.rs`
358. `crates/graphcal-eval/src/eval/plot_unavailable.rs`
359. `crates/graphcal-eval/src/eval/types.rs`
360. `crates/graphcal-eval/src/assertion_eval.rs`
361. `crates/graphcal-eval/src/eval/runtime/root_names.rs`
362. `crates/graphcal-eval/src/eval/public_projection.rs`
363. `crates/graphcal-eval/src/eval/plot_data.rs`
364. `crates/graphcal-eval/src/eval/runtime/dependency_failures.rs`
365. `crates/graphcal-eval/src/eval/runtime/evaluated_root.rs`
366. `crates/graphcal-eval/src/eval_expr/hir_eval.rs`
367. `crates/graphcal-eval/src/eval_expr/mod.rs`
368. `crates/graphcal-eval/src/eval/runtime/assertions.rs`
369. `crates/graphcal-eval/src/eval/runtime/plots.rs`
370. `crates/graphcal-eval/src/eval/runtime/value_entries.rs`
371. `crates/graphcal-eval/src/eval/runtime/root_loop.rs`
372. `crates/graphcal-eval/src/eval/runtime/root_outcome.rs`
373. `crates/graphcal-eval/src/eval/runtime.rs`
374. `crates/graphcal-eval/src/eval/mod.rs`
375. `crates/graphcal-eval/src/execution_check/const_eval.rs`
376. `crates/graphcal-eval/src/execution_check/domain_resolve.rs`
377. `crates/graphcal-eval/src/execution_check.rs`
378. `crates/graphcal-eval/src/exec_plan.rs`
379. `crates/graphcal-eval/src/eval/runtime/plots/tests.rs`
380. `crates/graphcal-eval/src/interpreter_tests.rs`
381. `crates/graphcal-project/src/dependency_ordered.rs`
382. `crates/graphcal-project/src/import_surface.rs`
383. `crates/graphcal-project/src/lib.rs`
384. `crates/graphcal-project/src/loader/module_path.rs`
385. `crates/graphcal-project/src/inline_dag.rs`
386. `crates/graphcal-project/src/loader/loaded_file.rs`
387. `crates/graphcal-project/src/loader/inline_dags.rs`
388. `crates/graphcal-project/src/package_cache.rs`
389. `crates/graphcal-project/src/package_snapshot.rs` (capture and authenticate sources plus declared Wasm artifacts)
390. `crates/graphcal-project/src/package_sources.rs` (explicit native-cache versus isolated embedded dependency authority)
391. `crates/graphcal-project/src/prepare/model_schema.rs`
392. `crates/graphcal-project/src/project_bundle.rs` (bounded portable artifacts and virtual mounting; consumed by report assembly and browser preparation)
393. `crates/graphcal-project/src/project_compiler/model.rs`
394. `crates/graphcal-project/src/prepare/output.rs`
395. `crates/graphcal-project/src/project_compiler/template.rs`
396. `crates/graphcal-project/src/tests/exec_plan.rs`
397. `crates/graphcal-project/src/loader/budget_violation.rs`
398. `crates/graphcal-project/src/loader/loaded_project.rs`
399. `crates/graphcal-project/src/project_compiler/hir_project.rs`
400. `crates/graphcal-project/src/project_compiler/including_module.rs`
401. `crates/graphcal-project/src/binding_error.rs`
402. `crates/graphcal-project/src/load_error.rs`
403. `crates/graphcal-project/src/compile_error.rs`
404. `crates/graphcal-project/src/loader/source_snapshot.rs`
405. `crates/graphcal-project/src/project_compiler/binding_values.rs`
406. `crates/graphcal-project/src/project_compiler/entry_interface.rs`
407. `crates/graphcal-project/src/project_compiler/checked_project.rs`
408. `crates/graphcal-project/src/project_compiler/module_resolve_errors.rs`
409. `crates/graphcal-project/src/loader/budget.rs`
410. `crates/graphcal-project/src/loader/build.rs`
411. `crates/graphcal-project/src/loader/build/tests.rs`
412. `crates/graphcal-project/src/loader/source_authority.rs`
413. `crates/graphcal-project/src/loader.rs`
414. `crates/graphcal-project/src/project_compiler/generic_leakage.rs`
415. `crates/graphcal-project/src/project_compiler/generic_leakage/tests.rs`
416. `crates/graphcal-project/src/project_compiler/imports.rs`
417. `crates/graphcal-project/src/project_compiler/imports/tests.rs`
418. `crates/graphcal-project/src/project_compiler/lowering.rs`
419. `crates/graphcal-project/src/project_compiler/checking.rs`
420. `crates/graphcal-project/src/project_compiler/pipeline.rs`
421. `crates/graphcal-project/src/project_compiler/session.rs`
422. `crates/graphcal-project/src/prepare/prepared.rs`
423. `crates/graphcal-project/src/prepare/mod.rs`
424. `crates/graphcal-project/src/loader/tests.rs`
425. `crates/graphcal-project/src/prepare/binding_compile.rs`
426. `crates/graphcal-project/src/prepare/tenax_model.rs`
427. `crates/graphcal-project/src/project_compiler/mod.rs`
428. `crates/graphcal-project/src/tests.rs`
429. `crates/graphcal-project/src/graph_ir/mod.rs`
430. `crates/graphcal-project/src/graph_ir/dot.rs`
431. `crates/graphcal-project/src/tests/checked_expressions.rs`
432. `crates/graphcal-project/src/tests/presentation_evidence.rs`
433. `crates/graphcal-project/src/tests/sealed_program.rs`
434. `crates/graphcal-report/src/lib.rs`
435. `crates/graphcal-report/src/escape.rs`
436. `crates/graphcal-report/src/vega_assets.rs`
437. `crates/graphcal-report/src/report_hydrate.rs`
438. `crates/graphcal-report/src/vega.rs`
439. `crates/graphcal-report/src/plot_page.rs`
440. `crates/graphcal-report/src/value_display.rs`
441. `crates/graphcal-report/src/report_ir.rs`
442. `crates/graphcal-report/src/report_html.rs`
443. `crates/graphcal-report/src/report_markdown.rs`
444. `crates/graphcal-test-support/src/lib.rs`
445. `crates/graphcal-test-support/src/project.rs`
446. `crates/graphcal-test-support/src/bytes.rs`
447. `crates/graphcal-tenax/src/lib.rs`
448. `crates/graphcal-wasm/src/project.rs`
449. `crates/graphcal-wasm/src/output.rs`
450. `crates/graphcal-wasm/src/js_request.rs`
451. `crates/graphcal-wasm/src/bindings.rs`
452. `crates/graphcal-wasm/src/diagnostics.rs`
453. `crates/graphcal-wasm/src/browser_report.rs`
454. `crates/graphcal-wasm/src/one_shot.rs`
455. `crates/graphcal-plugin-host/src/cache.rs`
456. `crates/graphcal-plugin-host/src/convert.rs`
457. `crates/graphcal-plugin-host/src/limits.rs`
458. `crates/graphcal-plugin-host/src/module.rs`
459. `crates/graphcal-plugin-host/src/host.rs`
460. `crates/graphcal-plugin-host/src/registry.rs`
461. `crates/graphcal-wasm/src/prepared.rs`
462. `crates/graphcal-wasm/src/lib.rs`
463. `crates/graphcal-plugin-host/src/lib.rs`
464. `crates/graphcal-fmt/src/lib.rs`
465. `crates/graphcal-fmt/src/format/doc.rs`
466. `crates/graphcal-fmt/src/format/formatter.rs`
467. `crates/graphcal-fmt/src/format/unit_dim.rs`
468. `crates/graphcal-fmt/src/format/expr.rs`
469. `crates/graphcal-fmt/src/format/decl.rs`
470. `crates/graphcal-fmt/src/format/mod.rs`
471. `crates/graphcal-lsp/src/lib.rs`
472. `crates/graphcal-lsp/src/convert.rs`
473. `crates/graphcal-lsp/src/cursor_context.rs`
474. `crates/graphcal-lsp/src/symbol_identity.rs`
475. `crates/graphcal-lsp/src/nominal_type_index.rs`
476. `crates/graphcal-lsp/src/symbol_table.rs`
477. `crates/graphcal-lsp/src/project_symbols.rs`
478. `crates/graphcal-lsp/src/formatting.rs`
479. `crates/graphcal-lsp/src/workspace_revision.rs`
480. `crates/graphcal-lsp/src/analysis_schedule_state.rs`
481. `crates/graphcal-lsp/src/formatting_scheduler.rs`
482. `crates/graphcal-lsp/src/client_capabilities.rs`
483. `crates/graphcal-lsp/src/filesystem_events.rs`
484. `crates/graphcal-lsp/src/file_identity.rs`
485. `crates/graphcal-lsp/src/fn_signatures.rs`
486. `crates/graphcal-lsp/src/imported_definitions.rs`
487. `crates/graphcal-lsp/src/analysis.rs`
488. `crates/graphcal-lsp/src/resolve.rs`
489. `crates/graphcal-lsp/src/signature_help.rs`
490. `crates/graphcal-lsp/src/inlay_hints.rs`
491. `crates/graphcal-lsp/src/document_symbols.rs`
492. `crates/graphcal-lsp/src/document_links.rs`
493. `crates/graphcal-lsp/src/goto_definition.rs`
494. `crates/graphcal-lsp/src/references.rs`
495. `crates/graphcal-lsp/src/hover.rs`
496. `crates/graphcal-lsp/src/value_format.rs`
497. `crates/graphcal-cli/src/lib.rs`
498. `crates/graphcal-lsp/src/diagnostics.rs`
499. `crates/graphcal-lsp/src/code_actions.rs`
500. `crates/graphcal-lsp/src/analysis_pipeline.rs`
501. `crates/graphcal-lsp/src/completion.rs`
502. `crates/graphcal-lsp/src/rename.rs`
503. `crates/graphcal-lsp/src/server.rs`
504. `crates/graphcal-lsp/src/protocol_tests.rs`
505. `crates/graphcal-cli/src/display.rs`
506. `crates/graphcal-cli/src/format.rs`
507. `crates/graphcal-cli/src/json_input.rs`
508. `crates/graphcal-cli/src/overrides.rs`
509. `crates/graphcal-cli/src/main.rs`
510. `crates/graphcal-cli/src/report.rs`
511. `crates/graphcal-cli/src/model.rs`
512. `crates/graphcal-cli/src/dump.rs`
513. `crates/graphcal-cli/src/deps.rs`
514. Build-time shell (explicit path modules, curated beyond the heuristic):
     `crates/graphcal-cli/build_support/bundle.rs` →
     `crates/graphcal-cli/build_support/engine.rs` →
     `crates/graphcal-cli/build.rs` →
     `crates/graphcal-cli/examples/export_report_engine.rs`
515. `crates/graphcal-cli/src/plugin.rs`
516. `crates/graphcal-ast-derive/tests/format_equivalent.rs`
517. `crates/graphcal-ast-derive/tests/phase_lift.rs`
518. `crates/graphcal-plugin/tests/expansion.rs`
519. `crates/graphcal-plugin/tests/prelude_drift.rs`
520. `crates/graphcal-plugin/tests/abi_memory.rs`
521. `crates/graphcal-project/tests/chunk5_regressions.rs`
522. `crates/graphcal-project/tests/chunk6_regressions.rs`
523. `crates/graphcal-project/tests/declaration_order.rs`
524. `crates/graphcal-project/tests/edge_case_bugs.rs`
525. `crates/graphcal-project/tests/error_snapshots.rs`
526. `crates/graphcal-project/tests/generated_projects.rs`
527. `crates/graphcal-project/tests/host_fn_results.rs`
528. `crates/graphcal-project/tests/namespace_formal_conformance.rs`
529. `crates/graphcal-project/tests/phase0_regressions.rs`
530. `crates/graphcal-project/tests/phase1_regressions.rs`
531. `crates/graphcal-project/tests/phase4_regressions.rs`
532. `crates/graphcal-project/tests/static_definition_regressions.rs`
533. `crates/graphcal-project/tests/todo.rs`
534. `crates/graphcal-report/tests/report.rs`
535. `crates/graphcal-wasm/tests/tutorial_examples.rs`
536. `crates/graphcal-wasm/tests/wasm_runtime.rs`
537. `crates/graphcal-wasm/tests/wasm_presentation.rs`
538. `crates/graphcal-wasm/tests/playground_examples.rs`
539. `crates/graphcal-wasm/tests/playground_experiments.rs`
540. `crates/graphcal-plugin-host/tests/runtime.rs`
541. `crates/graphcal-plugin-host/tests/project_eval.rs`
542. `crates/graphcal-fmt/tests/todo.rs`
543. `crates/graphcal-fmt/tests/format_tests.rs`
544. `crates/graphcal-cli/tests/todo.rs`
545. `crates/graphcal-cli/tests/cli.rs`
546. `crates/graphcal-cli/tests/plugin_cmd.rs`
547. `crates/graphcal-cli/tests/plugin_e2e.rs`
548. `crates/graphcal-cli/tests/dump.rs`
549. `crates/graphcal-cli/tests/presentation.rs`
550. `crates/graphcal-cli/tests/report_engine.rs`
551. `crates/graphcal-cli/tests/output_golden.rs`

<!-- END generated by `./internals/reading-order.py --write-guide` -->
