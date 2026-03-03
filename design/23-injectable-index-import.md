# Injectable Index in Parameterized Import

> Allow indices to be passed as bindings in parameterized imports, making Graphcal files usable as reusable, index-generic libraries.

## Status

**Decision level:** Draft.

## Motivation

Parameterized imports currently allow injecting `param` values into a dependency, enabling reusable computation templates (e.g., the rocket equation with different masses). However, **indices** — the finite label sets used as table axes — are always hardcoded in the imported file. This prevents a file from being truly reusable when it needs to operate over a caller-defined set of labels.

Consider a power budget library:

```gcl
// power_budget.gcl — NOT reusable: Subsystem is hardcoded
index Subsystem { ADCS, Propulsion, Comms, Payload }

param power_draw: Power[Subsystem];
node total_power: Power = sum(for s: Subsystem { @power_draw[s] });
```

A different mission may have different subsystems. Without injectable indices, the library cannot be parameterized over the label set. The user must copy and modify the file, defeating the purpose of multi-file organization.

**Injectable indices** solve this by allowing the caller to supply the index definition, just as they supply param values.

## Design

### Principle: Indices Follow the Same Required/Default Pattern as Params

| Concept | Param | Index |
| --- | --- | --- |
| Has default | `param x: Length = 5.0 m;` | `index Phase { Design, Build, Test }` |
| Required (no default) | `param x: Length;` | `index Subsystem;` |
| Bound via import | `(x = 5.0 m)` | `(Subsystem = MySubsystems)` |

### Required Index Declaration

A new form of index declaration with no variants:

```gcl
index Subsystem;  // required — must be provided via parameterized import
```

This is syntactically minimal and directly analogous to `param x: Length;` (required param with no default value). A file containing required indices cannot be evaluated standalone, just like a file with required params.

### Binding Syntax: Reuse the Same Parentheses

Index bindings and param bindings coexist in the same `(...)` list of a parameterized import:

```gcl
index MySubsystems { ADCS, Propulsion, Comms }

import "./power_budget.gcl"(
    Subsystem = MySubsystems,
    power_draw = @my_power,
) { total_power };
```

The two kinds of bindings are visually distinguishable by Graphcal's existing naming conventions:

- **PascalCase = PascalCase** → index binding (`Subsystem = MySubsystems`)
- **snake_case = expr** → param binding (`power_draw = @my_power`)

No new syntax beyond what parameterized imports already support. The parser distinguishes the two by checking whether the LHS name refers to an `index` declaration or a `param` declaration in the dependency.

### Semantics: Index Substitution

During instantiation, an index binding performs a **name substitution**: every reference to the bound index name in the dependency is replaced with the provided index name.

Given `Subsystem = MySubsystems`:

| Location | Before | After |
| --- | --- | --- |
| Type annotation | `Power[Subsystem]` | `Power[MySubsystems]` |
| `for` binding | `for s: Subsystem` | `for s: MySubsystems` |
| Generic constraint | `<I: Index>` where `I = Subsystem` | `<I: Index>` where `I = MySubsystems` |
| Variant literal | N/A (cannot use with required index) | N/A |

The dependency's index registry entry for the bound index is **not** registered in the importer's scope — the importer already has the target index. Non-bound indices from the dependency are registered as usual.

### What a Library Can/Cannot Do with a Required Index

A required index has no variants defined, so the library can only use it **generically**:

| Operation | Allowed | Example |
| --- | --- | --- |
| Type annotation (axis) | Yes | `param x: Velocity[Subsystem]` |
| `for` comprehension | Yes | `for s: Subsystem { @x[s] }` |
| Generic function call | Yes | `total<Velocity, Subsystem>(@x)` |
| Index access with loop var | Yes | `@x[s]` where `s: Subsystem` |
| Aggregation | Yes | `sum(for s: Subsystem { @x[s] })` |
| Variant literal | **No** | `Subsystem::ADCS` — variants unknown |
| Map literal | **No** | `{ Subsystem::ADCS: ... }` — can't enumerate |

This constraint is a feature: it forces library code to be truly generic over the index, using `for` comprehensions and aggregations rather than hardcoded variants. The compiler rejects variant literals and map literal keys for required indices at the type-checking stage.

### Strict Binding Mode: Extends to Indices

The existing strict binding rule ("when any binding is provided, ALL params with defaults must be explicitly bound unless `#[allow_defaults]`") naturally extends to indices:

- **Required indices** (`index X;`) must always be bound when the import has any bindings.
- **Indices with default variants** (`index X { A, B }`) must also be explicitly bound under strict mode, unless `#[allow_defaults]`.

```gcl
// ERROR: Phase has defaults but isn't bound (strict mode)
import "./lib.gcl"(cost = @my_cost) { total };

// OK: all bindings provided
import "./lib.gcl"(Phase = MyPhase, cost = @my_cost) { total };

// OK: #[allow_defaults] lets Phase use its declared variants
#[allow_defaults]
import "./lib.gcl"(cost = @my_cost) { total };
```

### Overriding Default Indices

An index with declared variants can be overridden by an import binding:

```gcl
// lib.gcl
index Phase { Design, Build, Test }
param cost: Money[Phase] = {
    Phase::Design: 1000.0 USD,
    Phase::Build: 5000.0 USD,
    Phase::Test: 2000.0 USD,
};
```

```gcl
index MyPhases { Concept, Dev, Validation, Production }

import "./lib.gcl"(Phase = MyPhases, cost = @my_cost) { ... };
```

When `Phase` is overridden, the default value of `cost` (which references `Phase::Design` etc.) becomes invalid. This is safe because strict mode requires `cost` to also be explicitly bound. If `#[allow_defaults]` is used without binding `Phase`, the original variants and dependent defaults remain valid.

## Examples

### Minimal Example

```gcl
// total.gcl — generic summation over any index
index I;

param values: Dimensionless[I];
node total: Dimensionless = sum(for i: I { @values[i] });
```

```gcl
// main.gcl
index Color { Red, Green, Blue }

param color_weights: Dimensionless[Color] = {
    Color::Red: 0.3,
    Color::Green: 0.5,
    Color::Blue: 0.2,
};

import "./total.gcl"(I = Color, values = @color_weights) { total };
// @total == 1.0
```

### Multi-Index Library

```gcl
// thermal_analysis.gcl — reusable thermal model
index Component;  // required
index OpMode;     // required

param power_dissipation: Power[Component, OpMode];
param thermal_conductance: Power[Component]; // simplified

node temperature_rise: Temperature[Component, OpMode] = for c: Component, m: OpMode {
    @power_dissipation[c, m] / @thermal_conductance[c] * 1.0 K
};

node max_temp_rise: Temperature = max(
    for c: Component, m: OpMode { @temperature_rise[c, m] }
);
```

```gcl
// mission.gcl
index Payload { Camera, Antenna, Computer, Battery }
index Mode { Idle, Science, Downlink }

// ... param definitions for payload_power and payload_conductance ...

import "./thermal_analysis.gcl"(
    Component = Payload,
    OpMode = Mode,
    power_dissipation = @payload_power,
    thermal_conductance = @payload_conductance,
) { temperature_rise, max_temp_rise };

// @temperature_rise has type Temperature[Payload, Mode]
```

### Multiple Instantiations with Different Indices

```gcl
index Avionics { IMU, StarTracker, GPS }
index Propulsion { Thruster, Valve, Tank }

import "./thermal_analysis.gcl"(
    Component = Avionics,
    OpMode = Mode,
    power_dissipation = @avionics_power,
    thermal_conductance = @avionics_conductance,
) as avionics_thermal;

import "./thermal_analysis.gcl"(
    Component = Propulsion,
    OpMode = Mode,
    power_dissipation = @prop_power,
    thermal_conductance = @prop_conductance,
) as prop_thermal;

// Each instantiation has its own index:
// @avionics_thermal::temperature_rise : Temperature[Avionics, Mode]
// @prop_thermal::temperature_rise     : Temperature[Propulsion, Mode]
```

### Mixed Required and Default Indices

```gcl
// budget.gcl
index Phase { A, B, C, D }  // default set (mission phases are usually standard)
index LineItem;              // required (line items vary per project)

param cost: Money[Phase, LineItem];

node phase_total: Money[Phase] = for p: Phase {
    sum(for item: LineItem { @cost[p, item] })
};
```

```gcl
#[allow_defaults]  // Phase uses its defaults
import "./budget.gcl"(
    LineItem = MyItems,
    cost = @my_cost_matrix,
) { phase_total };
```

## Implementation Plan

### Phase 1: AST and Parser

1. Add `Required` variant to `IndexDeclKind`:

    ```rust
    pub enum IndexDeclKind {
        Named { variants: Vec<Spanned<VariantName>> },
        Range { start: Box<Expr>, end: Box<Expr>, step: Box<Expr> },
        Required,  // NEW
    }
    ```

2. Parse `index Foo;` as `IndexDeclKind::Required`.

3. In the tree-sitter grammar, add the `index IDENT ;` production.

4. In the import param binding parser, no changes needed — bindings are already `name = expr`. The distinction between index binding and param binding is made during semantic analysis, not parsing. For the RHS of an index binding, the "expression" will parse as an identifier which is then validated to be an index name.

### Phase 2: Semantic Analysis (Import Processing)

1. In `process_instantiated_import`, after parsing bindings, classify each binding as either a param binding or an index binding by checking whether the name refers to a `param` or `index` declaration in the dependency.

2. Store index bindings separately in `DeferredInstantiatedImport`:

    ```rust
    struct DeferredInstantiatedImport {
        // ... existing fields ...
        index_bindings: HashMap<IndexName, IndexName>,  // NEW: dep_index → importer_index
    }
    ```

3. Validate that each index binding's RHS is a valid index name in the importer's scope.

4. Extend the strict binding check to include indices with default variants.

### Phase 3: IR Merge — Index Substitution

1. In `merge_dependency`, add an `index_bindings` parameter.

2. Before prefixing expressions, perform index name substitution:
    - In type annotations (`TypeExpr`): replace bound index names.
    - In `for`-comprehension bindings: replace the index name.
    - In variant literals: should not appear for required indices (caught earlier).

3. When merging the registry, skip registering indices that are bound (the importer already has the target index).

### Phase 4: Type Checking

1. `IndexKind::Required` in the index registry indicates an unresolved index. Type-checking a file with unresolved required indices produces a clear error (unless they are being resolved via parameterized import).

2. Reject variant literals and map literal keys that reference a required index.

3. Allow required indices in type annotations, `for` bindings, and generic constraints.

### Phase 5: LSP, Docs, Tests

1. LSP: hover on a required index shows "required index (must be bound via import)".
2. Diagnostics: clear error messages for unbound required indices, invalid variant references.
3. Tree-sitter: update grammar, syntax highlighting.
4. Editor extensions: update TextMate grammar for `index Foo;` syntax.
5. Tests: add fixtures for required index imports, multi-index imports, strict mode interactions.
6. Documentation: update `docs/language/indexes.md`, `docs/tutorial/`, and `README.md`.

## Open Questions

- **Inline index definition in binding?** Should `(Subsystem = { A, B, C })` be allowed as a shorthand that creates an anonymous index? *Recommendation: no — require a named index for explicitness. Consistent with Graphcal's safety-over-usability philosophy.*

- **Range index injection?** Can a required index be bound to a range index? *Recommendation: yes. The library code only uses the index generically (`for` loops, aggregations), which work identically for named and range indices. This maximizes reusability.*

- **CLI binding for required indices?** Can `--set` or `--input` provide index definitions for a standalone file with required indices? *Recommendation: defer. Focus on parameterized import bindings first. CLI binding can be designed later as a natural extension.*

- **Index constraint syntax?** Should a required index declaration support constraints like minimum variant count or required dimension (for range indices)? E.g., `index TimeAxis: range(Time);` or `index Items: named;`. *Recommendation: defer. Start with unconstrained required indices. Constraints can be added later if needed.*

- **Can a required index appear in a const declaration?** E.g., `const weights: Dimensionless[I] = ...;`. Since const values must be fully evaluable at compile time and a required index has unknown variants, this seems problematic. *Recommendation: disallow required indices in const declarations for now.*

## Dependencies

- **07-indexes.md:** Extended with the concept of required indices.
- **Phase 4 (multi-file):** Parameterized imports are the binding mechanism.
- **20-type-system-stratification.md:** Required index is a new kind of type-level entity.
- **21-separate-label-indexes-from-tagged-unions.md:** The `Label(IndexName)` type must handle the case where `IndexName` refers to a required (unresolved) index during compilation of the library file.
