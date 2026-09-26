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

## Metrics

Counts cover production sources only: `tests.rs`, `tests/` directories, inline
`#[cfg(test)] mod … {` blocks, and comment lines are excluded.

| Metric | What is counted | Goal |
|---|---|---|
| `internal_error_calls` | `internal_error(` calls in `graphcal-compiler` and `graphcal-eval` | Invariants carried by types instead of X001 fallbacks |
| `resolved_name_from_def_outside_resolver` | `Resolved*Name::from_def` outside `syntax/module_resolve*` (compiler, eval, LSP) | Resolved names are minted only by the resolver |
| `expect_valid_format` | `expect_valid(format!(…))` | No names fabricated from formatted strings |
| `too_many_arguments_expects` | `clippy::too_many_arguments` suppressions (compiler, eval, LSP) | Context structs instead of long positional argument lists |
| `build_declared_types_calls` | `.build_declared_types(` calls | Declaration records carry their resolved types |
| `module_resolve_str_key_lookups` | map lookups keyed by `.as_str()`, `.as_ref()`, or `&*` in `syntax/module_resolve*` | Typed keys instead of string keys |
| `pipeline_layers_exceptions` | `[[exception]]` entries in `internals/pipeline-layers/baseline.toml` | Dependency direction without exceptions |
| `reading_order_sccs` | strongly connected components of two or more files reported by `internals/reading-order.py` | An acyclic file dependency graph |
| `graphcal_error_variants` | variants of `GraphcalError` | Per-phase error types |

The counts are textual approximations. They exist to detect direction, not to
prove an invariant, so a metric may be refined when it miscounts, together with
a baseline update in the same change.
