# Graphcal Codebase Reading Checklist

Files to read in pipeline order, grouped by directory. See `codebase-reading-guide.md`
for the conceptual map.

## Stage 0 — Crate entry points (skim first)

- [ ] `crates/graphcal-compiler/src/lib.rs`
- [ ] `crates/graphcal-eval/src/lib.rs`
- [ ] `crates/graphcal-io/src/lib.rs`
- [ ] `crates/graphcal-fmt/src/lib.rs`
- [ ] `crates/graphcal-lsp/src/lib.rs`

## Stage 1 — Lexing primitives

- [ ] `crates/graphcal-compiler/src/syntax/span.rs`
- [ ] `crates/graphcal-compiler/src/syntax/token.rs`
- [ ] `crates/graphcal-compiler/src/syntax/lexer.rs`
- [ ] `crates/graphcal-compiler/src/syntax/comments.rs`

## Stage 2 — Typed names and DAG identity

- [ ] `crates/graphcal-compiler/src/syntax/names.rs`
- [ ] `crates/graphcal-compiler/src/syntax/dag_id.rs`

## Stage 3 — AST and phase machinery

- [ ] `crates/graphcal-compiler/src/syntax/mod.rs`
- [ ] `crates/graphcal-compiler/src/syntax/phase.rs`
- [ ] `crates/graphcal-compiler/src/syntax/ast.rs`
- [ ] `crates/graphcal-compiler/src/syntax/attribute.rs`
- [ ] `crates/graphcal-compiler/src/syntax/dimension.rs`
- [ ] `crates/graphcal-compiler/src/syntax/visitor.rs`

## Stage 4 — Parser

- [ ] `crates/graphcal-compiler/src/syntax/parser/mod.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/expr.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/type_expr.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/compound.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/table.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/mod.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/value.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/multi.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/dag.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/dim_unit.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/index.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/type_decl.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/import.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/figure.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/layer.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/plot.rs`
- [ ] `crates/graphcal-compiler/src/syntax/parser/decl/tests.rs`

## Stage 5 — Desugaring

- [ ] `crates/graphcal-compiler/src/desugar/mod.rs`
- [ ] `crates/graphcal-compiler/src/desugar/convert.rs`
- [ ] `crates/graphcal-compiler/src/desugar/desugared_ast.rs`
- [ ] `crates/graphcal-compiler/src/desugar/resolved_ast.rs`
- [ ] `crates/graphcal-compiler/src/syntax/desugar.rs`

## Stage 6 — Name resolution

- [ ] `crates/graphcal-compiler/src/syntax/name_resolve.rs`

## Stage 7 — IR lowering

- [ ] `crates/graphcal-compiler/src/ir/mod.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/mod.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/names.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/scope.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/deps.rs`
- [ ] `crates/graphcal-compiler/src/ir/resolve/tests.rs`
- [ ] `crates/graphcal-compiler/src/ir/lower.rs`

## Stage 8 — Registry

- [ ] `crates/graphcal-compiler/src/registry/mod.rs`
- [ ] `crates/graphcal-compiler/src/registry/types.rs`
- [ ] `crates/graphcal-compiler/src/registry/declared_type.rs`
- [ ] `crates/graphcal-compiler/src/registry/runtime_value.rs`
- [ ] `crates/graphcal-compiler/src/registry/resolve_types.rs`
- [ ] `crates/graphcal-compiler/src/registry/builtins.rs`
- [ ] `crates/graphcal-compiler/src/registry/prelude.rs`
- [ ] `crates/graphcal-compiler/src/registry/time_scale.rs`
- [ ] `crates/graphcal-compiler/src/registry/format.rs`
- [ ] `crates/graphcal-compiler/src/registry/manifest.rs`
- [ ] `crates/graphcal-compiler/src/registry/error.rs`

## Stage 9 — TIR and dimension checking

- [ ] `crates/graphcal-compiler/src/tir/mod.rs`
- [ ] `crates/graphcal-compiler/src/tir/typed.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/mod.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/helpers.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/builtins.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/mod.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/scalar.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/control.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/collections.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/infer/functions.rs`
- [ ] `crates/graphcal-compiler/src/tir/dim_check/tests.rs`

## Stage 10 — Filesystem abstraction (`graphcal-io`)

- [ ] `crates/graphcal-io/src/real_fs.rs`
- [ ] `crates/graphcal-io/src/in_memory_fs.rs`
- [ ] `crates/graphcal-io/src/overlay_fs.rs`

## Stage 11 — Loader and project orchestration

- [ ] `crates/graphcal-eval/src/loader.rs`
- [ ] `crates/graphcal-eval/src/eval/project/mod.rs`
- [ ] `crates/graphcal-eval/src/eval/project/pipeline.rs`
- [ ] `crates/graphcal-eval/src/eval/project/lowering.rs`
- [ ] `crates/graphcal-eval/src/eval/project/imports.rs`
- [ ] `crates/graphcal-eval/src/inline_dag.rs`

## Stage 12 — Execution plan and domain checks

- [ ] `crates/graphcal-eval/src/exec_plan.rs`
- [ ] `crates/graphcal-eval/src/domain_check.rs`

## Stage 13 — Runtime evaluator

- [ ] `crates/graphcal-eval/src/eval/mod.rs`
- [ ] `crates/graphcal-eval/src/eval/runtime.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/mod.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/arithmetic.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/collections.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/control.rs`
- [ ] `crates/graphcal-eval/src/eval_expr/functions.rs`
- [ ] `crates/graphcal-eval/src/eval/display.rs`
- [ ] `crates/graphcal-eval/src/eval/types.rs`
- [ ] `crates/graphcal-eval/src/eval/tests.rs`

## Stage 14 — CLI shell

- [ ] `crates/graphcal-cli/src/main.rs`
- [ ] `crates/graphcal-cli/src/overrides.rs`
- [ ] `crates/graphcal-cli/src/json_input.rs`
- [ ] `crates/graphcal-cli/src/display.rs`
- [ ] `crates/graphcal-cli/src/plot.rs`

## Stage 15 — Formatter (`graphcal-fmt`)

- [ ] `crates/graphcal-fmt/src/format/mod.rs`
- [ ] `crates/graphcal-fmt/src/format/decl.rs`
- [ ] `crates/graphcal-fmt/src/format/expr.rs`
- [ ] `crates/graphcal-fmt/src/format/type_expr.rs`

## Stage 16 — Language server (`graphcal-lsp`)

- [ ] `crates/graphcal-lsp/src/server.rs`
- [ ] `crates/graphcal-lsp/src/convert.rs`
- [ ] `crates/graphcal-lsp/src/resolve.rs`
- [ ] `crates/graphcal-lsp/src/cursor_context.rs`
- [ ] `crates/graphcal-lsp/src/symbol_table.rs`
- [ ] `crates/graphcal-lsp/src/diagnostics.rs`
- [ ] `crates/graphcal-lsp/src/hover.rs`
- [ ] `crates/graphcal-lsp/src/goto_definition.rs`
- [ ] `crates/graphcal-lsp/src/references.rs`
- [ ] `crates/graphcal-lsp/src/rename.rs`
- [ ] `crates/graphcal-lsp/src/completion.rs`
- [ ] `crates/graphcal-lsp/src/signature_help.rs`
- [ ] `crates/graphcal-lsp/src/inlay_hints.rs`
- [ ] `crates/graphcal-lsp/src/formatting.rs`
- [ ] `crates/graphcal-lsp/src/document_symbols.rs`
- [ ] `crates/graphcal-lsp/src/document_links.rs`
- [ ] `crates/graphcal-lsp/src/code_actions.rs`

## Stage 17 — Integration tests (read after the corresponding stage)

- [ ] `crates/graphcal-eval/tests/declaration_order.rs`
- [ ] `crates/graphcal-eval/tests/edge_case_bugs.rs`
- [ ] `crates/graphcal-eval/tests/error_snapshots.rs`
- [ ] `crates/graphcal-fmt/tests/format_tests.rs`
- [ ] `crates/graphcal-cli/tests/cli.rs`
