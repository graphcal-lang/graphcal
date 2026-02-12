# Computation Model

> How the computation graph works: nodes, edges, evaluation strategy.

## Status

**Decision level:** Mostly settled. Core primitives (`param`, `node`, `const`) are established.

## Summary

The fundamental unit is a **cell graph**: a directed acyclic graph (DAG) of named nodes. The runtime topologically sorts the graph and evaluates reactively. Changes to parameters propagate automatically to all dependent nodes.

## Node Kinds

| Kind | Keyword | Semantics |
| --- | --- | --- |
| Parameter | `param` | User-supplied input with a type and default value. Adjustable at runtime. |
| Computation | `node` | Derived value computed from other nodes. Can have multi-line body. |
| Constant | `const` | Immutable value. Not user-adjustable. Inlined by the compiler. |

```rust
param mass   = 5000 kg;        // input
const G0     = 9.80665 m/s^2;  // constant
node  thrust = @mass * @G0;    // computed
```

## Evaluation Strategy

- **Reactive:** changing a `param` triggers re-evaluation of all downstream `node`s.
- **Topological order:** the compiler sorts the DAG; cycles are compile errors.
- **Lazy vs eager:** Open question. Eager (spreadsheet-style: recompute everything downstream) is the default mental model. Lazy (only recompute when a value is demanded) could optimize large graphs.

## DAG Constraints

- The graph must be acyclic. Cycles are compile errors.
- Self-referencing nodes are not allowed (no `node x = @x + 1`).
- System dynamics (temporal feedback) uses `scan` over a time axis, not cyclic references (see [11-system-dynamics.md](./11-system-dynamics.md)).

## Open Questions

- **Lazy vs eager evaluation:** Should the engine recompute everything downstream of a changed param (eager/spreadsheet-style), or only recompute values when demanded (lazy)? Eager is simpler; lazy scales better for large graphs.
- **Incremental recomputation:** Should the engine track which nodes are "dirty" and only recompute those, or recompute the full topological order? Dirty-tracking is an optimization.
- **Parallel evaluation:** Independent branches of the DAG can be evaluated in parallel. Should this be automatic, or opt-in?
- **Error propagation:** If a node evaluation fails (e.g., division by zero), should the error propagate downstream as `Error` values (like Excel's `#DIV/0!`), or should the entire graph evaluation fail?

## Dependencies on Other Aspects

- **Syntax Design** ([02](./02-syntax-design.md)): How nodes are declared.
- **Scoping** ([08](./08-scoping.md)): How `@` references resolve graph edges.
- **Tables** ([10](./10-tables-and-autofill.md)): Tables are collections of nodes.
- **System Dynamics** ([11](./11-system-dynamics.md)): Temporal simulation as a pattern on top of the DAG.
