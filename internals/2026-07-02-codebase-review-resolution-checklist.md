# Resolution checklist for `.local/2026-07-02_codebase-review.md`

Date: 2026-07-03

This file is the concrete audit trail for the 2026-07-02 review. It enumerates every finding ID present in the review file and maps it to the stacked branch/commit that resolved it plus focused verification evidence.

## Source enumeration

The review file contains exactly the following finding IDs in headings/list bullets:

- High: H1 H2 H3 H4 (4)
- Medium: M1 M2 M3 M4 M5 M6 M7 M8 M9 M10 M11 M12 M13 M14 M15 M16 M17 M18 M19 M20 (20)
- Low: L1 L2 L3 L4 L5 L6 L7 L8 L9 L10 L11 L12 L13 L14 L15 L16 L17 L18 L19 L20 L21 L22 L23 L24 L25 L26 L27 L28 L29 L30 L31 L32 L33 (33)
- Smell: S1 S2 S3 S4 S5 S6 S7 S8 S9 S10 S11 S12 S13 S14 S15 S16 S17 S18 S19 S20 S21 S22 S23 S24 S25 S26 S27 (27)

Extraction command used:

```sh
{ rg -o '^### (H[0-9]+)' -r '$1' .local/2026-07-02_codebase-review.md; \
  rg -o '^- \\*\\*(M[0-9]+)\\.' -r '$1' .local/2026-07-02_codebase-review.md; \
  rg -o '^- \\*\\*(L[0-9]+)\\.' -r '$1' .local/2026-07-02_codebase-review.md; \
  rg -o '^- \\*\\*(S[0-9]+)\\.' -r '$1' .local/2026-07-02_codebase-review.md; } | sort -V
```

## Final whole-stack verification

- `cargo fmt --check` — passed after latest code changes.
- `cargo check --workspace --all-targets` — passed after latest code changes.
- `cargo test --workspace --all-targets` — passed after the full stacked fix set.
- `but status -fv` — clean workspace before this evidence-only checklist was added.

## High severity

| ID | Status | Resolution / invariant now enforced | Branch / commit | Focused evidence |
|---|---|---|---|---|
| H1 | Resolved | User value declarations cannot silently shadow builtin constants/time scales; builtin-value names are rejected in declaration collection. | `review/safety-core` `2cf34b06` | `builtin_value_names_rejected`; full workspace tests. |
| H2 | Resolved | Indexed runtime collections are normalized to declaration order so `scan`, aggregations, and plot channels do not depend on map literal source order. | `review/safety-core` `2cf34b06` | `eval_scan_uses_index_order_for_map_literals`; full workspace tests. |
| H3 | Resolved | Range loop variables carry/check index identity and dimension; runtime range-label indexing rejects mismatched owners instead of positional reads. | `review/safety-core` `2cf34b06` | `range_loop_var_cannot_index_different_range_indexed_value`; full workspace tests. |
| H4 | Resolved | Function-call argument trailing comments render after commas, matching map/match behavior and preserving parseability. | `review/parser-formatter-fixes` `dbe31416` | `formats_fn_call_argument_trailing_comment_before_comma`; formatter snapshots. |

## Medium severity

| ID | Status | Resolution / invariant now enforced | Branch / commit | Focused evidence |
|---|---|---|---|---|
| M1 | Resolved | Unit-expression lookahead no longer consumes `/ (` arithmetic as a unit expression. | `review/parser-formatter-fixes` `dbe31416` | Parser unit/arithmetic regression tests; full workspace tests. |
| M2 | Resolved | Duplicate `min:`/`max:` domain bounds now produce `ParseError::DuplicateDomainBound`. | `review/safety-core` `2cf34b06` | Duplicate-domain-bound parser tests; full workspace tests. |
| M3 | Resolved | Deep expression drop/clone paths are guarded against stack overflow. | `review/parser-formatter-fixes` `dbe31416` | `deeply_nested_unary_chain_errors_instead_of_stack_overflow`; `long_operator_chains_are_not_depth_limited`. |
| M4 | Resolved | Visitors traverse expressions inside generic arguments, so dependency collection and include rewrites see domain-bound refs. | `review/parser-formatter-fixes` `dbe31416` | Visitor/dependency regression tests; full workspace tests. |
| M5 | Resolved | Contextual keywords are accepted where grammar permits identifiers, and grammar includes string literals. | `review/parser-contextual-tables` `554086ae`; `review/parser-formatter-fixes` `dbe31416` | Contextual identifier parser tests; grammar update; full workspace tests. |
| M6 | Resolved | Unit and constructor import collisions are rejected consistently with other namespaces. | `review/module-resolve-collisions` `04d8a212` | `unit_import_colliding_with_local_unit_is_rejected`; `constructor_import_colliding_with_local_constructor_is_rejected`. |
| M7 | Resolved | Bare variant resolution deduplicates by canonical resolved index identity. | `review/module-resolve-dedupe` `8c645add` | `alias_reimport_of_same_index_does_not_ambiguous_bare_variant`; full workspace tests. |
| M8 | Resolved | `unfold` init self-references are dependency cycles, not deferred eval undefined refs. | `review/checker-eval-gaps` `4fc56ace` | `unfold_init_self_reference_is_cycle`; full workspace tests. |
| M9 | Resolved | Generic parameters shadow same-spelled module type/index names consistently across type positions. | `review/generic-shadowing` `de853d8d` | `generic_index_param_shadows_same_named_module_index_in_type_args`. |
| M10 | Resolved | Unary negation accepts only numeric scalar/int cases and rejects `Bool`, `Datetime`, and `Fin` loop indices. | `review/checker-eval-gaps` `4fc56ace` | `negation_rejects_datetime`; `negation_rejects_fin_index_variable`. |
| M11 | Resolved | Aggregations reject non-scalar/int/datetime/multi-axis elements at check time. | `review/checker-eval-gaps` `4fc56ace` | `aggregation_rejects_non_scalar_elements`; `aggregation_rejects_int_elements`. |
| M12 | Resolved | Unfold self identity is keyed by typed runtime declaration identity, not leaf strings. | `review/runtime-url-identity` `be8fd71e` | `eval_unfold_uses_resolved_declared_range_index_owner_with_same_leaf_indexes`. |
| M13 | Resolved | Diagnostic code duplication corrected and gaps documented/avoided. | `review/small-correctness` `0518e593` | Error snapshot suite; full workspace tests. |
| M14 | Resolved | Dimension resolution preserves/report the failing term name instead of fabricating the declaration name. | `review/dimension-diagnostics` `40188b70` | Unknown-dimension diagnostics snapshots; full workspace tests. |
| M15 | Resolved | User-facing unit/dimension formatting parenthesizes multi-term denominators in range/domain displays. | `review/checker-eval-gaps` `4fc56ace` | Registry formatting tests and error snapshots. |
| M16 | Resolved | Failed const/display-unit evaluations count as errors via `EvalResult::has_errors`, and CLI uses the unified error predicate. | `review/checker-eval-gaps` `4fc56ace` | `eval_has_errors_true_when_node_fails`; CLI exit tests. |
| M17 | Resolved | Small nonzero text scalars use scientific fallback instead of rendering as `0`/`-0`. | `review/checker-eval-gaps` `4fc56ace` | `format_small_nonzero_decimal`; CLI text output tests. |
| M18 | Resolved | Dependency source hashing rejects symlinked source files/dirs instead of following outside the checkout. | `review/deps-symlink-hash` `d39e267f` | `hash_source_tree_rejects_symlinked_source_file`. |
| M19 | Resolved | Rename/code actions read current live text and use versioned/stale-safe edit computation. | `review/lsp-stale-edits` `4b66dfec`; `review/lsp-structured-edits` `36aad539` | LSP code-action/rename tests; full workspace tests. |
| M20 | Resolved | Formatter table-cell comment handling no longer consumes row comments during pre-rendering. | `review/formatter-table-comments` `f5cc378d` | `preserves_leading_comment_before_2d_table_row_without_embedding_in_cell`. |

## Low severity

| ID | Status | Resolution / invariant now enforced | Branch / commit | Focused evidence |
|---|---|---|---|---|
| L1 | Resolved | Nat generic arguments strip underscore separators consistently. | `review/parser-formatter-fixes` `dbe31416` | Nat generic parser regression tests. |
| L2 | Resolved | Table slice-label axis qualifiers are validated early. | `review/parser-contextual-tables` `554086ae`; `review/parser-low-diagnostics` `9e13b352` | Table parser diagnostics tests. |
| L3 | Resolved | Brace expression errors point at the offending token. | `review/parser-low-diagnostics` `9e13b352` | Parser delimiter diagnostic tests. |
| L4 | Resolved | Multi-decl arity diagnostics report slot counts correctly. | `review/parser-low-diagnostics` `9e13b352` | Multi-decl parser diagnostics tests. |
| L5 | Resolved | Missing match-arm commas produce direct delimiter diagnostics. | `review/parser-low-diagnostics` `9e13b352` | Compound parser diagnostics tests. |
| L6 | Resolved | `NatExpr` display parenthesizes by precedence. | `review/nat-display-precedence` `630fb31d` | Nat display tests. |
| L7 | Resolved | Colliding indexes from includes/imports are rejected, not dropped via first/last wins. | `review/module-resolve-collisions` `04d8a212` | `inline_include_index_collision_is_rejected`; full workspace tests. |
| L8 | Resolved | Panicking `From<String> for NamePath` conversions removed in favor of fallible/typed construction. | `review/namepath-conversions` `a07a5656` | NamePath conversion tests; full workspace tests. |
| L9 | Resolved | Ambiguous variant diagnostics are canonical-deduped and deterministically sorted. | `review/module-resolve-dedupe` `8c645add` | `alias_reimport_of_same_index_does_not_ambiguous_bare_variant`. |
| L10 | Resolved | Lexer exponent regex accepts underscores in exponents. | `review/small-correctness` `0518e593` | Lexer numeric literal tests. |
| L11 | Resolved | Synthetic child declaration path resolution validates existence instead of fabricating unresolved keys. | `review/synthetic-decl-existence` `966a60bd` | `inline_dag_include_and_call_share_body_import_semantics`. |
| L12 | Resolved | Unknown include/import item attributes are rejected via normal attribute validation. | `review/import-item-attributes` `efcafddb` | `project_selective_import_item_rejects_unknown_attribute`. |
| L13 | Resolved | Param overrides use scoped/typed names and report ambiguity instead of first leaf match. | `review/override-scoped-params` `64e546b0` | Override ambiguity/scoped-param tests. |
| L14 | Resolved | Dimension rational negation/display avoids `i32::MIN` panic paths. | `review/numeric-edge-cleanups` `0e5f3dc8` | Dimension overflow tests; full workspace tests. |
| L15 | Resolved | Compound unit scales are validated as positive finite values before registry exposure. | `review/registry-nat-cleanups` `64da295c` | `resolve_unit_expr_rejects_non_finite_compound_scale`. |
| L16 | Resolved | `sign(0)` now follows mathematical sign semantics. | `review/builtin-sign-zero` `008d4c1d` | `builtin_sign` tests. |
| L17 | Resolved | Index-binding validation is deterministic independent of dependency `HashMap` iteration order. | `review/index-binding-determinism` `0c59acc8` | `project_injectable_index_kind_mismatch`. |
| L18 | Resolved | Empty `min`/`max` aggregations have explicit empty-collection errors. | `review/aggregation-empty-errors` `5068aa52` | `empty_min_max_report_empty_collection`. |
| L19 | Resolved | Integer-to-float domain and datetime numeric conversions reject inexact/out-of-range casts. | `review/exact-int-casts` `acdccee3` | `rejects_int_too_large_for_exact_domain_comparison`; datetime cast tests. |
| L20 | Resolved | Negative range index variables report the explicit negative-index error. | `review/safety-core` `2cf34b06` | Negative index runtime tests; full workspace tests. |
| L21 | Resolved | Git URL credential detection accepts standard `ssh://git@...` and rejects credential-bearing/scp-like forms. | `review/runtime-url-identity` `be8fd71e` | `manifest_accepts_standard_ssh_git_user_url`; `manifest_rejects_scp_like_credential_url`. |
| L22 | Resolved | Plot failures and no-visible-plot cases affect CLI reporting/errors instead of silently succeeding. | `review/plot-error-exit` `a95a2dda` | `eval_plot_failure_reported_on_stderr`; `eval_plot_no_plots_warns`. |
| L23 | Resolved | `graphcal.lock` writes are atomic and concurrent cache rename collisions are handled. | `review/deps-atomic-lock` `83274a68` | Deps lock tests; full workspace tests. |
| L24 | Resolved | Cache hits are rematerialized/validated rather than blessing tampered source trees. | `review/deps-cache-rematerialize` `c3e5b498` | `package_consumers_reject_cached_source_hash_mismatch`. |
| L25 | Resolved | Lockfile validation rejects dependency cycles. | `review/package-cycle-validation` `520733d2` | `lockfile_rejects_dependency_cycles`. |
| L26 | Resolved | Lockfile TOML serialization is fallible/lossless and escapes control characters. | `review/lockfile-toml-escaping` `7e09b010` | `lockfile_round_trips_deterministic_toml`; `lockfile_toml_escapes_control_characters`. |
| L27 | Resolved | In-memory canonicalization keeps `/..` at root, matching std behavior. | `review/small-correctness` `0518e593` | `in_memory_canonicalize_parent_of_root_stays_root`. |
| L28 | Resolved | 2D table display headers use the row where each column key is observed. | `review/table-header-display` `a3fb9811` | `format_table_grid_column_headers_use_row_containing_column`. |
| L29 | Resolved | Override input reads use a single capped file handle; sandbox reads use the checked canonical path. | `review/input-file-handle-read` `3a050595`; existing `real_fs` tests | `parse_overrides_input_file_too_large`; `rooted_rejects_symlink_escapes`. |
| L30 | Resolved | Binop comment drain keeps RHS continuations indented. | `review/formatter-binop-comments` `c453d8cb` | `binop_rhs_after_trailing_comment_is_indented`. |
| L31 | Resolved | Multiline formatter docs/fallbacks use hardlines, so dag-body nesting applies to every line. | `review/multiline-doc-nesting` `449f6605` | `multi_decl_inside_dag_body_is_nested`; `multiline_text_uses_hardlines_for_nesting`. |
| L32 | Resolved | File-trailing comments produce a single convergent final newline. | `review/formatter-trailing-comments` `492deeb2` | `file_trailing_comment_has_single_final_newline`. |
| L33 | Resolved | LSP diagnostic publishing is generation-gated and open-document scoped to prevent stale imported snapshots winning. | `review/lsp-stale-edits` `4b66dfec`; `review/lsp-structured-edits` `36aad539` | LSP server diagnostics tests; full workspace tests. |

## Code smells

| ID | Status | Resolution / invariant now enforced | Branch / commit | Focused evidence |
|---|---|---|---|---|
| S1 | Resolved | LSP symbol keys remain structured instead of flattening scoped names to strings. | `review/lsp-structured-edits` `36aad539` | `symbol_key_helpers`; LSP diagnostics/inlay tests. |
| S2 | Resolved | Generics-leakage checks route through typed substitution/name categories instead of cross-namespace string fallthrough. | `review/runtime-url-identity` `be8fd71e`; `review/module-resolve-collisions` `04d8a212` | `project_pub_include_leaks_private_type_v006`; full workspace tests. |
| S3 | Resolved | Include/import item attributes parse to `AttributeName` instead of string-matching `"hidden"`. | `review/import-item-attributes` `efcafddb` | `project_selective_import_item_rejects_unknown_attribute`; hidden include item tests. |
| S4 | Resolved | DAG recursion/import identity uses typed declaration/runtime keys rather than `&str` graph identity. | `review/dag-recursion-typed` `e59d4bbd`; `review/runtime-url-identity` `be8fd71e` | `inline_dag_recursive_error`; runtime same-leaf tests. |
| S5 | Resolved | Module-resolution namespace handling is centralized through typed/surface namespace categories at boundaries; collisions no longer depend on ad-hoc string tests. | `review/module-resolve-collisions` `04d8a212` | Module collision tests; stdlib/deferred import tests. |
| S6 | Resolved | `ScopedName::local("a.b")` parses boundary dotted text into qualifier+leaf and cannot display as an ambiguous local leaf. | `review/scoped-name-boundary` `6b9f60ef` | `scoped_name_local_splits_dotted_boundary_text`. |
| S7 | Resolved | All `Resolved*Entry.name` fields use typed `DeclName`. | `review/resolved-entry-names` `4e12d433` | `cargo check --workspace --all-targets`; resolved-entry lowering tests. |
| S8 | Resolved | Dimension alias formatting uses `Dimension::is_compound()` instead of sniffing rendered strings. | `review/dimension-diagnostics` `40188b70` | Dimension formatting/diagnostic tests. |
| S9 | Resolved | Unit formatting is keyed by `UnitRef`, record-shape checks use typed atoms at the boundary, and builtin dispatch is covered by typed `BuiltinFnName` consistency tests. | `review/unit-format-typed-keys` `2a50b45c`; `review/value-decl-shape` `3bc54a4d` | `cargo test -p graphcal-compiler canonical_`; `builtin_function_tables_agree`. |
| S10 | Resolved | `epoch()` parses the date string and applies the explicit typed `TimeScale`; no `format!("{s} {scale}")` parser input fabrication. | `review/epoch-scale-parse` `997b800f` | `epoch_constructor_applies_explicit_scale_without_suffix_concat`; `epoch_constructor_rejects_embedded_scale_suffix`. |
| S11 | Resolved | Struct definition lookup is canonical-owner based only; the bare-leaf registry fallback was removed. | `review/struct-lookup-canonical` `21bb02b5` | `project_struct_type_uses_resolved_owner_with_same_leaf_types`; `project_struct_type_rejects_same_leaf_wrong_owner_constructor`. |
| S12 | Resolved | LSP code-action keyword edits use structured symbol/UTF-16-safe locations instead of line `starts_with` byte counts. | `review/lsp-structured-edits` `36aad539` | `find_keyword_position_uses_utf16_columns`; `v002_adds_pub_bind_with_indentation`. |
| S13 | Resolved | `NatLinearForm` backward-compat alias removed. | `review/registry-nat-cleanups` `64da295c` | Nat tests; full workspace tests. |
| S14 | Resolved | `TypeDef::fields()` backward-compat shim removed; callers use union/member APIs explicitly. | `review/remove-typedef-fields-shim` `8f4c1f44` | Type/union field-access tests; LSP symbol-table tests. |
| S15 | Resolved | Dead `DesugarSugar` trait removed. | `review/dead-code-cleanups` `0d083614` | `cargo check --workspace --all-targets`. |
| S16 | Resolved | Dead flat-string graph-ref collectors, write-only universe scaffolding, and unused eval locals removed. | `review/dead-code-cleanups` `0d083614` | `cargo check --workspace --all-targets`; full workspace tests. |
| S17 | Resolved | Raw parser lookahead scanners now share whitespace/comment/identifier helpers and remove non-grammar block-comment handling. | `review/parser-lookahead-helpers` `c92f2353` | `comparison_with_comment_containing_gt_paren_is_not_turbofish`; full workspace tests. |
| S18 | Resolved | Figure/layer parser duplication is shared through composition parsing, and node/const-node declarations share one `ValueDecl` shape. | `review/composition-parser-dedupe` `3f08b62c`; `review/value-decl-shape` `3bc54a4d` | `parse_figure_without_trailing_comma_after_last_field`; `parse_layer_without_trailing_comma_after_last_field`; `parse_node_with_compound_dim_type`. |
| S19 | Resolved | NamePath conversion and decl-binding duplication were consolidated around typed helpers. | `review/namepath-conversions` `a07a5656`; `review/synthetic-decl-existence` `966a60bd` | NamePath conversion tests; synthetic child validation tests. |
| S20 | Resolved | Prelude dimension/unit name lists are pinned against the loaded registry. | `review/prelude-list-pinning` `532914b8` | `prelude_name_lists_match_loaded_registry`. |
| S21 | Resolved | Stale doc comments were updated to match actual first/last-wins behavior and typed DAG keying. | `review/namepath-conversions` `a07a5656`; `review/runtime-url-identity` `be8fd71e` | Documentation comments compile; full workspace tests. |
| S22 | Resolved | `TypeNameRef` equality/hash use canonical resolved identity and ignore display leaf. | `review/type-ref-identity` `19806076` | `type_name_ref_equality_uses_canonical_identity_not_display_leaf`. |
| S23 | Resolved | Generic arg arity mismatches are rejected exactly in typed unification and builtin dim inference. | `review/generic-arity-guards` `733ddb87` | `generic struct argument count must match exactly`; builtin dim arity tests. |
| S24 | Resolved | Formatter alignment uses Unicode display width, and non-contiguous 3D+ table slices merge under one header. | `review/formatter-display-width` `881f2659` | `alignment_helpers_use_display_width_not_bytes`; `format_3d_table_merges_non_contiguous_slice_headers`. |
| S25 | Resolved | JSON input/override handling is explicit: unknown/ambiguous object keys and duplicate `--set` are rejected. | `review/json-input-explicitness` `7488bd67` | `ambiguous_object_rejected`; `object_unknown_key_errors`; `parse_overrides_rejects_duplicate_set_name`. |
| S26 | Resolved | `graphcal-cli` build-dependency `gix` inherits the workspace version. | `review/small-correctness` `0518e593` | `crates/graphcal-cli/Cargo.toml`; `cargo check --workspace --all-targets`. |
| S27 | Resolved | Monomial ordering avoids per-comparison `Vec` allocations; remaining non-empty invariants are enforced at parse/check boundaries. | `review/nat-monomial-ordering` `bd53cc45` | `cargo test -p graphcal-compiler nat`; full workspace tests. |
