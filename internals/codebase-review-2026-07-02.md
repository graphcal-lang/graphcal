# Codebase Review: Possible Bugs and Code Smells (2026-07-02)

Full-workspace review (~98k lines of Rust across 7 crates) performed as seven
parallel passes: syntax layer, HIR/desugar, registry, IR lowering/resolution,
TIR type & dimension checking, evaluator, CLI/io/package, LSP/formatter, and a
cross-cutting sweep (grammar drift, panic surface, numeric casts, CLAUDE.md
hard-rule compliance). Findings marked **[verified]** were reproduced
end-to-end with the compiled `graphcal` CLI or probe tests against the real
crates; all others were confirmed by direct code reading with quoted evidence.
Baseline: `cargo clippy --workspace --all-targets` is clean.

Severity scale: **high** = silent wrong results or unusable core feature;
**medium** = wrong behavior with visible symptoms, unsound checks, or safety
bounds dropped; **low** = edge cases, misleading diagnostics, latent traps;
**smell** = violates the project's own standards (CLAUDE.md) or invites future
bugs.

---

## 1. High severity

### H1. Builtin constants silently shadow user `const` declarations **[verified]**

`crates/graphcal-compiler/src/hir/expr.rs:1154` (`lower_bare_name_ref`) resolves
`BuiltinConst::parse(...)` *before* declaration lookup (repeated for qualified
fallbacks at `expr.rs:1374`), and `check_builtin_name_shadowing`
(`crates/graphcal-compiler/src/ir/resolve/mod.rs:89-128`) guards only
dims/types/indexes/units — never value declarations.

```
const node E: Dimensionless = 2.0;
node x: Dimensionless = E;
```

`graphcal check` passes; `graphcal eval` prints `E = 2` but `x = 2.718282` —
the user's value is silently replaced by Euler's number. Same for `PI`, `TAU`,
`SQRT2`, `LN2`, `LN10`. This is the Mars-Climate-Orbiter failure class the
project's charter names as the worst possible. (Constructor collisions *are*
caught; the gap is specific to builtins.) Related low-severity variant: time
scale names (`TT`, `UTC`, …) are also accepted as decl names, and references
then fail loudly-but-wrongly with "found time scale" (`expr.rs:1160`).

### H2. `scan()` folds in map-literal source order, not index order **[verified]**

`crates/graphcal-eval/src/eval_expr/hir_eval.rs:1149` iterates the source
`IndexMap` directly; single-axis map literals are built in *source* order
(`hir_eval.rs:874-885`), while docs (`docs/language/indexes.md:96`) promise
accumulation "across the index order".

```
index Phase = { A, B };
node x: Dimensionless[Phase] = { Phase.B: 10.0, Phase.A: 1.0 };
node y: Dimensionless[Phase] = scan(@x, 0.0, |acc, val| acc + val);
```

prints `y[B] = 10, y[A] = 11`; per docs it must be `y[A] = 1, y[B] = 11`.
Reordering a map literal — semantically a no-op (`runtime_value_equals` treats
reordered maps as equal, `eval_expr/arithmetic.rs:54-70`) — silently changes
computed results. Inconsistently, the multi-axis literal path normalizes the
*outer* axis to declaration order (`hir_eval.rs:891-923`) but not inner axes.
Secondary effects: FP `sum()`/`mean()` accumulation order follows literal order
(`aggregations.rs:42-52`), and plot channel alignment compares variant
*sequences* (`plot_data.rs:47-50`), so a literal-ordered channel vs. a
for-comp channel over the same index is rejected as incompatible. Fix:
normalize `Indexed` entries to index-declaration order at construction (note
`eval/runtime.rs:124-130` pairs range labels positionally and must be kept in
sync).

### H3. Range-index identity and dimension are never checked when a range loop variable indexes a collection **[verified]**

`crates/graphcal-compiler/src/tir/dim_check/infer/hir.rs:1428-1445`: in
`IndexArg::Var` position, the `InferredType::Scalar(_)` arm discards the loop
variable's dimension and only requires the target axis to be *some* range
index — never the same index, never the same dimension (contrast the
named-index arm directly above, which checks `label_index != &index`). The
runtime doesn't catch it either: `RuntimeValue::RangeLabel` is converted to a
positional key with no index-identity check
(`crates/graphcal-eval/src/eval_expr/hir_eval.rs:1092-1094`, unlike the
`Label` arm at 1081).

```
pub index TimeGrid = linspace(0.0 s, 2.0 s, step: 1.0 s);
pub index LenGrid  = linspace(0.0 m, 2.0 m, step: 1.0 m);
param v: Dimensionless[LenGrid] = for x: LenGrid { 1.0 };
node w: Dimensionless[TimeGrid] = for t: TimeGrid { @v[t] };
```

`graphcal check` → ok; `graphcal eval` prints values — a Time-keyed loop reads
a Length-keyed table positionally with no diagnostic anywhere. With unequal
cardinality it degrades to a runtime "variant not found"; equal cardinality is
fully silent. Existing test `scalar_range_loop_var_cannot_index_named_indexed_value`
(`dim_check/tests.rs:1434`) covers range-vs-named only, not
range-vs-different-range.

### H4. Formatter: a trailing comment on a function-call argument makes the whole file unformattable **[verified]**

`crates/graphcal-fmt/src/format/expr.rs:246-269` (`format_fn_call_expr`)
appends the trailing comment to the arg doc *before* the comma inserted by
`soft_parenthesized_list` (`format/mod.rs:242`), rendering `1.0 // first,` —
the comma (and in flat layout everything up to `)`) is swallowed by the line
comment. The reparse self-check then fails:
`min(\n    1.0, // first\n    2.0,\n);` →
`FormatError::Reparse(UnexpectedEof expected ')')`. Net effect: `format_source`
errors on valid input, and LSP `format_document`
(`crates/graphcal-lsp/src/formatting.rs:16`, `ok()?`) silently returns no edits
for the entire document. Map literals (`expr.rs:369-378`) and match arms
(`expr.rs:893-900`) already do this correctly (comma before trailing comment);
fn-call args are the odd one out.

---

## 2. Medium severity

### Parser / syntax

- **M1. Valid arithmetic rejected by the unit-expression lookahead [verified].**
  `crates/graphcal-compiler/src/syntax/parser/type_expr.rs:431-456`: the
  continuation heuristic contradicts its own comment — for `/ (` it continues
  into unit parsing and then fails on the number.
  `node x: Dimensionless = 459.3 W / (1.0 m^2);` →
  `UnexpectedToken { expected: "identifier", found: "number" }`, the exact
  example the comment says should work.
- **M2. Duplicate `min:`/`max:` domain bounds silently last-wins [verified].**
  `crates/graphcal-compiler/src/syntax/parser/type_expr.rs:144-191` accepts
  `param m: Mass(min: 1.0 kg, min: 2.0 kg) = 1.5 kg;`; downstream
  (`crates/graphcal-eval/src/exec_plan.rs:611`) simply overwrites, so a declared
  safety bound is silently discarded end-to-end. The plot parser rejects exactly
  this shadowing pattern (`DuplicatePlotField`); domain constraints — the actual
  safety feature — do not.
- **M3. Stack-overflow abort in `Expr` drop glue.**
  `crates/graphcal-compiler/src/syntax/ast/value.rs:476-487`: `Expr` has a
  stack-growth-guarded manual `Clone` but no `Drop` guard. Left-nested operator
  chains are built iteratively and intentionally not depth-limited
  (`parser/mod.rs:282-291`), so a ~100k-term chain parses fine and then aborts
  the process (including the LSP server) when the `File` drops — the exact
  failure `src/stack.rs` exists to prevent.
- **M4. Visitors skip expressions inside generic args.**
  `crates/graphcal-compiler/src/syntax/visitor.rs:56,83,272,294`: both visitors
  ignore `FnCall.type_args` / `ConstructorCall.generic_args`, whose
  `GenericArg::Type(TypeExpr)` can carry domain-bound `Expr`s. `f<Mass(min: @m)>(x)`
  hides `@m` from dependency collection (`ir/resolve/deps.rs`) and from the
  include-inlining rewriters (`ir/lower.rs`) — missing dependency edges /
  unrewritten refs, i.e. wrong output rather than a crash.
- **M5. Grammar/lexer drift: `scan`, `unfold`, `linspace`, `step` are hard-reserved [verified].**
  `grammar.ebnf:55-60` declares them contextual keywords; the lexer reserves
  them (`syntax/token.rs:77-84`) and `parse_any_ident` accepts only
  `Token::Ident` (`syntax/parser/mod.rs:603-611`). `param step: Int = 1;` fails
  with "unexpected token 'step'" although the source-of-truth grammar says it is
  legal. Also: string literals are lexed and consumed (`token.rs:89-90`,
  `ast/value.rs:510`, timezone args in `tir/dim_check/infer/hir.rs:929,982`)
  but **grammar.ebnf has no string-literal rule at all**.
- **M6. Import collisions for units/constructors are silently shadowed.**
  `crates/graphcal-compiler/src/syntax/module_resolve.rs:2159-2161,1403-1446,1698-1706`:
  `check_import_addition_exclusive_names` explicitly skips `Unit`/`Constructor`
  additions and local lookup wins, so `unit m` + an imported `m` silently
  resolves every reference to the local unit; the import is dead. All other
  namespaces hard-error (`DuplicateImportName`).
- **M7. Alias re-import spuriously "ambiguous".**
  `module_resolve.rs:1296-1335`: `resolve_bare_index_variant` never dedupes
  candidates by canonical identity, so `import lib.{ Phase, Phase as P };`
  makes a bare variant label ambiguous although exactly one canonical index
  exists.

### HIR / IR

- **M8. Self-cycle through `unfold`'s `init` escapes cycle detection [verified].**
  `crates/graphcal-compiler/src/hir/expr.rs:640-650` (`has_ref_outside_unfold`)
  blankets the whole `Unfold` subtree as safe, but `init` is evaluated before
  any previous step exists (`graphcal-eval/src/eval_expr/hir_eval.rs:1209`
  evaluates `init` before the self-key overlay at 1218-1225).
  `node y: Dimensionless[Step] = unfold(sum(@y), |p, t| @y[p] + 1.0);` passes
  `check` and fails at eval with a bogus ``undefined graph reference `@…y` ``
  instead of the `G001 cyclic dependency` a plain cycle gets. The sibling
  `Scan` arm (expr.rs:682-688) recurses into its `init` correctly.
- **M9. Inconsistent generic-parameter shadowing by syntactic position [verified].**
  `crates/graphcal-compiler/src/hir/lower.rs:410-475`
  (`lower_single_term_nominal_type`) resolves concrete indexes/types *before*
  `ctx.generic_scope`, while `lower_dim_term` (506-524) and
  `lower_index_expr_name` (566-583) check the generic scope *first*. Given a
  module-level `index I` and a generic param `I: Index`, `Box<I>` binds to the
  module index while `Dimensionless[I]` in the same declaration binds to the
  generic — the same spelled identifier means two different things, which can
  silently instantiate a generic with the wrong index.

### Type / dimension checking (TIR)

- **M10. `Neg` accepts every non-Bool type [verified].**
  `crates/graphcal-compiler/src/tir/dim_check/infer/rules.rs:478-490`: the arm
  rejects only `Bool` and forwards the type otherwise. `-i` on `i: Fin(N)`
  stays `Fin(N)`, defeating the compile-time bounds check
  (`@v[-i]` passes `check`, fails at eval with "negative value");
  `-datetime(...)` types as `Datetime` and fails at eval. Needs an explicit
  `Scalar`/`Int` match (and a decision for `Fin` → `Int`).
- **M11. Aggregations don't check the element type is scalar [verified].**
  `tir/dim_check/infer/hir.rs:601-608` forwards `*element` unvalidated while
  the evaluator requires finite scalar floats
  (`graphcal-eval/src/eval_expr/aggregations.rs:37-40`). `sum(@flags)` on
  `Bool[Phase]` and `sum(@c)` on `Int[Phase]` both pass `check` and fail at
  eval; for `Int` the checker's result type (`Int`) is a lie even in spirit —
  the runtime only produces `Scalar`. Same for `Datetime[I]` and multi-axis
  `T[I][J]`.

### Evaluator

- **M12. Unfold identity is a leaf-name string; same-leaf decls conflate [verified].**
  `crates/graphcal-eval/src/eval/runtime.rs:214-234` builds
  `UnfoldContext { self_name: &name_str }` from `name.member()` and
  `hir_eval.rs:1177-1186` resolves it via `ScopedName::local(self_name)` — for
  an include-merged `alias.n` this finds the *root* `n`. Today the checker's
  identical leaf lookup makes valid programs error loudly (bogus `D002`
  declared/inferred mismatch when a root decl shares the leaf); the eval-side
  lookup is one refactor from a silent wrong-length unfold. Also a direct
  CLAUDE.md violation (`self_name: &'a str` where a resolved name exists).

### Registry / diagnostics

- **M13. Duplicate diagnostic code `graphcal::M017`.**
  `crates/graphcal-compiler/src/registry/error.rs:1172` and `:1200` assign
  M017 to two unrelated errors (cross-file import in virtual package; index
  binding not an index) — `M019` is unused, so the second is almost certainly a
  typo. `M000` is likewise shared by `FileNotFound` (697) and
  `InvalidSourcePath` (702). Gaps `I008`, `M019`, `P013`, `S001` are undocumented.
- **M14. Dimension resolution loses the failing name, producing verified-wrong diagnostics.**
  `crates/graphcal-compiler/src/registry/dimension_registry.rs:10-34` returns
  `Ok(None)` without saying which term failed; callers then fabricate names:
  `unit foo: Blah = …` with unknown `Blah` reports ``unknown dimension `foo` ``
  (`ir/lower.rs:2911-2921`), and `dim Foo = Bar * Baz;` with unknown `Bar`
  reports ``unknown dimension `Foo` `` (`ir/lower.rs:2826`). The unit side was
  already fixed for exactly this (`registry/unit.rs:112-116` docs); the
  dimension side kept the old design.
- **M15. Misleading multi-term denominator rendering still live (issue #577).**
  `registry/format.rs:70-82` with the default `parenthesize_multi_denom=false`
  renders `m/s/s` as `"m/s * s"` (reads as `m`). Two user-facing paths still
  use the default: range-index display labels (`ir/lower.rs:3551`) and
  domain-bound display (`graphcal-eval/src/exec_plan.rs:959`).

### CLI

- **M16. Failed `const` evaluations print `ERROR` but exit 0.**
  `crates/graphcal-cli/src/main.rs:372-381` checks `result.params` and
  `result.nodes` but never `result.consts`, which can be `Err` at runtime
  (display-unit attachment failure,
  `graphcal-eval/src/eval/runtime.rs:406-416`). A CI pipeline gating on exit
  code treats the failed run as success. Upstream `EvalResult::has_errors`
  (`graphcal-eval/src/eval/types.rs:607`) has the same omission, and the CLI
  re-implements it instead of calling it.
- **M17. Text output renders small nonzero scalars as `0` (and `-0`).**
  `crates/graphcal-compiler/src/registry/format.rs:10-24`: `{value:.6}` then
  zero-trimming turns any `|v| < 5e-7` into `"0"`. `node leakage: A = 3.0e-9 A;`
  prints `leakage = 0 A` — silent wrongness in the display path; JSON output is
  unaffected, so the two formats disagree. There is no scientific-notation
  fallback for small magnitudes (large ones have the `< 1e15` branch).
- **M18. Dependency source hashing follows symlinks out of the materialized tree.**
  `crates/graphcal-cli/src/deps.rs:431` uses symlink-following `fs::metadata`
  on entries of a gix checkout that materializes symlinks (`deps.rs:306-318`).
  A malicious dependency committing `src/evil.gcl -> /home/user/.ssh/id_ed25519`
  gets the target's content hashed into `tree_hashes.sha256` and later read as
  source; a symlinked directory lets the walk escape the tree entirely.

### LSP / formatter

- **M19. Rename and code-action edits computed against a stale snapshot but applied to the live buffer.**
  `crates/graphcal-lsp/src/server.rs:1428-1447`, `rename.rs:180-201`,
  `code_actions.rs:33,89-93,124`: `latest_text`'s own doc (server.rs:118-124)
  says text-sensitive requests must read it, and completion/signature-help/
  formatting do — but rename, prepare-rename, and code-action use
  `analysis.source` (up to 300 ms debounce + 10 s analysis old, retained
  indefinitely while the buffer doesn't parse, #834). The `WorkspaceEdit` uses
  plain `changes` (no versioned `document_changes`, `rename.rs:210-214`), so
  the client applies old-text spans to the new buffer — silent corruption.
- **M20. Table-cell pre-rendering relocates row comments into expressions and wrecks alignment [verified].**
  `crates/graphcal-fmt/src/format/expr.rs:441-445` (also 557 and
  `decl.rs:866-881`): cells are rendered (consuming the shared comment cursor)
  *before* the per-row comment drain at `expr.rs:458`, so a comment belonging
  before row `Q` is embedded mid-expression with a hardline:
  `Q:  3.0 + // note about Q\n4.0;`, and its length is counted into column
  widths. Idempotent and reparse-clean, so the mangled output ships.

---

## 3. Low severity

### Parser / syntax

- **L1.** Nat generic args don't strip `_` separators: `eye<1_000>()` fails
  while `table[1_000]` parses (`syntax/parser/expr.rs:917-923`). [verified]
- **L2.** Slice-label axis qualifier never validated in `parse_table_sliced`
  (`syntax/parser/table.rs:334-345`) unlike the multi-decl path
  (`decl/multi.rs:397-405`); typos surface later as a confusing HIR error. [verified]
- **L3.** `parse_brace_expr` error spans point at the `{` instead of the
  offending token (`syntax/parser/expr.rs:619-656`). [verified]
- **L4.** Multi-decl arity diagnostic reports column count as slot count for
  v2 heterogeneous rows (`syntax/parser/decl/multi.rs:477-484`).
- **L5.** Missing comma between match arms lexes the next arm's pattern as a
  unit literal, producing a misleading diagnostic
  (`syntax/parser/compound.rs:29-42`, `expr.rs:533`). [verified]
- **L6.** `NatExpr` `Display` omits precedence parens; `graphcal-fmt` prints
  through it (`syntax/ast/value.rs:867-876`). Latent.
- **L7.** Colliding indexes from a second `include` dropped via `.or_insert`
  with no diagnostic (`module_resolve.rs:1090-1093`).
- **L8.** `impl From<String> for NamePath` panics on dotted input — a std trait
  conversion that aborts on exactly the shape the type represents
  (`syntax/names.rs:540-550`); should be `TryFrom`.
- **L9.** `AmbiguousIndexVariant.indexes` built from `HashMap::values()` —
  nondeterministic diagnostic ordering (`module_resolve.rs:1303-1333`); the
  single-segment `resolve_module_path` error names only one of two searched
  candidates (`:1350-1362`).
- **L10.** Lexer exponent regex forbids `1e1_0`, which grammar.ebnf permits
  (`syntax/token.rs:173`).

### HIR / IR / registry

- **L11.** `resolve_synthetic_child_decl_path` fabricates `ResolvedDeclName`s
  with no existence check on the graph-ref path (`hir/expr.rs:1468-1480`,
  unguarded caller at `:1458`; same pattern at `ir/lower.rs:1079-1088` and
  `tir/typed/collect.rs:1025-1044`). Traced as not currently user-reachable —
  producers verify membership first — but the API contract violates
  "invalid use impossible", and the dim-check backstop degrades the name to its
  leaf (`dim_check/infer/hir.rs:413`).
- **L12.** Unknown attributes on include/import items silently dropped instead
  of `UnknownAttribute` (`ir/lower.rs:1748-1764`), unlike declarations
  (`ir/resolve/mod.rs:534-541`).
- **L13.** `override_param_default` matches params by leaf name only
  (`ir/lower.rs:1310-1322`; caller `eval/project/mod.rs:470-497`): with merged
  instances, an override key `x` binds to the first leaf match with no
  ambiguity diagnostic.
- **L14.** `Rational`'s `Neg` can panic during dimension display: the
  `expect` reason ("negation overflow is impossible") is false for
  `i32::MIN`, reachable via `Dimension::pow` products
  (`dimension.rs:142-152`, `format_exponents` at `:333`). Adversarial input,
  but a compiler panic on a valid program.
- **L15.** `resolve_unit_expr_impl` silently returns an infinite/zero compound
  scale — `km^400` overflows raw `f64` math to `+inf` and the positive-finite
  invariant is dropped at the return type; only some callers re-validate
  (`registry/unit.rs:147-175`).
- **L16.** `sign(0) == 1` (`registry/builtins.rs:378-384`): `f64::signum`
  semantics (`signum(-0.0) == -1`) instead of the mathematical sign function;
  test-pinned, so deliberate, but surprising for an engineering language.

### Evaluator

- **L17.** Index-binding validation consults dependency registries in
  `HashMap` iteration order — acceptance vs. `IndexKindMismatch` can be
  nondeterministic when two deps define the same index name
  (`eval/project/imports.rs:437-446`).
- **L18.** `min()`/`max()` seed with `±INFINITY`; an empty collection would
  report "produced infinite result" instead of an empty-collection error
  (`eval_expr/aggregations.rs:54-76`). Currently unreachable (indexes have ≥1
  variant).
- **L19.** Domain checks cast `Int` to `f64` unchecked: for `|i| > 2^53` the
  cast rounds and declared bounds can silently pass/fail incorrectly
  (`domain_check.rs:41-47`). Same unenforced-smallness cast for
  `from_unix()/from_jd()/from_mjd()` (`eval_expr/hir_eval.rs:456-457`).
- **L20.** Negative-index handling differs between `IndexArg::Expr` (explicit
  "negative value" error) and `IndexArg::Var` (falls through to
  ``variant `#-3` not found``) (`hir_eval.rs:1095` vs `1107-1112`).

### CLI / package / io

- **L21.** `GitUrl` credential check: false positive on standard
  `ssh://git@…` and false negative on scp-form `user:token@host` — the
  embedded-token leak it exists to stop passes (`graphcal-package/src/lib.rs:225-235`).
- **L22.** `--plot FILE.html`/`--plot browser` silently write nothing when all
  plots are `#[hidden]` (`graphcal-cli/src/main.rs:327-335`); plot evaluation
  errors are invisible without `--plot` (`main.rs:318-325,378`).
- **L23.** `graphcal.lock` written non-atomically (`deps.rs:67`), and two
  concurrent `deps lock` runs can collide on the cache `rename` (`deps.rs:245`).
- **L24.** Cache-hit path re-hashes a possibly tampered tree and blesses it
  into the lockfile (`deps.rs:223-226`); the commit is never re-verified.
- **L25.** `Lockfile::validate` accepts cyclic dependency graphs
  (`graphcal-package/src/lib.rs:475-533`); only the CLI resolver has a
  `visiting` set.
- **L26.** `to_deterministic_toml` can emit lossy/invalid TOML: non-UTF-8
  `source_dir` corrupted via `to_string_lossy`, control chars emitted raw and
  break re-parsing (`graphcal-package/src/lib.rs:594-598,1202-1213`).
- **L27.** `InMemoryFileSystem::canonicalize` mishandles `/..` (PathBuf::pop
  removes the root), diverging from `std::fs::canonicalize`
  (`graphcal-io/src/in_memory_fs.rs:67-76`).
- **L28.** 2D table headers resolve display names through the first row only
  (`graphcal-cli/src/display.rs:224-229`).
- **L29.** TOCTOU races: overrides size-check vs read
  (`overrides.rs:180-198`), sandbox canonicalize vs re-traverse
  (`graphcal-io/src/real_fs.rs:76-83`). Low for a local CLI, but the sandbox
  documents symlink-escape rejection as a guarantee.

### LSP / formatter

- **L30.** Binop comment drain emits the rhs at column 0
  (`graphcal-fmt/src/format/expr.rs:233-242`). [verified]
- **L31.** `format_multi_decl` builds one text node with literal `\n` and
  hard-coded indents, bypassing `nest` — continuation lines land at column 0
  inside a `dag` body (`format/decl.rs:702-847`; same property for the
  verbatim comment-fallback slice at `format/mod.rs:160`). [verified]
- **L32.** File-trailing comments yield a double newline at EOF that never
  converges (`format/mod.rs:182-189`). [verified]
- **L33.** Diagnostics publish race between concurrent analyses of different
  documents sharing an imported URI — older merged snapshot can be published
  last (`graphcal-lsp/src/server.rs:316-340`). Self-corrects next cycle.

---

## 4. Code smells (measured against CLAUDE.md's own rules)

### Flat-string / typed-name violations in the core

- **S1.** `SymbolKey::TopLevel(name.to_string())` from a `ScopedName` — the
  banned stringified-qualified-name pattern; scoped names then silently fall
  back to `Range::default()` (`graphcal-lsp/src/diagnostics.rs:34`; same shape
  in `inlay_hints.rs:32-35`).
- **S2.** `check_generics_leakage` flattens typed names via `display_path()`
  and does semantic lookup through three `&str`-keyed maps with `or_else`
  fallthrough — indexes/types/dims are separate namespaces, so same-lexeme
  collisions resolve through the wrong map
  (`graphcal-eval/src/eval/project/lowering.rs:1656-1701`); decl categories as
  literal strings `"dim"`, `"unit"`, … (`lowering.rs:1545,1636-1645`).
- **S3.** `imports.rs:164` string-matches `attr.name.name.as_str() != "hidden"`
  although the typed `AttributeName` enum exists and is used everywhere else.
- **S4.** `check_dag_recursion` erases typed `DeclName` keys to
  `HashMap<&str, Vec<&str>>` for graph identity
  (`graphcal-eval/src/eval/project/imports.rs:1198-1215`); same pattern for
  inline-DAG import shadowing (`eval_expr/hir_eval.rs:1465-1482`) and the
  resolver's `&HashSet<&str>` binding-name API
  (`module_resolve.rs:1062`; `graphcal-eval/src/loader.rs:403-408`).
- **S5.** `ModuleResolveError.namespace: &'static str` (`"name"`, `"dag"`, …)
  with tests string-matching it, although `SurfaceNameKind` exists
  (`module_resolve.rs:2270-2315`); reserved namespaces inline as
  `== "graphcal" || == "std"` (`loader.rs:1974`).
- **S6.** `ScopedName` stores raw unvalidated segments; `ScopedName::local("a.b")`
  displays indistinguishably from a qualified name
  (`syntax/module_name.rs:32-37,119-124`).
- **S7.** `Resolved*Entry.name: String` instead of `DeclName` across seven
  entry types (`registry/resolve_types.rs:79-132`) while `source_order` in the
  same struct is typed.
- **S8.** `dimension_registry.rs:67` decides "is compound" by sniffing the
  *rendered* string (`canonical.contains([' ', '^', '*', '/'])`) — structure
  recovered from formatting output; the fact is available on `Dimension`.
- **S9.** Record-shape decided by cross-namespace lexeme comparison
  (`ConstructorName` vs `StructTypeName` string equality,
  `registry/type_def.rs:128`); `BTreeMap<String, Rational>` keyed by
  stringified `UnitRef` although `UnitRef: Ord` (`registry/format.rs:108-116`);
  builtin table keyed by `&'static str` although `BuiltinFnName` exists
  (`registry/builtins.rs:181`).
- **S10.** `epoch()` fabricates parser input by string concatenation
  (`format!("{s} {scale}")` fed to hifitime, `eval_expr/hir_eval.rs:629`) —
  correctness silently coupled to `Display` matching a third-party parser's
  vocabulary.
- **S11.** Struct-type lookup falls back to a bare-leaf registry lookup with no
  `TODO(#NNN)` fence (`tir/dim_check/helpers.rs:143-151`) — diamond imports
  with same-named types resolve to whichever leaf entry the registry holds.
- **S12.** Quick-fix keyword location re-derived by `starts_with` on the source
  line plus a byte count used as a UTF-16 column
  (`graphcal-lsp/src/code_actions.rs:23-26,171-189`).

### Backward-compat shims and dead code (banned in an unpublished project)

- **S13.** `pub type NatLinearForm = NatPolyForm;` explicitly labeled
  "Backward-compatible alias" (`nat.rs:138-139`).
- **S14.** `TypeDef::fields()` — an explicit backward-compat accessor returning
  `&[]` for unions, causing 4 live callers to silently treat unions as
  field-less records (`registry/type_def.rs:131-138`).
- **S15.** `DesugarSugar` trait has zero implementations; module docs instruct
  contributors to implement/wire it, which is impossible to follow
  (`desugar/mod.rs:9-57`).
- **S16.** `collect_graph_refs`/`collect_graph_ref_names` are dead exports with
  flat-string ref sets (`ir/resolve/deps.rs:72-99`); `ExclusiveUniverse` is
  write-only (`ir/resolve/mod.rs:61-87`); dead `_empty_locals`
  (`eval/runtime.rs:369`).

### Duplication / drift risks

- **S17.** Three raw-byte lookahead scanners re-implement lexing with divergent
  rules — one skips `/* */` comments that don't exist in the grammar, another
  skips none (`syntax/parser/expr.rs:739-896`).
- **S18.** `figure.rs`/`layer.rs` parsers ~100 lines duplicated verbatim;
  `NodeDecl`/`ConstNodeDecl` field-for-field duplicates
  (`syntax/ast/decl.rs:462-477`).
- **S19.** `scoped_name_to_name_path` and the freeze decl-bindings block are
  duplicated between `ir/lower.rs:786-812,1059-1096` and
  `tir/typed/collect.rs:1051-1063,945-979`.
- **S20.** Prelude name lists (`PRELUDE_DIMENSION_NAMES`/`PRELUDE_UNIT_NAMES`)
  are hand-maintained parallel to the `register_*` calls with no pinning test —
  consumed by the builtin-shadowing check, so a missed update silently disables
  protection for that name (`registry/prelude.rs:15-42`). Derived dimension
  algebra duplicated between `load_derived_dimensions` and `load_derived_units`
  with only the `N`/Force pair test-pinned (`prelude.rs:134-157` vs `206-214`).
- **S21.** `register_type` doc says first-wins; code and inline comment say
  last-wins (`registry/types.rs:290-307`). `EvaluatedFile::dag_tirs` doc still
  describes the banned `"alias::dag_name"` key scheme the code no longer uses
  (`eval/project/mod.rs:176-181`).
- **S22.** `TypeNameRef` derives `PartialEq`/`Hash` over the display leaf,
  which `with_display_leaf` allows to diverge from canonical identity — two
  references to the same resolved type can compare unequal
  (`registry/declared_type.rs:18-43`).
- **S23.** `unify_resolved_type` tolerates a generic-arg count mismatch via
  `zip` (re-opening the hole its own comment says it closes); currently no
  production callers but exported (`tir/typed/ops.rs:624-640`). Sibling:
  `infer_fn_dim_from_spans` ignores surplus args (`dim_check/builtins.rs:23-34`).
- **S24.** Formatter column alignment uses byte length, not display width
  (`format/expr.rs:449-452,564-570`; `decl.rs:725,732,857-889`); non-contiguous
  3D+ table slices emit duplicate slice headers (`format/expr.rs:670-677`).
- **S25.** CLI: `--format json` output is not accepted by `--input`
  (round-trip asymmetry); `convert_object` silently ignores unknown keys and
  has undocumented `variant`-over-`type` precedence (`json_input.rs:292-311`);
  duplicate `--set` flags and duplicate JSON keys silently last-win — implicit
  behavior in a project that bans it.
- **S26.** `graphcal-cli` `[build-dependencies]` re-pins `gix = "0.85.0"`
  instead of inheriting the workspace entry — Renovate bumps will silently
  drift (`crates/graphcal-cli/Cargo.toml`).
- **S27.** `Ord for Monomial` allocates two `Vec`s per comparison and is the
  `BTreeMap` key for every polynomial op (`nat.rs:118-124`);
  `NonEmpty` not used for `ForComp.bindings` / `domain_bounds`, leaving
  `bindings[0]`-style indexing on plain `Vec`s
  (`eval_expr/hir_eval.rs:965`, `exec_plan.rs:587`).

---

## 5. Strengths (verified clean areas)

- **Panic discipline is exceptional**: 16 non-test `unwrap`/`expect`/`panic!`
  sites across the entire workspace, every one carrying
  `#[expect(clippy::…, reason = …)]`; none plainly reachable from user input.
  LSP request handlers have no panics on malformed input; analysis is
  `spawn_blocking` + timeout with panic logging.
- **Numeric core is careful**: checked integer ops with explicit div/mod-by-zero
  errors, explicit range check instead of saturating `as` in `to_int`,
  finiteness enforced at every value construction, `overflow-checks = true` in
  release, deliberate/annotated float `==`.
- **Dimension algebra** (`dimension.rs`): checked arithmetic throughout,
  `RationalError` on overflow, canonical `BTreeMap` identity, property tests —
  marred only by L14.
- **LSP position conversion** (`convert.rs`) handles UTF-16 columns, surrogate
  pairs, char-boundary defense, and EOL/EOF clamping with round-trip tests
  including astral characters.
- **Span handling** is consistently byte-offset based; no unicode-index
  confusion found in the compiler.
- **Determinism**: topological sorts tie-break by name, `BTreeSet` ready
  queues, sorted key iteration in import merging (exceptions: L9, L17, S-level
  merge-conflict report ordering).
- **`graphcal-io` sandbox** canonicalizes correctly with real symlink-escape
  tests; `tests/cli.rs` is unusually strong (exit-code assertions, an
  invariant sweep asserting check-failure ⇒ eval-failure, end-to-end git
  package scenarios); the formatter's reparse + `format_equivalent` self-check
  means semantic corruption cannot ship (it fails closed — H4's symptom).
- **Datetime rules**, Pow exponent guards, map-literal/match exhaustiveness,
  and Nat-bounds machinery in the dim checker are symmetric and sound.

---

## 6. Prioritized recommendations

1. **Fix the four verified silent-wrong-value bugs first**: builtin-const
   shadowing (H1 — extend `check_builtin_name_shadowing` to value decls, or
   resolve decls before builtins), `scan` ordering (H2 — normalize `Indexed`
   to index order at construction), range-index identity (H3 — compare index
   identity + dimension in the `Scalar` arm and add a runtime identity check
   for `RangeLabel`), duplicate domain bounds (M2 — error like
   `DuplicatePlotField`).
2. **Close the checker/evaluator gaps** that let programs pass `check` and die
   at eval: `Neg` on non-numeric (M10), non-scalar aggregation elements (M11),
   unfold-init cycles (M8).
3. **Make the CLI exit code honest** (M16) by using/extending
   `EvalResult::has_errors`, and fix small-value display (M17 / H-adjacent)
   with a scientific-notation fallback.
4. **Fix the formatter trailing-comment bug** (H4) — one-line ordering fix,
   mirroring the map-literal path — and the comment-cursor pre-rendering
   design (M20).
5. **Reconcile grammar.ebnf with the lexer** (M5): decide contextual vs.
   reserved for `scan`/`unfold`/`linspace`/`step`, and add the string-literal
   rule.
6. **Burn down the CLAUDE.md-violation smells** (S1–S12) starting where they
   are load-bearing: the leaf-string unfold identity (M12), generics-leakage
   lookup (S2), and prelude-list pinning tests (S20) are the ones most likely
   to become the next silent bug.
