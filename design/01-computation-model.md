# Computation Model

> How the computation graph works: nodes, edges, evaluation strategy.

## Status

**Decision level:** Mostly settled. Core primitives (`param`, `node`, `const node`) are established. Evaluation strategy (hybrid eager/lazy with incremental recomputation) is resolved.

## Summary

The fundamental unit is a **cell graph**: a directed acyclic graph (DAG) of named nodes. The runtime topologically sorts the graph and evaluates reactively. Changes to parameters propagate automatically to all dependent nodes.

## Node Kinds

| Kind | Keyword | Semantics |
| --- | --- | --- |
| Parameter | `param` | User-supplied input with a type and default value. Adjustable at runtime. |
| Computation | `node` | Derived value computed from other nodes. Can have multi-line body. |
| Constant | `const node` | Immutable value. Not user-adjustable. Inlined by the compiler. |

```gcl
param mass        = 5000 kg;        // input
const node G0     = 9.80665 m/s^2;  // constant
node  thrust      = @mass * @G0;    // computed
```

## Evaluation Strategy

- **Reactive:** changing a `param` triggers re-evaluation of downstream `node`s.
- **Topological order:** the compiler sorts the DAG; cycles are compile errors.
- **Hybrid eager/lazy:** Nodes currently visible in the live view are recomputed eagerly (spreadsheet-style). Non-visible nodes are evaluated lazily (on demand). This provides responsive UX for displayed values while scaling to large graphs.

### Incremental Recomputation

The runtime uses **dirty tracking with early cutoff** (inspired by Salsa's red-green algorithm and Typst's comemo):

1. **Revision counter:** Each `param` change bumps a global revision counter. All downstream nodes become "potentially dirty."
2. **On-demand verification:** When a node's value is demanded, the runtime checks whether its inputs actually changed. If all inputs have the same values as the last evaluation, the cached result is reused without re-execution.
3. **Early cutoff (backdating):** If a node recomputes and produces the same value as before, its dependents are NOT marked dirty. This prevents unnecessary cascading recomputation through the graph. This is especially valuable for engineering calculations where many nodes are insensitive to small parameter perturbations (e.g., rounding, clamping, comparison nodes).

```gcl
param tolerance = 0.01;
param x = 3.14159;
node rounded = round(@x / @tolerance) * @tolerance;  // changes less often than x
node downstream = f(@rounded);                        // skips recomputation if rounded is unchanged
```

### Durability Classification

Inputs are classified by how frequently they change, enabling the runtime to skip validation for stable subgraphs:

| Classification | Graphcal concept | Change frequency |
| --- | --- | --- |
| Low | `param` adjusted by slider or input field | Every interaction |
| Medium | Scenario overlays | On scenario switch |
| High | `const node`, imported material properties | Rarely or never |

When a low-durability `param` changes, nodes that depend only on high-durability inputs (`const node`, external data) skip validation entirely. This optimization (borrowed from Salsa/rust-analyzer) eliminates redundant dependency-graph walks.

### Stable Node Identity

Nodes are identified by **name**, not by declaration order or source position. Reordering declarations, adding whitespace, or inserting comments does not invalidate any caches. Renaming a node is treated as a delete-and-create operation. This principle (learned from both Typst's span-number system and rust-analyzer's AstIdMap pattern) is critical for cache stability.

### Static Dependency Extraction

Unlike Salsa (which discovers dependencies at runtime) or comemo (which tracks observed accesses), Graphcal can extract the full dependency graph **statically** from `@` references in the source. This is a significant simplification: the DAG is known at compile time, enabling ahead-of-time topological sorting and parallel scheduling without runtime tracking overhead.

## DAG Constraints

- The graph must be acyclic. Cycles are compile errors.
- Self-referencing nodes are not allowed (no `node x = @x + 1`).
- System dynamics (temporal feedback) uses `scan` over a time axis, not cyclic references (see [11-system-dynamics.md](./11-system-dynamics.md)).

## Open Questions

- ~~**Lazy vs eager evaluation:**~~ **Resolved.** Hybrid eager/lazy: eagerly recompute displayed nodes, lazily evaluate the rest. See "Evaluation Strategy" above.
- ~~**Incremental recomputation:**~~ **Resolved.** Dirty tracking with early cutoff (backdating). See "Incremental Recomputation" above.
- **Parallel evaluation:** Independent branches of the DAG can be evaluated in parallel. The purity of `node` computations and the acyclic DAG constraint make this safe. A fork-join model (via Rayon) is the likely implementation: fork the evaluation context for independent branches, join results. Should be automatic for sufficiently large subgraphs.
- **Error propagation:** Errors are collected via an **accumulator pattern** (inspired by Salsa): diagnostics are gathered as a side channel during evaluation, not threaded through return types. This means errors don't contaminate memoization -- a node that produces the same value but a different warning doesn't trigger downstream recomputation. For the user-facing model, errors propagate downstream as error values (like Excel's `#DIV/0!`) so that independent parts of the graph can still evaluate. See [17-error-messages.md](./17-error-messages.md) for the diagnostic system.

## Prior Art

The evaluation architecture draws from:

- **Salsa** ([salsa-rs/salsa](https://github.com/salsa-rs/salsa)): Revision-based dirty tracking, early cutoff (backdating), durability classification, accumulator pattern for diagnostics. Used by rust-analyzer.
- **Typst/comemo** ([typst/comemo](https://github.com/typst/comemo)): Constraint-based memoization, stable identity for cache stability, age-based cache eviction. comemo tracks which parts of inputs were actually accessed, enabling cache hits even when unobserved parts of the input change.
- Research notes: `.local/2026-02-12_incremental-computation-insights.md`

## Dependencies on Other Aspects

- **Syntax Design** ([02](./02-syntax-design.md)): How nodes are declared.
- **Scoping** ([08](./08-scoping.md)): How `@` references resolve graph edges.
- **Tables** ([10](./10-tables-and-autofill.md)): Tables are collections of nodes.
- **System Dynamics** ([11](./11-system-dynamics.md)): Temporal simulation as a pattern on top of the DAG.
