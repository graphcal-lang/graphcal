---
icon: material/graph
---

# Computation Model

Graphcal programs describe a **directed acyclic graph (DAG)** of computations. This page covers the core model: declaration kinds, the `@` sigil, evaluation semantics, and naming conventions.

## Declaration Kinds

Every top-level declaration belongs to one of four kinds:

| Kind | Keyword | Semantics | In DAG? |
|------|---------|-----------|---------|
| Parameter | `param` | Named DAG input port, optionally with a default value | Yes |
| Node | `node` | Computed value derived from other values | Yes |
| Constant | `const node` | Compile-time immutable value | No |
| Assertion | `assert` | Post-evaluation boolean check | No |

### Parameters

```
param dry_mass: Mass = 1200.0 kg;  // optional param (has default)
param fuel_mass: Mass;              // required param (no default)
```

A `param` declares a named input port in the computation graph; it is not an
ordinary private declaration that becomes implicitly public. The declaration
kind itself supplies the external-input role, so params never take `pub` or
`pub(bind)`:

- A param in the entry DAG can be supplied with `--param`, `--params-json`, or
  `--params-json-file`.
- A param in a callable DAG can be supplied by name in an `include` binding or
  inline DAG call.
- Its effective value is also externally readable: callers may select it from
  an `include` or project it from an inline call. The result is the supplied
  binding, or the default when no binding was supplied.
- A param with a default (`= expr`) keeps that value when the caller does not
  supply the port.
- A param without a default is **required**. Leaving a required port unsatisfied
  is a compile error.

Use a private `node` for an internal computed value or a private `const node`
for an internal fixed value. If a sub-computation needs internal
parameterization, put it behind a private DAG or module boundary rather than
using an "internal param" naming convention.

Parameters (and nodes) can carry **domain constraints** that declare valid value ranges, checked at runtime:

```
param bus_mass: Mass(min: 100.0 kg, max: 2000.0 kg) = 500.0 kg;
```

See [Type System — Domain Constraints](type-system.md#domain-constraints) for details.

### Nodes

```
node total_mass: Dimensionless = @dry_mass + @fuel_mass;
```

Nodes are computed values. Their expressions can reference parameters, other nodes, and constants. Graphcal evaluates nodes in topological order determined by the dependency graph.

### Constants

```
const node g0: Acceleration = 9.80665 m/s^2;
```

Constants are evaluated at compile time before the DAG is built. They cannot reference parameters or nodes. DAG calls are anonymous runtime includes, so they are also prohibited in `const node` expressions and compile-time domain bounds.

### Assertions

```
assert fuel_positive = @fuel_mass > 0.0 kg;
```

Assertions are post-evaluation checks. They can reference parameters and nodes but are **not part of the DAG** -- no other declaration can reference an assert. Assertions are always evaluated last, after the entire graph. See [Assertions and Attributes](assertions.md) for full details.

## The `@` Sigil

The `@` prefix is the central scoping mechanism:

| Reference | Meaning | Allowed in |
|-----------|---------|------------|
| `@name` | Parameter, node, or const node in the graph | `node` expressions, `dag` block bodies |
| `@dag(args)::out` | Runtime DAG instantiation projecting one output | Runtime expressions only |
| `NAME` | Built-in constant (`PI`, `E`, `TAU`, etc.) | Everywhere |
| `name` | Local variable (loop variable, match binding) | Expression bodies |

All user graph declarations require `@`, including `const node` values. A bare
identifier never refers to a `param`, `node`, or `const node`; omitting the
sigil is a compile error. This keeps every runtime and compile-time dependency
edge explicit, so scheduling and cycle detection see the same graph expressed
by the source.

Time-scale spellings such as `UTC` and `TT` live in a separate static
namespace. A graph declaration may reuse one: `@UTC` selects the graph value,
while `Datetime<UTC>` and `epoch<UTC>(...)` select the time scale. Bare `UTC`
in an ordinary value expression remains a type error. Built-in numeric
constants such as `E` and `PI` are different: their spellings stay reserved in
the graph namespace because a missing `@` could otherwise select a valid
numeric value silently.

### Where `@` Is Allowed

| Context | `@` Allowed? |
|---------|-------------|
| `node` expression | Yes |
| `param` default value | No |
| `const node` expression | No |
| `dag` block body (inside `node` expressions) | Yes |

## Evaluation Order

1. **Parse** -- Source files are parsed into an AST
2. **Resolve** -- Imports are loaded; references are resolved while lowering to the compiler's internal representation
3. **Dimension check** -- All expressions are checked for dimensional consistency
4. **Const evaluation** -- Constants are evaluated in dependency order
5. **DAG construction** -- A dependency graph is built from `param` and `node` declarations
6. **Topological evaluation** -- Nodes are evaluated in topological order
7. **Assertion checking** -- Assert declarations are evaluated and reported

### Cycle Detection

Circular dependencies between nodes are detected at compile time:

```
node a: Dimensionless = @b + 1.0;
node b: Dimensionless = @a + 1.0;  // ERROR: cycle detected
```

### Call-Site Identity

When a `dag` is instantiated — whether via top-level `include` or via the
inline `@dag(args)::out` expression form — each **syntactic call site** is a
fresh instantiation. Two textually distinct occurrences with identical
arguments denote two distinct sub-graphs in the underlying DAG, not a shared
sub-graph. Programs must not rely on sharing across call sites.

An eval engine is free to detect structurally identical sub-graphs and reuse
their computation as an internal optimization, but this is not part of the
language semantics.

## Fault Isolation

If a node's evaluation fails (e.g., division by zero), only that node and its dependents are affected. Independent nodes still evaluate successfully.

## Naming Conventions

Graphcal recommends the following naming conventions:

| Declaration | Recommended Convention | Example |
|-------------|----------------------|---------|
| `param` | `lower_snake_case` | `dry_mass` |
| `node` | `lower_snake_case` | `total_dv` |
| `const node` | `lower_snake_case` | `g0`, `margin_factor` |
| `assert` | `lower_snake_case` | `fuel_positive` |
| `dag` | `lower_snake_case` | `orbital_velocity` |
| `type` | `PascalCase` | `TransferResult` |
| `dim` | `PascalCase` | `Velocity` |
| `index` | `PascalCase` | `Maneuver` |
| `unit` | (various) | `km`, `kN`, `MPa` |

These conventions are not enforced by the compiler, but following them is strongly recommended for consistency and readability.

## Comments

Graphcal supports line comments:

```
// This is a comment
param mass: Mass = 100.0 kg;  // inline comment
```

A `///` doc comment documents the declaration that follows it. A doc block is
a contiguous run of `///` lines placed directly above the declaration (no
blank line or `//` comment in between); it attaches through any leading
attributes. Doc comments never change program semantics — they surface in
editor hover (LSP) and as captions in generated output:

```
/// Specific impulse of the qualified engine.
param isp: Time = 320.0 s;
```

Note that `////` (four or more slashes) is an ordinary line comment, and a
`///` comment placed at the end of a code line documents nothing.
