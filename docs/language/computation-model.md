---
icon: material/graph
---

# Computation Model

Graphcal programs describe a **directed acyclic graph (DAG)** of computations. This page covers the core model: declaration kinds, the `@` sigil, evaluation semantics, and naming conventions.

## Declaration Kinds

Every top-level declaration belongs to one of four kinds:

| Kind | Keyword | Semantics | In DAG? |
|------|---------|-----------|---------|
| Parameter | `param` | User-supplied input, optionally with a default value | Yes |
| Node | `node` | Computed value derived from other values | Yes |
| Constant | `const node` | Compile-time immutable value | No |
| Assertion | `assert` | Post-evaluation boolean check | No |

### Parameters

```
param dry_mass: Mass = 1200.0 kg;  // optional param (has default)
param fuel_mass: Mass;              // required param (no default)
```

Parameters are the inputs to your computation graph. A param with a default value (`= expr`) can be overridden at runtime via `--set` or `--input`. A param without a default value is **required** — it must be provided via `--set`, `--input`, or a parameterized import binding. Evaluating a file with an unsatisfied required param is a compile error.

When any override is provided (via `--set`, `--input`, or parameterized import binding), **all** params must be explicitly provided by default. This strict mode prevents accidentally relying on stale defaults. Use `--allow-defaults` (CLI) or `#[allow_defaults]` (import attribute) to opt out. See [CLI Reference](../cli-reference.md#strict-parameter-override-mode) and [Multi-File Projects](multi-file.md#strict-binding-mode) for details.

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

Constants are evaluated at compile time before the DAG is built. They cannot reference parameters or nodes (the `@` sigil is prohibited in `const node` expressions).

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
| `@dag(args).out` | Inline DAG invocation projecting one output | Same as above |
| `NAME` | Built-in constant (`PI`, `E`, `TAU`, etc.) | Everywhere |
| `name` | Local variable (loop variable, match binding) | Expression bodies |

### Where `@` Is Allowed

| Context | `@` Allowed? |
|---------|-------------|
| `node` expression | Yes |
| `param` default value | No |
| `const node` expression | No |
| `dag` block body (inside `node` expressions) | Yes |

## Evaluation Order

1. **Parse** -- Source files are parsed into an AST
2. **Resolve** -- Names are resolved, imports are loaded
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
inline `@dag(args).out` expression form — each **syntactic call site** is a
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
