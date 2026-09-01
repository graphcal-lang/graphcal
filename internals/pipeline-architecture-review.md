# Compiler/Evaluator Pipeline — Architecture Review

> [!WARNING]
> This review was written by an AI agent and must be verified by a human developer. After human verification, this alert may be removed.

- **Reviewed at:** commit `a9d02f3` (2026-09-01)
- **Scope:** the compile/evaluate pipeline — `graphcal-compiler` (syntax → HIR → TIR), `graphcal-eval` (loader, project compiler, exec plan, runtime), and the seams to the LSP/CLI/WASM consumers. Formatter, package manager, plugins, and report rendering are covered only where they constrain the pipeline.
- **Method:** the whole orchestration layer (`project_compiler/session.rs`, `pipeline.rs`, `checking.rs`, `hir_project.rs`, `eval/mod.rs`) was read in full; the heavyweight stages (`ir/lower.rs`, `loader.rs`, `hir/`, `tir/`, `exec_plan.rs`, `eval/runtime.rs`, `project_compiler/imports.rs`/`lowering.rs`) were surveyed exhaustively with line-level verification of every claim cited below. `internals/codebase-reading-guide.md` was used as the map and checked against the code.

---

## 1. Verdict in brief

**The stage architecture is fundamentally sound and unusually well-disciplined.** The pipeline is forward-only, each stage boundary is a real phase value with private construction, illegal states are made unrepresentable at exactly the places that matter for a safety-first calculation language, and the documented rationale (the reading guide) matches the code to a degree that is rare. No stage should be removed; none is missing.

The problems are not in the stage *design* but in three places:

1. **Physical placement.** The pipeline's entire back half of *compilation* — project lowering, HIR→TIR checking orchestration, the module-resolver construction pass, and the check-time half of `exec_plan.rs` — lives in `graphcal-eval`, producing dependency-direction inversions and misleading names (`exec_plan.rs` is ~90 % check-time code; the loader contains a ~490-line elaboration pass).
2. **The include/instantiation strategy.** Monomorphization by syntactic rewriting (clone template → rewrite paths with string-erased maps → re-lower → re-check per instance) is the single largest complexity tax in the codebase (~2,150 lines in `ir/lower.rs` alone, plus resolver-level instance synthesis in the loader). The typed `InstanceRecord` machinery is already halfway to the semantic alternative.
3. **Redundant recomputation inside the check loop.** Declared types are resolved twice per DAG and rebuilt at least four more times per file; every declaration body is traversed end-to-end by ten separate exhaustive passes; several per-const/per-file clone patterns are quadratic.

**Can it be simplified without losing safety? Yes — mostly by finishing migrations the codebase already started.** Each of the five simplifications proposed in §6 deletes machinery while preserving (in two cases strengthening) the checks. The stage count itself is not the problem and should not be reduced.

The review also surfaced **two concrete defect candidates** in the instantiation machinery — an inline-DAG template recording the *importer's* export surface instead of its own, and an asymmetric resolution-owner rewrite — detailed at the end of F2.

---

## 2. Stage inventory

The developers' own canonical boundary list is the `graphcal dump` command (`crates/graphcal-cli/src/dump.rs:129-138`): **source → tokens → ast → desugared → modules → hir → tir → plan → runtime → result**. Expanded to the actual phase values:

```text
Source text (+ LoaderBudget)
   │  syntax/lexer.rs (logos)
   ▼
Tokens with Span
   │  syntax/parser/  (recursive descent, Pratt expressions)
   ▼
File<Raw> ──────────────────────────► graphcal-fmt (formatter reads this phase)
   │  desugar/ + syntax/desugar.rs
   ▼
File<Desugared>                       (final syntax-AST phase)
   │  graphcal-eval/loader.rs         (I/O, DagId minting, load order, plugins)
   ▼
LoadedProject ── build_module_resolver() ──► ModuleResolver   ("modules")
   │  project_compiler/{imports,lowering}.rs + ir/resolve + ir/lower.rs
   ▼
UnfrozenIR  (per DAG: shells checked, registry built, includes merged)
   │  UnfrozenIR::freeze — the single resolution stage
   ▼
HirDag  (per DAG)  ──collected──►  HirProject   ("hir")
   │  project_compiler/checking.rs + tir/ + dim_check/ + exec_plan.rs (check half)
   ▼
CheckedProject = TIR (DagTIR + DagSemanticBody) + CheckedExecutionFacts   ("tir")
   │  eval/project/prepare.rs + exec_plan.rs (thin half)
   ▼
ExecPlan + PreparedProject            ("plan")
   │  eval/runtime.rs                 (topological interpretation, containment)
   ▼
RuntimeEvaluation                     ("runtime": SI values + contained errors)
   │  eval/project/prepared.rs + output.rs + display.rs + public_projection.rs
   ▼
EvalResult                            ("result": public, display-aware)
```

Weight of each region (non-test lines where measured):

| Region | Location | ~LOC | Notes |
|---|---|---:|---|
| Lexer/parser/AST | `compiler/src/syntax/` | 28.3k total | `module_resolve.rs` ≈ 4.0k non-test; parser ≈ 9k |
| Desugar + phase machinery | `compiler/src/desugar/`, `syntax/phase.rs` | 0.9k | deliberately small; `phase.rs` is 78 lines |
| Loader | `eval/src/loader.rs` | 3.7k non-test | 22 distinct responsibilities (F8) |
| IR assembly + freeze + includes | `compiler/src/ir/` | 10.8k | `lower.rs` ≈ 5.8k non-test, ~37 % include machinery |
| HIR | `compiler/src/hir/` | 6.1k | vocabulary + lowering; the `HirDag` aggregate lives in `ir/lower.rs` |
| Registry | `compiler/src/registry/` | 8.4k | frontend `Registry` + `SemanticRegistry` + built-ins |
| TIR + type/dimension checking | `compiler/src/tir/` | 23.7k | `dim_check/infer/hir.rs` ≈ 5.5k, no in-file tests |
| Project compiler | `eval/src/project_compiler/` | 6.3k | orchestration + imports + instantiation |
| Exec plan + facts | `eval/src/exec_plan.rs` + `execution_facts.rs` | 2.0k | ~90 % is check-time work (F4) |
| Runtime + output | `eval/src/eval/`, `eval_expr/` | 14.5k | interpreter, projection, display |

---

## 3. What each stage means

The reading guide frames every stage as a question; the code confirms each answer. This section states the question, what the stage actually does, and the invariant its output type carries.

### 3.1 Lexing and parsing → `File<Raw>`

*"What did the user write?"* `logos`-based lexer with byte-accurate `Span`s; recursive-descent parser with Pratt expression precedence. `File<Raw>` preserves everything surface: multi-declarations, table literals, punctuation-sensitive shapes. It is the formatter's input and the only phase that may care about concrete syntax. Reference positions are parsed as structured `IdentPath`/`NamePath` — never pre-split strings — so qualification survives losslessly to the resolver.

### 3.2 Desugaring → `File<Desugared>`

*"What is the simpler form of that syntax?"* Multi-declarations expand to ordinary declarations; table literals become `MapLiteral`. The phase system (`syntax/phase.rs`) is a sealed trait with two associated sugar slots; in `Desugared` those slots are `core::convert::Infallible`, so post-desugar code handles the impossible case with `phase::never(x)` instead of `unreachable!()`. This is the cheapest possible implementation of "parser conveniences are statically gone downstream" — 78 lines of machinery, high leverage. `File<Desugared>` is deliberately the *final* AST phase: references stay syntactic so that resolution happens exactly once, later.

### 3.3 Loading → `LoadedProject` (and the `ModuleResolver`)

*"Which file or DAG does a module path mean, and in what order do we compile?"* The loader is the imperative shell: root/manifest discovery, budgeted reads through `graphcal-io`, parse+desugar per file, import-path→file resolution, inline-`dag` lifting (`LoadedDag` holds a validated `DagBodyLocator` into the owning AST — it never clones the body), plugin byte loading with hash pinning, DFS load order with cycle detection, and package-cache reads with tree-hash re-verification. Its types enforce real invariants: `LoadedProject::from_parts` is the only constructor and derives the owner map; `ResolvedModuleTarget` can only be minted for a loaded module; `ModulePathKey` keeps module paths structural.

The *"modules"* dump boundary is `LoadedProject::build_module_resolver()`, which turns loader-resolved edges into the pure, project-wide `ModuleResolver` (`syntax/module_resolve.rs`): per-`DagId` symbol tables plus per-module alias/selective-import scopes. Findings F1 and F10 concern where this construction lives, not what it does.

### 3.4 IR assembly → `UnfrozenIR`

*"What declarations does this DAG body contain?"* Assembly (`ir/resolve/` + `ir/lower.rs`) groups a desugared body into consts/params/nodes/asserts/plots/figures/layers, runs every check that needs the whole DAG body but no cross-module resolution — duplicate names, namespace collision rules, visibility, bindability (V002, backed by a Lean-verified pure core in `ir/required_bindability.rs`), attribute placement — and builds the leaf-keyed frontend `Registry` in a five-phase registration order that permits forward references. Include instantiation (`merge_dependency`) and override application also happen here because they are syntactic operations that must precede resolution (see F2 for the cost of that choice).

### 3.5 Freeze → `HirDag`

*"What does each reference point to?"* `UnfrozenIR::freeze` is the pipeline's single resolution stage (`ir/lower.rs:1203`, 411 lines): it lowers every body and type annotation to HIR — classifying declarations, dimensions, indexes, constructors, locals, generic params, and built-ins in one pass — then narrows the registry via `Registry::into_semantic()`, which *drops* syntax-backed nominal definitions and DAG ASTs so that `NominalTypeRegistry` is the only nominal authority afterward. That narrowing is a model example of capability reduction by type.

HIR itself (`hir/`) carries canonical identities everywhere: `ResolvedName<Ns>`, `ResolvedIndexVariant`, `LocalId`, `GenericParamId`, closed built-in enums, semantic match patterns, spans on every node and reference. Lowering is diagnostic-accumulating (`lower_expr_tolerant`) for the LSP; the strict entry rejects any tree containing `ExprKind::Error`, so the batch pipeline never observes one (an invariant that is currently dynamic — see F7).

### 3.6 Project lowering → `HirProject`

*"The whole project, resolved, before any checking."* `ProjectCompiler::lower()` walks `load_order`, processes each file's imports/includes, and freezes every file root and inline DAG exactly once, retaining only HIR interfaces between files (frontend registries are deliberately discarded after lowering; the one semantic fact kept is the exported runtime-unit set). `HirProject` is a genuine phase value: private construction guarantees every module is frozen, every reference canonical, and no checked facts exist yet. It exists as a separate stage for a hard reason: the project-wide `ProjectTypeStore` must be built from the **complete** HIR before any module can be checked — which is why lowering and checking are two loops, not one.

### 3.7 Checking → `CheckedProject` (TIR + `CheckedExecutionFacts`)

*"Is the program type- and dimension-correct — and what is every compile-time value?"* Per file in load order, `check_hir_file` resolves declared types, attaches checked imported bindings, builds `DagTIR` (declaration records owning their HIR bodies exactly once, plus the derived-facts-only `DagSemanticBody`), merges dependency `DagTIR`s under canonical `DagId`s, and runs `check_dimensions_tir` — which, despite its name, is the entire semantic type checker (dimensions, indexes, structs, datetimes, domains, plots, presentation facts).

Two design decisions deserve emphasis because they are easy to miss and load-bearing:

- **Checking is where compile-time execution happens.** `check_execution_facts_with_inherited` (`exec_plan.rs:240`, called from `checking.rs:203`) evaluates every `const node`, topologically sorts both the const pool and the runtime schedule, resolves and validates every domain bound (including checking const values against their domains at compile time), and freezes the results into `Arc`-shared `CheckedExecutionFacts`. This gives `check` full-totality semantics: a project that checks has *all* static work done.
- **`CheckedProject` cannot be fabricated.** Private fields (`session.rs:123`) mean the only way to obtain one is to run every mandatory check.

### 3.8 Preparation → `ExecPlan` / `PreparedProject`

*"What exact work should runtime evaluation do?"* Because checking already did the heavy lifting, preparation is nearly free by design: a required-index completeness check, then `Arc` selection of the root DAG's facts into `ExecPlan` (46 lines of construction). There is a regression test asserting the reuse is pointer-identical (`exec_plan.rs:1544-1563` uses `Arc::ptr_eq`). `PreparedProject` adds the reusable surface: typed parameter ports, the binding builder (external values are lowered, closed-form-validated, evaluated against an empty environment, and domain-checked *at bind time*), model schema, and plan-id tagging so a binding row can never be replayed against a different plan.

### 3.9 Runtime → `RuntimeEvaluation`

*"What are the values?"* `eval/runtime.rs` walks `topo_order`, interpreting HIR bodies read directly from the checked TIR (`ExecPlan` holds no cloned bodies — verified). Failure containment is two-tier and consistently applied: `InternalError` and `Cancelled` abort; everything else becomes a per-declaration `NodeError`, and dependents are skipped with a root-cause message. Every `EvalContext` carries a non-optional `DagTIR` scope, so a lookup can never silently reinterpret a missing scope as the root DAG. Inline DAG calls get a fresh value environment, the callee's own source for diagnostics, and a shared work budget.

### 3.10 Output assembly → `EvalResult`

*"What does the user see?"* `RuntimeValue` is internal and SI-normalized; public `Value` carries dimension + display-unit metadata. The boundary is atomic: `EvaluatedValue::new(runtime_value, declared_type)` must be projected together, and any mismatch is an internal error rather than a silent untyped fallback. Display units are attached strictly after projection, with dynamic unit scales re-evaluated in the *defining* DAG's context; retained presentation value-maps (`EvaluatedPresentationCalls`) let display resolution reach callee-internal values without re-running the DAG — a genuinely elegant mechanism.

---

## 4. What the architecture gets right

These are the properties any refactor must preserve; several are better than what mainstream compilers do.

- **S1 — Phase values with private construction, all the way down.** `File<Raw>`→`File<Desugared>` (impossible sugar), `HirProject` (all-frozen guarantee), `CheckedProject` (all-checked guarantee), `SemanticRegistry` (post-freeze capability narrowing), `EvaluatedValue` (value+type atomicity), `ResolvedDomainConstraint` (private three-family carrier so quantity/int/datetime bounds cannot mix), `PositiveFiniteDisplayScale`, plan-id-tagged binding rows. The pipeline is forward-only in types, not just in convention.
- **S2 — One resolution stage.** All reference classification happens at the freeze boundary; downstream code pattern-matches typed variants. The tolerant/strict lowering split cleanly serves the LSP without ever letting `ExprKind::Error` into the batch path.
- **S3 — Canonical identity discipline.** `DagId` (structural, filesystem-free), `ResolvedName<Ns>`, `ResolvedIndexVariant`, `RuntimeDeclKey` (runtime maps cannot collide across same-leaf declarations from different DAGs), `ModulePathKey`, typed name newtypes with a single `NameAtom` leaf type. The Mars-Climate-Orbiter ethos is visibly encoded.
- **S4 — Check-time totality + cheap preparation.** Consts, schedules, and domain resolution all happen at check; `ExecPlan` is `Arc` selection; the reuse is pinned by a `ptr_eq` test. Runtime error containment is uniform across declarations, assertions, plots, presentation, and display projection.
- **S5 — Crate-level hygiene.** `graphcal-compiler` has zero internal dependencies; `graphcal-io` isolates the filesystem behind a capability-style reader (rooted real FS, in-memory, overlay); `graphcal-package` is a pure domain crate; Git I/O lives only in the CLI, and the loader *verifies* checkouts by re-hashing rather than trusting the cache.
- **S6 — Verification-guided development.** Lean specifications (`formal/`) with Model/Spec/Reference/Proofs structure, differential conformance oracles wired into the Rust tree (`ir/resolve/formal_conformance.rs`, `ir/static_external_surface_formal_conformance.rs` — 20-case V002 matrix, 55 external-surface scenarios), a mutation-testing ratchet, fuzzing, and Miri/ASan on the plugin ABI. For a codebase developed AI-heavy with minimal review, this is the right belt-and-suspenders, and it directly protects the areas a pipeline refactor would touch.
- **S7 — The reading guide is true.** Nearly every claim in `internals/codebase-reading-guide.md` checked out at the line level. The few divergences found are listed in F12. A codebase whose documentation can be trusted is itself an architectural asset.

---

## 5. Findings — where it can be cleaner

Ordered by architectural leverage. Each finding cites the evidence and states a concrete direction.

### F1 — The compiler's back half lives in the evaluation crate

**Evidence.**

- `graphcal-eval/src/project_compiler/` (6.3k lines) owns the HIR→TIR orchestration, import processing, and include instantiation — the second half of compilation.
- The loader contains a ~490-line pure elaboration pass: `build_module_resolver` plus its helper block (`loader.rs:1252-1732`) synthesizes include-instance modules (`owner.instance_child(prefix)`), propagates scopes/re-exports recursively for nested includes, injects instantiated indexes into importer namespaces, and checks include-expansion acyclicity. None of that is I/O.
- `merge_dependency` — the 687-line include-merge primitive in `graphcal-compiler` — has **zero callers inside its own crate**; its only caller is `project_compiler/lowering.rs`.
- `project_compiler/checking.rs:203` calls *down* into `exec_plan.rs` (an eval-named module) for check-time work (see F4), and `eval/project/prepare.rs` calls into the same module for the plan — one file serving two masters across the check/runtime boundary.
- `inline_dag.rs` and `import_surface.rs` sit outside `project_compiler/` purely to consume loader-owned vocabulary (`ModulePathKey`, `InlineBodyImportResolution`) without a module cycle — placement dictated by data ownership, not by role.
- A pre-HIR **AST rewrite pass runs inside the eval crate**: `rewrite_qualified_refs_in_compilation_body` (`pipeline.rs:74`, `qualified_refs.rs`) mutates the desugared AST before lowering.
- The split also breeds parallel bookkeeping: three per-`DagId` side tables (`ModuleTemplateStore`, `LoweringModuleInterface`, `ModuleArtifact`) overlap — `LoweringModuleInterface` duplicates the template's `Registry` + `ExternalDeclSurface` and adds only a derivable runtime-unit set (`model.rs:53-62`) — and in at least nine signatures the parameter name `module_artifacts` is bound to the `LoweringModuleInterface` map while `ModuleArtifact` is a different type used two files away (e.g. `imports.rs:71` vs `checking.rs:117`), the single most confusing naming in the layer.

**Why it matters.** The guide's own layering rule ("a file should only consume upstream files") is inverted at three points; names mislead (`graphcal-eval` is where most compilation happens; `check` lives partly in `exec_plan`); and the compiler's most complex subsystem (instantiation) is split across two crates, which makes it harder to reason about — and harder to eventually replace (F2).

**Direction.** Two workable end states:

1. *Three-crate split:* `graphcal-compiler` (pure per-DAG core, unchanged) ← `graphcal-project` (loader + module resolver construction + project compiler + check-time execution facts) ← `graphcal-eval` (prepare + runtime + output only). The loader's pure resolver-construction pass moves next to `ModuleResolver` itself.
2. *Minimum move, same crate:* relocate the check-time half of `exec_plan.rs` into `project_compiler/`, move `build_module_resolver` construction out of `loader.rs` into a `project_compiler/resolver_build.rs`, and move the loader vocabulary types to a neutral module so `inline_dag.rs`/`import_surface.rs` fold into `project_compiler/`.

Option 2 is mechanical and could be done incrementally today; option 1 is the honest end state. Either way, the *conceptual* pipeline does not change — this is about making the crate map tell the truth.

### F2 — Monomorphizing include instantiation is the largest accidental-complexity center

**How it works today.** An `include lib(mass: 5.0 kg) as p;` is processed in four acts:

1. *Classification* (`imports.rs:990-1267`): bindings are sorted into value/type/dim/index maps, validated against the dependency's declaration index, and recorded as a deferred 16-field `IncludeInstanceRequest`. (The inline-DAG variant, `imports.rs:1282-1535`, is a ~250-line near-verbatim duplicate of this that re-scans the DAG body per selective item where the file path uses an O(1) index.)
2. *Template fetch* (`lowering.rs:868-1134`): `ModuleTemplateStore` caches one elaborated `UnfrozenIR` + frontend `Registry` per canonical `DagId` — but every hit immediately does `template.unfrozen.clone()` **and** `template.frontend_registry.clone()` (`lowering.rs:912-913, 998-999, 1128-1129`), so the `Arc` deduplicates elaboration work only, never memory or per-site cost. Two of the three fetch branches are inlined re-implementations of the primary lowering functions (~215 lines).
3. *Merge* (`merge_dependency`, `ir/lower.rs:1857`, 687 lines): every dependency declaration is renamed under the prefix, its owner rebased onto `importer.instance_child(prefix)`, and its body rewritten by **three or four separate mutating AST traversals** (`substitute_indexes`, `substitute_type_names_in_expr`, `prefix_expr_refs`, plus a local-unit scan; three more per type annotation). A bound param's default is literally the include site's expression spliced in (`ir/lower.rs:2251-2265`). Source order is re-stitched by an O(n·m) interleave (`:1766`).
4. *Re-check per copy:* the importer's `UnfrozenIR` now contains N physical copies of the dependency for N include sites, and every copy is independently frozen, type-resolved, dimension-checked, and const-evaluated. **There is no per-template checked-result cache**, and the template store itself lives only for one `lower_project_perfile` call — the LSP pays full re-elaboration every analysis.

**The measured tax.** ~1,774 contiguous lines of `ir/lower.rs` plus ~370 diffused (≈37 % of the file); the `Unfrozen*`/frozen entry-type doubling exists chiefly so bodies stay syntactic until this rewriting is done; the loader's resolver-construction pass synthesizes the matching instance modules on its side (F1); `registry_merge.rs` exists to flatten dependency registries per instance (module-form file includes merge the dependency registry **twice** — once aliased at seed time, once flat at elaboration); and support machinery — `retarget_existing_resolution_owners`, `interleave_merged_source_order`, `add_selective_aliases_inner`, `UnitMergeConflict`, include-debug-name maps, `PendingOverrideReconciliation` — all exist because the merge prefix *is* the identity.

**The typed alternative is already half-built.** `merge_dependency` emits an `InstanceRecord` (`InstanceId` pairing template with concrete owner, plus a full `InstanceBindingEnvironment` of value/index/type/dimension substitutions) — but that record is a *side-description of a flattening that already happened*: it is read in exactly two places (a name-resolution fallback at freeze, `hir/expr.rs:2238-2260`, and graph-export clustering). The checker and evaluator consume the copied declarations, never the environment. The reading guide documents this honestly ("the current evaluator still monomorphizes declarations into the importer").

**What semantic instantiation would delete** (check the template once per canonical `DagId`; keep instances as `(template, InstanceBindingEnvironment)` edges with `DagId::instance_child` identity exactly as today): `merge_dependency` and all substitution/prefixing/rebasing helpers, the retarget/interleave/alias-synthesis machinery, `UnitMergeConflict`, the per-site template clones, the two inlined lowering re-implementations, and the string-erased substitution maps of F6. What survives, unchanged: binding classification and conformance, visibility/capability gating, `PureImport` vs `ConcreteInstance` runtime-unit semantics, recursion and generic-leakage checks (run per distinct binding signature instead of per site). Checking cost drops from O(instances × body) to O(distinct signatures × body). The Lean external-surface spec (`formal/Graphcal/Static/ExternalSurface.lean`) already models include projection with applicative specialization identities — the formal groundwork for this change exists.

**Two concrete defect candidates found in this machinery** (mechanics verified; user-visible impact needs a regression test):

- **Inline-DAG templates can record the wrong export surface.** The inline-DAG fetch branch stores `external_surface: extract_external_decl_surface(importer_ast)` — the *including file's* surface — (`lowering.rs:1124`), where the file-root branch correctly uses the dependency's own AST (`:994`) and the primary path uses the frozen body's own surface (`:618`). `ModuleTemplateStore::insert` is first-write-wins (`template.rs:56-61`), and `elaborate_include_instances` runs *before* `lower_inline_dag_modules` in `lower_file_to_hir` — so when a file `include`s its own inline DAG, the wrong-surface template is inserted first and wins. That surface then feeds the DAG's `LoweringModuleInterface` (`pipeline.rs:105-111`), which drives `exported_runtime_units` and export filtering for every downstream module importing that inline DAG.
- **`retarget_existing_resolution_owners` is called asymmetrically:** present in both template-miss branches immediately after lowering with the same owner (`lowering.rs:972, 1102`), absent from the otherwise line-for-line-equivalent primary paths. Either it is redundant in the branches or missing from the paths; the pair should be reconciled, because it bulk-rewrites the resolution owner of every existing entry.

### F3 — The check loop recomputes what it already knows

**Evidence.**

- **Declared types are resolved twice per DAG and rebuilt ≥4× per file.** Full HIR-type resolution (`resolve_declared_type_exprs`) runs once in `checking.rs:88-112` (`local_declared_interfaces`, needed so same-file inline-DAG imports can be typed) and again inside `type_resolve_dag` (`typed.rs:551`). The cheaper `build_declared_types` projection then runs in `dim_check/mod.rs:1432`, `checking.rs:209`, `pipeline.rs:170-178`, and `dim_check/mod.rs:1152` — plus **two unbounded call sites inside loops** (`dim_check/infer/hir.rs:3672` per field-bound per concrete struct application; `dim_check/mod.rs:1859` per bound), each allocating a fresh map including all built-in constants.
- **Every declaration body is traversed end-to-end by ten separate exhaustive `ExprKind` passes** across IR-lower/TIR-resolve/dim-check (dependency collection, unit-name collection, collection refs, constructor refs, policy checking, inference, ineffective-conversion scan, presentation resolution, extern scan, generic visitor). Collection-ref and constructor-ref passes are invoked back-to-back at three sites where one visitor with two sinks would do; `collect_override_dependency_summary` (`dim_check/mod.rs:1142-1196`) re-infers every parameter default that the main pass just inferred.
- **Import lookups scan linearly over all module artifacts.** `declared_type_for_target` and `value_for_target` (`checking.rs:22, 34`) do `module_artifacts.values().find_map(...)` per imported binding — O(files) per import — even though the artifacts are keyed by `DagId` and the target's owner is in hand.
- **Per-file un-Arc/re-Arc churn:** `check_dag_execution_facts` clones every inherited per-DAG fact struct out of its `Arc` (`exec_plan.rs:247-251`) and re-freezes at the end — O(files × DAGs) map rebuilds per project. `CheckedExecutionFacts::merge` deep-clones every `ResolvedDomainConstraint` per file.
- **Per-const map clones:** `visible_values_with_imports` clones the entire known-const map *inside the per-const evaluation loop* (`exec_plan.rs:416`) — O(consts × total consts); `known_const_values` is materialized twice in the same function (`:258`, `:284`).
- **Instantiation-layer clone tax** (beyond F2's per-site template copies): `store_and_freeze_module_template` clones the `UnfrozenIR` + `Registry` for *every* module, instantiated or not (`lowering.rs:609-610`); `merge_dep_dag_tirs` clones every dependency `DagTIR` per importer (`lowering.rs:824`); the qualified-ref rewrite clones the whole `File` AST whenever any alias exists (`qualified_refs.rs:85`); inline-DAG include classification clones the entire DAG body read-only (`imports.rs:1321-1323`).
- Assorted: `type_resolve_impl` deep-clones `ir.asserts` and the imported-binding map to satisfy borrows (`typed.rs:326-327`); `TirBuilder::new` clones the whole `ProjectTypeStore` which `checking.rs:176` then replaces wholesale; the loader's `module_declarations` linear-scans every file's inline DAGs on each nested-include lookup although an O(1) owner map exists (`loader.rs:1680-1693` vs `:1075-1082`).

**Why it matters.** None of this is a correctness bug — it is compile-time cost and, more importantly, *reviewability* cost: the same fact having four birthplaces means four places to check when semantics change. The unbounded `build_declared_types` sites make check time superlinear in constructor-application-heavy programs.

**Direction.** Three moves, in increasing depth:

1. Key artifact lookups by owner `DagId` (delete the linear scans); hoist the visible-const map out of the const loop; `Arc` the merged constraint maps; keep inherited facts as `Arc` and build only new DAGs owned.
2. Cache `DeclaredType` maps on `DagTIR` (a `OnceCell`, or materialize once at the top of `check_dimensions_tir` and thread it).
3. **Two-pass TIR construction (signatures for all DAGs, then bodies).** This deletes `resolve_hir_declared_types_with_modules_and_cancellation` outright — its own doc admits it exists only because inline-DAG imports need types before the TIR is built — and halves type-resolution cost. This is the structural fix; 1–2 are stopgaps.

Fusing the ten body passes is *not* recommended as a goal in itself — pass separation aids reviewability — but the adjacent-pair collectors and the re-inference in the override summary are free wins.

### F4 — `exec_plan.rs` is check-time code wearing a runtime name

**Evidence.** The `ExecPlan` producer is 46 lines (`exec_plan.rs:118-163`). Everything else in the 1.8k-line file — const scheduling and evaluation, runtime topological sorting, domain-bound resolution, struct-field-constraint resolution, compile-time const domain validation — is the implementation of `check_execution_facts_with_inherited`, whose only non-test caller is `project_compiler/checking.rs:203`. The doc comment on `compile` (`exec_plan.rs:62-71`) still describes the pre-split behavior ("performs: topological sort … compile-time evaluation") that the function no longer performs. Meanwhile `eval/project/prepare.rs` is an 82-line module with one caller whose entire job is `PreparedProject::from_compiled`'s job description, and `prepared.rs` (1.9k lines) fuses three separable subsystems: the prepared-project surface, the external-value binding compiler (which contains its own HIR lowering), and the Tenax model projection.

**Why it matters.** Module names are the first thing a reviewer trusts. Here the names actively misdirect: "plan compilation" that plans nothing, a "prepare" module that forwards, a "prepared" module that contains a compiler. The dependency arrow `project_compiler → exec_plan` also crosses the check/runtime boundary backwards.

**Direction.** Split `exec_plan.rs` into the thin plan selector (keep the name, ~120 lines) plus `const_schedule.rs` and `domain_resolve.rs` (or move the check-time entry beside `execution_facts.rs`/into `project_compiler/`); fold `prepare.rs` into `prepared.rs`; split `prepared.rs` three ways (`prepared.rs` / `binding_compile.rs` / `tenax_model.rs`, each < 700 lines). Fix the stale doc. No behavior change anywhere.

### F5 — Three overlapping type-metadata stores

**Evidence.** Dimensions, units, and indexes exist in full in *both* the per-file, leaf-keyed `SemanticRegistry` and the project-wide, owner-qualified `ProjectTypeStore` (the store is built by copying registries out: `typed/model.rs:565-593, 605-661`); declared index definitions additionally live in a third per-DAG cache (`semantic.collection_refs.index_defs`). Two sibling functions answer the same cardinality question through different stores (`infer/mod.rs:24-36` vs `infer/hir.rs:160-179`). Within TIR, the `SemanticRegistry` is consulted for exactly **one** semantic lookup (`get_finite_index`, `dim_check/infer/mod.rs:30`); every other use is dimension *formatting*. The per-file registry also forces a real semantic restriction: dependency DAGs must be skipped by `check_dimensions_tir` because re-checking them against the importer's leaf-keyed registry would fail on include-renamed types (comment at `dim_check/mod.rs:1003-1007`).

**Why it matters.** Beyond duplication, this is the codebase's own "one authority per fact" rule violated in its core; and the skip-dependency-DAGs constraint is a checking asymmetry imposed purely by a formatting-oriented data structure. The precedent for the fix already exists: nominal types were removed from `SemanticRegistry` at the freeze boundary (`registry/types.rs:56-73`) for exactly this reason — the de-duplication campaign stopped partway.

**Direction.** Narrow `SemanticRegistry`'s TIR-facing role to a `FormattingRegistry` (base-dim names/symbols + the one finite-index lookup relocated), make `ProjectTypeStore` the sole semantic authority, and drop the per-DAG index-def cache or make it the store's own memo. This dissolves the dep-DAG-skip constraint rather than working around it.

### F6 — Typed-identity holes, measured against the project's own constitution

The `CLAUDE.md` rules ban flat-string encodings, string-matched control flow, and stringify-inside-the-core. The core is overwhelmingly compliant; the residue is small, concentrated, and worth clearing precisely because these rules are the project's stated safety mechanism:

- **String-erased substitution maps in include rewriting** (`ir/lower.rs:2955-3300`): the generic helpers are keyed `K: Borrow<str>`, erasing the type-vs-dimension distinction (which is why `substitute_type_expr_nominal_names` must be called twice in a row, `:2225-2226`) — and on the re-parse round-trip `NameAtom::parse` failure the substitution is **silently skipped** (`:2962-2967`, also `:3270`, `:3288`). Practically unreachable (the maps hold already-valid typed names), but structurally fail-open in a fail-closed codebase.
- **`ExecPlan.assumes_map` and `expected_fail` are `ScopedName`-keyed** (`exec_plan.rs:46, 49`) beside four `RuntimeDeclKey`-keyed neighbors — the residual instance of exactly the collision class `RuntimeDeclKey` was built to close. (The `EvalResult`-level re-keying to `ScopedName` at `runtime.rs:645-659` is fine — that is a deliberate presentation-boundary conversion, #813.)
- **Parse-at-a-non-boundary:** plot/mark/composition properties are stored as typed `PlotPropertyName` at lowering, then stringified and re-parsed through linear-scan `from_name(field.name.as_str())` at every runtime plot evaluation (`runtime.rs:1416, 1430, 1553`), with parse failure mapped to an internal error. Store the typed variant on `LoweredPlotField` and match.
- **Leaf-string identity comparisons where canonical comparison is in scope:** `display.rs:700` (immediately after the correct resolved comparison on `:699`), `eval_expr/mod.rs:116`, `hir_eval.rs:2524`; plus `IndexVariantName::expect_valid(type_name.as_str())` (`display.rs:486`) — a panicking cross-namespace name conversion.
- **Banned-pattern composites, contained but present:** `format!("{}.{}", key.constructor, key.field)` (`exec_plan.rs:1079`) and recursion-path strings (`:1293, :1306, :1319`) built inside the core and passed onward as `&str` display names; `DiGraph<&str>` node keys in both topo sorts (`ir/lower.rs:3662, 3731`); five `&str`-typed filter closures in registry registration.
- **Duplicated key-construction rule:** `find_struct_field_constraint` exists verbatim in two modules (`exec_plan.rs:1242` = `eval_expr/mod.rs:120`).

None of these is currently a bug; each is a place where the convention, not the type system, is holding the line. All are locally fixable; the substitution-map one falls away entirely under F2's endgame.

### F7 — Invariants enforced dynamically that the type system could carry

- **`ExprKind::Error` containment is dynamic and inconsistent.** Producers are tolerant-only, but three TIR collectors quietly recurse into error nodes (`typed/collect.rs:508, 802`, `typed.rs:1519`) while inference hard-rejects them (`infer/hir.rs:657`). A `CheckedExpr` newtype produced by strict lowering would delete the match arms and make "batch never sees an error node" structural. Related: `lower_expr` is `pub(crate)`, so `prepared.rs:1562` re-implements it (tolerant + first error) outside the crate.
- **Correlated `Option`s on `ParamEntry`** (`default_expr`/`default_src`, `ir/lower.rs:163, 169`): five separate sites raise the identical "missing default provenance" internal error (`typed/collect.rs:305, 712`, `typed.rs:1331`, `dim_check/mod.rs:1164, 1472`). `default: Option<ParamDefault { expr, src }>` deletes all five.
- **Dead optionality:** `DimCheckContext.dag: Option<&DagTIR>` has one construction site (always `Some`) and six unwrap-to-internal-error sites (~40 unreachable lines).
- **Default-then-fill on `DagSemanticBody`:** 4 of 11 fields are constructed empty and installed by later passes (materialized shapes and presentation are installed by `check_dimensions_tir` mutating `&mut TIR`, with borrow-checker-forced re-lookups producing "DAG disappeared while installing" internal errors, `dim_check/mod.rs:1027-1058`); two more fields are mutated after `finish()` (`typed.rs:45-74`, `dim_check/mod.rs:1081-1112`). A `DagSemanticBody` split into per-phase parts (resolve-time / body-time / check-time) — or a seed→checked type step, extending the existing `DagTIRSeed` pattern one phase further — would make the fill order a type fact. Relatedly, `MaterializedShapeCollector` smuggles inference output through `Rc<RefCell<…>>` recorded at the tail of every inference call (`infer/hir.rs:86-129, 994-996`), so `infer_hir_type`'s impurity is invisible in its signature.
- **Write-only state:** `CheckedDagExecutionFacts.struct_field_constraints` is populated, flattened into the project-wide map, and never read per-DAG — dead weight on a public struct. Similarly, the runtime `output_surface` is computed and remapped only to be overwritten on the project path (`runtime.rs:495` → `output.rs:56` → discarded at `prepared.rs:840`).
- **The frozen/Unfrozen entry-type doubling** (8 pairs in `ir/lower.rs`) means one conceptual field addition touches ~5 sites. The AST already solved this exact problem with the `Phase` parameter; parameterizing the entry types over body type (`Entry<B>` with `B = ast::Expr` vs `hir::Expr`) would halve the model and delete the parallel merge/freeze loops' shape drift risk.

### F8 — God files at every hot spot

The reading order works, but several files exceed any reasonable review unit and mix layers internally:

| File | non-test LOC | Distinct concerns |
|---|---:|---|
| `ir/lower.rs` | ≈5.8k | data model / assembly / freeze / **include machinery (~37 %)** / registry construction (1,835 lines, orthogonal — takes `&mut RegistryBuilder`) / extern-plugin resolution (609 lines, one call site) |
| `tir/dim_check/infer/hir.rs` | 5.5k | inference / generic unification+substitution / concrete-obligation validation / nominal-dep collection; **27 `#[expect(too_many_arguments)]`** plus a module-blanket waiver; 40/45 signatures thread `registry` (a field of the `tir` they also take), 34 thread `declared_types`, all thread a `&'static` (already tracked as #1489) |
| `loader.rs` | ≈3.7k | 22 responsibilities in 5 orthogonal groups; the single-package and locked-package paths are **~1,500 lines of structurally parallel implementations** of the same six steps (two DFS loaders, three near-identical inline-dag lifting walkers differing only in one leaf function, pairwise-duplicated path/target/stdlib/sandbox logic) |
| `syntax/module_resolve.rs` | ≈4.0k | project-wide semantic resolver living under `syntax/` |
| `tir/typed.rs` | 1.9k | entry points / policy checker (~250 lines of non-type body-walk) / signature dependency analysis / builder |
| `eval/project/prepared.rs` | 1.9k | see F4 |

Each has clean seams already identified in the table above; the extractions are mechanical. The highest-value single split is `ir/lower.rs` → `ir/{model,assembly,freeze,include/,registry_build,extern_fns}.rs`, which would also finally let `HirDag` live in `hir/` where readers look for it (its aggregate definition sitting in `ir/lower.rs:413` is the one place the reading guide has to apologize for the layout). The loader's biggest lever is collapsing the two package paths behind one generic DFS.

### F9 — Entry-point combinatorics instead of a session builder

Eighteen public functions form a Cartesian product over {source, project} × {default host, host fns, host metadata} × {± cancellation} × {check, prepare, eval}: `check_project_with_host_metadata_and_cancellation` and seventeen siblings. The session type that should absorb this **already exists** (`ProjectCompiler`). Growing it to `ProjectCompiler::new(project).host_metadata(m).cancellation(t)` with `.check()` / `.prepare()` / `.eval(bindings)` finishers would collapse the surface to one builder plus 2–3 conveniences, and new options (e.g. a future budget or feature-gate) would stop doubling the function count. (`graphcal-wasm` and the planned #1080 batch API both want exactly this shape.)

### F10 — Loader boundary leaks

- **The documented single-resolver invariant is contradicted in code:** "the loader remains the only layer that resolves import paths" (`loader.rs:1243-1245`), yet `register_module_imports` falls back to `ModuleResolver::resolve_module_path` when the loader edge is absent (`:1715`) — two independent resolution algorithms (filesystem-driven vs scope-driven) can answer the same include path. Either make the fallback impossible (the loader always records the edge or a typed absence) or update the invariant and delete one path.
- **Ambient capability:** `package_cache_root()` reads process env inside the loader (`:2661`) with a test-only thread-local workaround — the one non-injected input in an otherwise capability-clean design; thread it as a parameter like the filesystem.
- **Dead plumbing:** `parent_dir` is computed and threaded through 11 sites and ignored at both leaves (`_parent_dir`); DAG recursion is validated twice (`session.rs:46` and per-file at `imports.rs:88`).

### F11 — No incrementality anywhere (a fact to own, not necessarily fix)

Two levels, one pattern:

- **The LSP recompiles the whole project per analysis** (`server.rs:1315` calls the batch `check_project_*`), with cancellation as the only mitigation, plus a separate tolerant symbol table. There is no memoization boundary (salsa-style or otherwise).
- **Every evaluation row re-runs the full topo order from an empty environment** (`runtime.rs:93-232`): the plan's const map and imported values are *copied* per row (`:112-119` — the copy defeats the `Arc` the design so carefully preserved), and identity re-resolution (`local_key`, `runtime.rs:348-352`) re-derives `ScopedName → RuntimeDeclKey` mappings 4–6× per declaration per row that `PreparedProject` could precompute once.

For current project sizes this is a defensible simplicity choice, and nothing in the architecture *blocks* incrementality later — per-file `ModuleArtifact`s are already the right memo granularity, and `PreparedProject` is the right place for a dirty-set evaluator when #1080-style sweeps demand it. The cheap wins now: seed rows by `Arc`/COW instead of copying; precompute the source-order key table; leave dependency-driven invalidation until a real workload needs it.

### F12 — Documentation and hygiene nits

- Stale docs found: `exec_plan.rs:62-71` (describes pre-split behavior); the loader's single-resolver claim (F10); the reading guide's Stage-6/8 file lists predate `ir/static_interface.rs`, `ir/static_dependencies.rs`, `ir/static_external_surface_formal_conformance.rs` (all 2026-08) — worth a `reading-order.py` re-run.
- `dim_check` is now the whole semantic type checker (its own module doc says so); the name undersells it to newcomers — `tir/check/` or `tir/typeck/` would be honest.
- **Zero `TODO`/`FIXME` markers repo-wide** — admirable as a convention (issues are the tracker), but several known warts found in this review are tracked nowhere. Filing the F3/F5/F7 items as issues would match the project's own "fence it and point at a tracking issue" rule.

---

## 6. Could it be simplified without losing safety?

**Direct answer: yes — five simplifications delete real machinery while preserving every check, and two of them strengthen safety. But the stage list itself should not shrink.** Each stage was audited for removability first:

| Stage | Keep? | Why |
|---|---|---|
| `File<Raw>` vs `File<Desugared>` | Keep both | The formatter needs `Raw`; the phase machinery costs 78 lines; folding desugar into the parser would smear sugar knowledge across every consumer. |
| Loader as separate stage | Keep | I/O, budgets, and package trust must stay in the imperative shell; the *elaboration* inside it moves out (F1), the stage remains. |
| `UnfrozenIR` (assembly) | Keep — shrinks under F2 | Whole-DAG shell checks (duplicates, visibility, V002) genuinely precede resolution. Today it also exists because include rewriting is syntactic; under semantic instantiation the *stage* remains for shell checks while its biggest client disappears. |
| Freeze / `HirDag` | Keep | The single-resolution boundary is the load-bearing safety wall. |
| `HirProject` (lower-all before check-all) | Keep | `ProjectTypeStore` requires complete HIR; merging the two loops would force incremental type-store mutation during lowering — more complexity, not less. |
| TIR / `CheckedProject` | Keep | The checked boundary with private construction is the totality guarantee. |
| `ExecPlan` / `PreparedProject` | Keep (thin by design) | Already ~free; folding it into `CheckedProject` would conflate "checked" with "root selected + ports built" and break the reuse story (`wasm`, #1080). |
| `RuntimeEvaluation` vs `EvalResult` | Keep | Internal-SI vs public-display separation is the unit-safety boundary. |

The five real simplifications, in dependency order:

1. **Truthful placement and naming (F1 + F4 + F12), first.** Pure motion: split `exec_plan.rs`, move check-time code under `project_compiler/`, relocate resolver construction out of the loader, fold the orphan modules in. Zero semantic change, and it creates the clean seams the deeper work needs. *Safety effect: neutral; review clarity improves.*
2. **Two-pass TIR construction (F3).** Resolve all DAG signatures, then check bodies. Deletes the duplicate declared-type pre-pass and its function, removes the per-DAG cache pressure, and turns an ordering convention ("inline imports need types early") into structure. *Safety effect: neutral-to-positive — one birthplace per fact.*
3. **Registry narrowing (F5).** `FormattingRegistry` for display, `ProjectTypeStore` as the one semantic authority. Deletes a full copy of dims/units/indexes and *dissolves the dependency-DAG re-check exemption* — strictly less special-casing in the checker. *Safety effect: positive.*
4. **Semantic include instantiation (F2) — the big one.** Bindings applied as typed substitutions at the HIR/TIR level against a template checked once per canonical `DagId`; instance identity stays `InstanceRecord`/`DagId::instance_child` exactly as today. Deletes the string-erased substitution visitors, the ref-prefixer, the re-lower-per-include, the O(n·m) source-order interleave, and the fail-open reparse skips (F6's worst item) in one move. *Safety effect: positive — replaces convention-driven rewriting with typed substitution; also the precondition for cheap per-file memoization later (F11).* This is a multi-week design change and should ride behind the Lean external-surface spec that already models include projection.
5. **Session builder (F9) + typed-identity sweep (F6) + dynamic-invariant conversions (F7)** as an ongoing ratchet, each item independently shippable.

What should **not** be simplified: the phase-parameterized AST, the strict/tolerant lowering split, `RuntimeDeclKey` and the canonical-identity plumbing, check-time const evaluation, the two-tier runtime containment, and the `graphcal-io` capability layer. Each is carrying weight proportional to (or less than) its cost.

---

## 7. Suggested sequencing

| Step | Contents | Risk | Payoff |
|---|---|---|---|
| 1 | F4 splits + F12 doc fixes + F3 quick wins (keyed artifact lookups, hoisted clones, `Arc` seeding) + F10 dead-plumbing deletions | Low — mechanical, behavior-preserving; existing snapshot/mutation suites cover it | Immediate reviewability + measurable check-time cost |
| 2 | F7 type-level conversions (`ParamDefault`, non-optional `DimCheckContext.dag`, `CheckedExpr`, delete write-only state) + F6 sweep | Low | Deletes ~10 internal-error paths; strengthens the constitution |
| 3 | F8 file splits (`ir/lower.rs` first; loader path unification second) + F9 builder | Low–medium | Unblocks parallel work on instantiation; halves loader review surface |
| 4 | F3 two-pass TIR; F5 registry narrowing | Medium | One authority per fact; removes checker special cases |
| 5 | F2 semantic instantiation (spec-first, behind the Lean external-surface model) | High — the one real design change | Deletes the largest subsystem; enables per-template caching and future incrementality |
| 6 | F1 crate split (or the minimum-move variant), once 4–5 settle the seams | Medium | The crate map finally tells the truth |

Steps 1–3 are safe to interleave with feature work. Step 5 is the only one that needs a design pause — and the project's verification workflow is exactly the right tool to de-risk it.

---

## Appendix A — Orchestration call map (verified)

```text
compile_and_eval(source)                                eval/mod.rs:45
└─ LoadedProject::from_source / load_project            loader.rs
└─ compile_and_eval_from_project
   └─ ProjectCompiler::with_cancellation                session.rs:41
   │    ├─ validate_project_dag_recursion               pipeline.rs:17
   │    └─ LoadedProject::build_module_resolver         loader.rs:1252
   └─ .lower() → lower_project_perfile                  pipeline.rs:206
   │    └─ per file in load_order:
   │         imports::process_file_body_declarations    imports.rs
   │         rewrite_qualified_refs_in_compilation_body qualified_refs.rs
   │         lowering::lower_file_to_hir                lowering.rs
   │           └─ ir assembly → UnfrozenIR::freeze → HirDag
   └─ .check_with_host_metadata → check_hir_project     pipeline.rs:294
   │    ├─ build_project_type_store (complete HIR)      pipeline.rs:255
   │    └─ per file in load_order:
   │         checking::check_hir_file                   checking.rs:115
   │           ├─ local_declared_interfaces             checking.rs:88
   │           ├─ resolve_imported_bindings             checking.rs:41
   │           ├─ tir::type_resolve_builder…            typed.rs:241
   │           ├─ merge_dep_dag_tirs                    lowering.rs
   │           ├─ dim_check::check_dimensions_tir…      dim_check/mod.rs:993
   │           ├─ exec_plan::check_execution_facts…     exec_plan.rs:240  ← const eval, topo sorts, domains
   │           └─ build_checked_entry_interface         entry_interface.rs
   │         verify_host_functions                      pipeline.rs:387
   │         root → CheckedProject | dep → store_module_artifact
   └─ prepare…  → prepare_checked_project               prepare.rs:13
   │    ├─ exec_plan::compile_checked…(Arc selection)   exec_plan.rs:118
   │    └─ PreparedProject::from_compiled               prepared.rs:250
   └─ evaluate…  → run_eval_loop_with_bindings          runtime.rs:93
   └─ assemble_normal_result → EvalResult               prepared.rs:794
```

## Appendix B — Known-debt issues this review corroborates

- #1489 — `infer/hir.rs` positional-argument signatures → inference context (F8)
- #1080 — compile-once / rebind / evaluate-many (F9, F11)
- #1491 — `RegistryBuilder` accessor duplication (F5-adjacent)
- #611 — `ScopedName` refcount sharing (F3)
- #608 — inline-DAG stripping per-declaration clones (F3)
- #609 — deterministic-iteration map types (S3-adjacent)
- #610 — `Cow` from `FileSystemReader` (F3-adjacent)
