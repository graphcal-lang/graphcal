# Graphcal Codebase Reading Checklist

All Rust files in the workspace, in library-consumer order: every `use`d file appears before the file that imports it, so by the time you read a file you have already seen everything it imports. The order was derived from the actual `use`/`pub use` graph (re-exports resolved to the defining file) by `./reading-order.py`; re-run it after refactors to regenerate the order (stage headings are curated by hand as contiguous slices of its output). Keep this checklist in sync with dependency-affecting refactors so `AGENTS.md`'s topological-ordering guidance remains actionable. A few groups are mutually dependent and cannot be fully ordered; they are marked with a note and ordered with the most reusable definitions first. `mod.rs`/`lib.rs` files that only declare submodules carry no imports and appear early as a table of contents for their subtree. See `codebase-reading-guide.md` for the conceptual map.

## Stage 0 - Module maps and dependency-free leaves

- [ ] `crates/graphcal-compiler/src/syntax/attribute.rs`
- [ ] `crates/graphcal-compiler/src/syntax/non_empty.rs`
- [ ] `crates/graphcal-compiler/src/syntax/phase.rs`
- [ ] `crates/graphcal-compiler/src/syntax/mod.rs`
- [ ] `crates/graphcal-compiler/src/desugar/mod.rs`
- [ ] `crates/graphcal-compiler/src/registry/mod.rs`
- [ ] `crates/graphcal-compiler/src/registry/time_scale.rs`
- [ ] `crates/graphcal-compiler/src/registry/manifest.rs`
- [ ] `crates/graphcal-compiler/src/ir/mod.rs`
- [ ] `crates/graphcal-compiler/src/tir/mod.rs`
- [ ] `crates/graphcal-compiler/src/lib.rs`
- [ ] `crates/graphcal-compiler/src/dag_id.rs`

## Stage 1 - Spans, names, tokens, lexer

Note: `token.rs`, `comments.rs`, and `lexer.rs` are mutually dependent.

- [ ] `crates/graphcal-compiler/src/syntax/dimension.rs`
- [ ] `crates/graphcal-compiler/src/syntax/names.rs`
- [ ] `crates/graphcal-compiler/src/syntax/span.rs`
- [ ] `crates/graphcal-compiler/src/syntax/token.rs`
- [ ] `crates/graphcal-compiler/src/syntax/comments.rs`
- [ ] `crates/graphcal-compiler/src/syntax/lexer.rs`
- [ ] `crates/graphcal-compiler/src/syntax/nat.rs`

## Stage 2 - Core AST leaves and traversal

- [ ] `crates/graphcal-compiler/src/syntax/ast/common.rs`
- [ ] `crates/graphcal-compiler/src/stack.rs`
- [ ] `crates/graphcal-compiler/src/syntax/ast/value.rs`
- [ ] `crates/graphcal-compiler/src/syntax/ast/decl.rs`
- [ ] `crates/graphcal-compiler/src/syntax/visitor.rs`

## Stage 3 - Parser and surface desugaring

- [ ] `crates/graphcal-compiler/src/syntax/parser/mod.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/compound.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/type_expr.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/expr.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/table.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/dim_unit.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/index.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/type_decl.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/import.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/dag.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/layer.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/plot.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/figure.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/tests.rs`
- [ ] `crates/graphcal-compiler/src/syntax/desugar.rs`

## Stage 4 - Desugared AST phase

- [ ] `crates/graphcal-compiler/src/desugar/desugared_ast.rs`
- [ ] `crates/graphcal-compiler/src/desugar/convert.rs`

## Stage 5 - Registry and module resolution

Note: `registry/types.rs` and `registry/prelude.rs` are mutually dependent.

- [ ] `crates/graphcal-compiler/src/registry/format.rs`
- [ ] `crates/graphcal-compiler/src/registry/types.rs`
- [ ] `crates/graphcal-compiler/src/registry/prelude.rs`
- [ ] `crates/graphcal-compiler/src/registry/declared_type.rs`
- [ ] `crates/graphcal-compiler/src/registry/resolve_types.rs`
- [ ] `crates/graphcal-compiler/src/registry/error.rs`
- [ ] `crates/graphcal-compiler/src/registry/runtime_value.rs`
- [ ] `crates/graphcal-compiler/src/syntax/module_resolve.rs`

## Stage 6 - Name resolution (IR)

- [ ] `crates/graphcal-compiler/src/ir/resolve/names.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/deps.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/mod.rs`

## Stage 7 - HIR and builtin signatures

Note: `hir/types.rs`, `registry/builtins.rs`, `hir/lower.rs`, `hir/expr.rs`, and `hir/mod.rs` form a mutually dependent group.

- [ ] `crates/graphcal-compiler/src/hir/types.rs`
- [ ] `crates/graphcal-compiler/src/registry/builtins.rs`
- [ ] `crates/graphcal-compiler/src/hir/lower.rs`
- [ ] `crates/graphcal-compiler/src/hir/expr.rs`
- [ ] `crates/graphcal-compiler/src/hir/mod.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/builtins.rs`
- [ ] `crates/graphcal-compiler/src/hir/diagnostics.rs`

## Stage 8 - IR lowering, TIR, dimension checking, and late surface helpers

Note: `tir/typed.rs`, `tir/dim_check/helpers.rs`, and `tir/dim_check/mod.rs` are mutually dependent. `decl/multi.rs` and `decl/mod.rs` are also mutually dependent and appear here because their actual imports depend on AST aggregate/re-export files that sort after the core checker files.

- [ ] `crates/graphcal-compiler/src/ir/lower.rs`
- [ ] `crates/graphcal-compiler/src/tir/typed.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/helpers.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/mod.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/tests.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/mod.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/tests.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/rules.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/hir.rs`
- [ ] `crates/graphcal-compiler/src/syntax/ast/format_equivalent.rs`
- [ ] `crates/graphcal-compiler/src/syntax/ast/plot_props.rs`
- [ ] `crates/graphcal-compiler/src/syntax/ast.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/multi.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/mod.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/value.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/plot.rs`

## Stage 9 - Filesystem abstraction (`graphcal-io`)

Note: all four files are mutually dependent; `lib.rs` comes last because it re-exports and ties together the implementations.

- [ ] `crates/graphcal-io/src/in_memory_fs.rs`
- [ ] `crates/graphcal-io/src/real_fs.rs`
- [ ] `crates/graphcal-io/src/overlay_fs.rs`
- [ ] `crates/graphcal-io/src/lib.rs`

## Stage 10 - Package domain model (`graphcal-package`)

- [ ] `crates/graphcal-package/src/lib.rs`

## Stage 11 - Runtime values and expression evaluator

Note: `eval_expr/arithmetic.rs`, `eval_expr/unit_scale.rs`, `eval_expr/hir_eval.rs`, and `eval_expr/mod.rs` form a mutually dependent group.

- [ ] `crates/graphcal-eval/src/decl_key.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/numeric.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/conversions.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/aggregations.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/functions.rs`
- [ ] `crates/graphcal-eval/src/domain_check.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/arithmetic.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/unit_scale.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/hir_eval.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/mod.rs`
- [ ] `crates/graphcal-eval/src/lib.rs`
- [ ] `crates/graphcal-eval/src/exec_plan.rs`
- [ ] `crates/graphcal-eval/src/import_surface.rs`

## Stage 12 - Project loading and runtime orchestration

Note: `eval/types.rs`, `eval/display.rs`, `loader.rs`, `eval/plot_data.rs`, `eval/runtime.rs`, `eval/project/mod.rs`, and `eval/mod.rs` form a mutually dependent group.

- [ ] `crates/graphcal-eval/src/eval/types.rs`
- [ ] `crates/graphcal-eval/src/eval/display.rs`
- [ ] `crates/graphcal-eval/src/loader.rs`
- [ ] `crates/graphcal-eval/src/eval/plot_data.rs`
- [ ] `crates/graphcal-eval/src/eval/runtime.rs`
- [ ] `crates/graphcal-eval/src/eval/project/mod.rs`
- [ ] `crates/graphcal-eval/src/eval/mod.rs`
- [ ] `crates/graphcal-eval/src/inline_dag.rs`
- [ ] `crates/graphcal-eval/src/eval/project/lowering.rs`
- [ ] `crates/graphcal-eval/src/eval/project/imports.rs`
- [ ] `crates/graphcal-eval/src/eval/project/pipeline.rs`
- [ ] `crates/graphcal-eval/src/eval/tests.rs`
- [ ] `crates/graphcal-eval/src/graph_ir/mod.rs`
- [ ] `crates/graphcal-eval/src/graph_ir/dot.rs`

## Stage 13 - Formatter (`graphcal-fmt`)

Note: `format/type_expr.rs`, `format/expr.rs`, `format/decl.rs`, and `format/mod.rs` form a mutually dependent group.

- [ ] `crates/graphcal-fmt/src/format/type_expr.rs`
- [ ] `crates/graphcal-fmt/src/format/expr.rs`
- [ ] `crates/graphcal-fmt/src/format/decl.rs`
- [ ] `crates/graphcal-fmt/src/format/mod.rs`
- [ ] `crates/graphcal-fmt/src/lib.rs`

## Stage 14 - LSP prelude and CLI shell

Note: `json_input.rs`, `overrides.rs`, and `main.rs` form a mutually dependent group. `main.rs` consumes the `graphcal` library target's `format` module as well as binary-local modules, so the CLI package is ordered as one shell group here.

- [ ] `crates/graphcal-lsp/src/lib.rs`
- [ ] `crates/graphcal-lsp/src/convert.rs`
- [ ] `crates/graphcal-lsp/src/cursor_context.rs`
- [ ] `crates/graphcal-lsp/src/symbol_table.rs`
- [ ] `crates/graphcal-lsp/src/formatting.rs`
- [ ] `crates/graphcal-cli/src/display.rs`
- [ ] `crates/graphcal-cli/src/plot.rs`
- [ ] `crates/graphcal-cli/src/deps.rs`
- [ ] `crates/graphcal-cli/src/format.rs`
- [ ] `crates/graphcal-cli/src/json_input.rs`
- [ ] `crates/graphcal-cli/src/overrides.rs`
- [ ] `crates/graphcal-cli/src/main.rs`
- [ ] `crates/graphcal-cli/src/lib.rs`

## Stage 15 - Language server (`graphcal-lsp`)

Note: the feature modules from `resolve.rs` onward and `server.rs` are mutually dependent (each feature references `server::Backend`); the features come first because `server.rs` orchestrates them all.

- [ ] `crates/graphcal-lsp/src/diagnostics.rs`
- [ ] `crates/graphcal-lsp/src/resolve.rs`
- [ ] `crates/graphcal-lsp/src/completion.rs`
- [ ] `crates/graphcal-lsp/src/signature_help.rs`
- [ ] `crates/graphcal-lsp/src/inlay_hints.rs`
- [ ] `crates/graphcal-lsp/src/document_symbols.rs`
- [ ] `crates/graphcal-lsp/src/document_links.rs`
- [ ] `crates/graphcal-lsp/src/code_actions.rs`
- [ ] `crates/graphcal-lsp/src/goto_definition.rs`
- [ ] `crates/graphcal-lsp/src/references.rs`
- [ ] `crates/graphcal-lsp/src/rename.rs`
- [ ] `crates/graphcal-lsp/src/hover.rs`
- [ ] `crates/graphcal-lsp/src/server.rs`

## Stage 16 - Integration tests

- [ ] `crates/graphcal-eval/tests/declaration_order.rs`
- [ ] `crates/graphcal-eval/tests/edge_case_bugs.rs`
- [ ] `crates/graphcal-eval/tests/phase0_regressions.rs`
- [ ] `crates/graphcal-eval/tests/error_snapshots.rs`
- [ ] `crates/graphcal-fmt/tests/format_tests.rs`
- [ ] `crates/graphcal-cli/tests/cli.rs`
