# Ideas for Graphcal

## Researched / Incorporated into Design

- [x] Cache mechanism for nodes with the same input values (memoization).
  - Dirty tracking with early cutoff (backdating), inspired by Salsa and Typst/comemo.
  - Incorporated into [01-computation-model.md](./01-computation-model.md).
  - Research: `.local/2026-02-12_incremental-computation-insights.md`
- [x] Error handling.
  - Use miette crate for structured diagnostics with labeled spans, error codes, and multiple output formats.
  - Incorporated into [17-error-messages.md](./17-error-messages.md).
  - Research: `.local/2026-02-12_miette-insights.md`
- [x] Support pure functions that can be reused multiple times.
  - Designed as `fn` keyword with purity enforced by `@` prohibition.
  - See [12-pure-functions.md](./12-pure-functions.md).

## Open Ideas

- [ ] Provide parameter values via input files, command-line arguments, or environment variables.
- [ ] Apache Arrow Flight server.
- [ ] Write a validation for parameter values (e.g., non-negative mass).
- [ ] Durability classification for external data sources (material properties, physical constants) to optimize incremental recomputation.
- [ ] Constraint-based memoization (comemo-style) for rich data types like tables and datasets -- track which parts of the input were actually accessed.
- [ ] Parallel evaluation of independent DAG branches via Rayon fork-join.
- [ ] Accumulator-based error collection during evaluation (Salsa pattern) -- errors as side channel, not in return types.
- [ ] Age-based cache eviction for long-running live view sessions.
