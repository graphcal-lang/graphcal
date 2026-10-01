# Refactor Metrics Ratchet

`internals/refactor-metrics.nu` counts code patterns that the invariant and
complexity refactor intends to remove. The checked-in baseline
`internals/refactor-metrics-baseline.toml` records the current counts, and CI
runs `nu internals/refactor-metrics.nu check`:

- A count above its baseline fails: the change reintroduced a pattern.
- A count below its baseline also fails until the baseline is lowered with
  `nu internals/refactor-metrics.nu update`, so improvements are locked in.

Run `nu internals/refactor-metrics.nu` to print the current counts. The
`reading_order_sccs` metric runs `internals/reading-order.py` through `uv`.

Run `nu internals/refactor-metrics.nu list` to see what is counted: it prints,
for every metric, each counted site as `path:line: <line>` (the line on which
the match starts), and `nu internals/refactor-metrics.nu list <metric>` limits
the output to one metric. The list uses the same sources and patterns as the
counts, so each metric's list has exactly as many entries as its count. For
`pipeline_layers_exceptions` the entries are the baseline's exceptions, and for
`reading_order_sccs` the cycles reported by `internals/reading-order.py`.

## Metrics

Counts cover production sources only: `tests.rs`, `tests/` directories, inline
`#[cfg(test)] mod … {` blocks, and comment lines are excluded.

| Metric | What is counted | Goal |
|---|---|---|
| `internal_error_calls` | Internal-error and invariant construction points in `graphcal-compiler`, `graphcal-eval`, and `graphcal-project`: calls of `InternalError::new`, `Invariant::violated`, and of every function, method, or closure whose name contains `internal` or `invariant` (for example `SemanticError::internal_error`, `into_internal_error`, local helpers), so a helper counts at each call site; `fn` definitions are not counted | Invariants carried by types instead of X001 fallbacks |
| `resolved_name_from_def_outside_resolver` | `Resolved*Name::from_def` outside `resolve/` (compiler, eval, LSP) | Resolved names are minted only by the resolver |
| `expect_valid_format` | `expect_valid(format!(…))` | No names fabricated from formatted strings |
| `too_many_arguments_expects` | `clippy::too_many_arguments` suppressions (compiler, eval, LSP) | Context structs instead of long positional argument lists |
| `build_declared_types_calls` | `.build_declared_types(` calls | Declaration records carry their resolved types |
| `module_resolve_str_key_lookups` | map lookups keyed by `.as_str()`, `.as_ref()`, or `&*` in `resolve/` | Typed keys instead of string keys |
| `pipeline_layers_exceptions` | `[[exception]]` entries in `internals/pipeline-layers/baseline.toml` | Dependency direction without exceptions |
| `reading_order_sccs` | strongly connected components of two or more files reported by `internals/reading-order.py` | An acyclic file dependency graph |
| `semantic_error_string_payloads` | `<field>: String` occurrences (payload fields of the diagnostic variants) in `graphcal-compiler/src/semantic_error/`; a `String` payload counts once however many sites construct it | Typed diagnostic payloads instead of pre-rendered text |

The counts are textual approximations. They exist to detect direction, not to
prove an invariant, so a metric may be refined when it miscounts, together with
a baseline update in the same change.
