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

| Concept | Param | Named Index | Range Index |
| --- | --- | --- | --- |
| Has default | `param x: Length = 5.0 m;` | `index Phase { Design, Build, Test }` | `index TimeStep = range(0.0 s, 1.0 s, step: 0.1 s);` |
| Required (no default) | `param x: Length;` | `index Subsystem;` | `index TimeStep: range(Time);` |
| Bound via import | `(x = 5.0 m)` | `(Subsystem = MySubsystems)` | `(TimeStep = MyTimeStep)` |

### Required Index Declaration

A required index declaration has no variants but must declare its **kind** — named or range — because the two kinds have fundamentally different semantics:

| Kind | Loop variable type | Semantics |
| --- | --- | --- |
| Named | `Label(IndexName)` | Categorical labels — identity, not arithmetic |
| Range | `Scalar(Dimension)` | Numeric steps — supports arithmetic, has physical dimension |

Code written for a named index (comparing labels, matching on variants) cannot work with a range index (doing arithmetic on scalar values), and vice versa. Therefore, a required index must commit to a kind so the compiler can type-check the library in isolation.

**Named required index:**

```gcl
index Subsystem;  // required named index
```

**Range required index:**

```gcl
index TimeStep: range(Time);  // required range index over the Time dimension
```

The dimension constraint in `range(Time)` is mandatory — range index loop variables are `Scalar(Dimension)`, so the compiler needs the dimension to type-check expressions like `@velocity[t] * dt` inside the library.

Both forms are analogous to `param x: Length;` (required param with no default value). A file containing required indices cannot be evaluated standalone, just like a file with required params.

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

| Operation | Named Required | Range Required | Example |
| --- | --- | --- | --- |
| Type annotation (axis) | Yes | Yes | `param x: Velocity[Subsystem]` |
| `for` comprehension | Yes | Yes | `for s: Subsystem { @x[s] }` |
| Generic function call | Yes | Yes | `total<Velocity, Subsystem>(@x)` |
| Index access with loop var | Yes | Yes | `@x[s]` where `s: Subsystem` |
| Aggregation | Yes | Yes | `sum(for s: Subsystem { @x[s] })` |
| Arithmetic on loop var | **No** | Yes | `t * 2.0` where `t: TimeStep` (scalar) |
| `unfold` | **No** | Yes | `unfold(@x0, \|prev, curr\| { ... })` |
| Variant literal | **No** | **No** | `Subsystem::ADCS` — variants unknown |
| Map literal | **No** | **No** | `{ Subsystem::ADCS: ... }` — can't enumerate |

This constraint is a feature: it forces library code to be truly generic over the index, using `for` comprehensions and aggregations rather than hardcoded variants. The compiler rejects variant literals and map literal keys for required indices at the type-checking stage.

### Kind Matching: No Mixing Named and Range

When binding a required index, the bound index must match the declared kind:

| Required declaration | Can bind to | Cannot bind to |
| --- | --- | --- |
| `index Subsystem;` (named) | Named index | Range index |
| `index TimeStep: range(Time);` (range) | Range index with dimension `Time` | Named index, Range index with wrong dimension |

**Named → Named:** The bound index must be a named index (one with explicit label variants).

```gcl
// OK: Color is a named index
index Color { Red, Green, Blue }
import "./lib.gcl"(I = Color) { ... };

// ERROR: TimeStep is a range index, but I is a required named index
index TimeStep = range(0.0 s, 1.0 s, step: 0.1 s);
import "./lib.gcl"(I = TimeStep) { ... };
//  error: index kind mismatch: `I` requires a named index, but `TimeStep` is a range index
```

**Range → Range with matching dimension:** The bound index must be a range index whose dimension matches the declared constraint.

```gcl
// OK: MyTimeStep is a range index over Time
index MyTimeStep = range(0.0 s, 10.0 s, step: 0.5 s);
import "./sim.gcl"(TimeStep = MyTimeStep) { ... };

// ERROR: DistStep is a range index over Length, not Time
index DistStep = range(0.0 m, 100.0 m, step: 1.0 m);
import "./sim.gcl"(TimeStep = DistStep) { ... };
//  error: dimension mismatch: `TimeStep` requires range(Time), but `DistStep` has dimension Length
```

This rule is essential for type safety. A library that does `@velocity[t] * (t_next - t_prev)` relies on the loop variable having a specific physical dimension. Allowing a length-dimensioned range index would produce dimensionally incorrect results — exactly the kind of error Graphcal is designed to prevent.

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

### Range Index Injection

```gcl
// integrator.gcl — generic time-stepping library
index TimeStep: range(Time);  // required range index over Time

param x0: Length;
param velocity: Velocity[TimeStep];

node position: Length[TimeStep] = unfold(@x0, |prev_t, t| {
    let dt = t - prev_t;
    @position[prev_t] + @velocity[t] * dt
});
```

```gcl
// mission.gcl
index FlightTime = range(0.0 s, 100.0 s, step: 1.0 s);

param flight_velocity: Velocity[FlightTime] = for t: FlightTime { 100.0 m/s };

import "./integrator.gcl"(
    TimeStep = FlightTime,
    x0 = 0.0 m,
    velocity = @flight_velocity,
) { position };

// @position has type Length[FlightTime]
```

```gcl
// ERROR: cannot bind a named index to a range-required index
index Phases { Launch, Coast, Entry }
import "./integrator.gcl"(TimeStep = Phases, ...) { position };
//  error: index kind mismatch: `TimeStep` requires a range index (dimension: Time),
//         but `Phases` is a named index
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

1. Add required variants to `IndexDeclKind`:

    ```rust
    pub enum IndexDeclKind {
        Named { variants: Vec<Spanned<VariantName>> },
        Range { start: Box<Expr>, end: Box<Expr>, step: Box<Expr> },
        RequiredNamed,                             // NEW: `index Foo;`
        RequiredRange { dimension: DimExpr },       // NEW: `index Foo: range(Time);`
    }
    ```

2. Parse `index Foo;` as `IndexDeclKind::RequiredNamed`.

3. Parse `index Foo: range(DimExpr);` as `IndexDeclKind::RequiredRange`.

4. In the tree-sitter grammar, add the `index IDENT ;` and `index IDENT : range ( DimExpr ) ;` productions.

5. In the import param binding parser, no changes needed — bindings are already `name = expr`. The distinction between index binding and param binding is made during semantic analysis, not parsing. For the RHS of an index binding, the "expression" will parse as an identifier which is then validated to be an index name.

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

1. `IndexKind::RequiredNamed` and `IndexKind::RequiredRange` in the index registry indicate unresolved indices. Type-checking a file with unresolved required indices produces a clear error (unless they are being resolved via parameterized import).

2. Reject variant literals and map literal keys that reference a required index.

3. Allow required indices in type annotations, `for` bindings, and generic constraints.

4. For `RequiredNamed`: infer loop variables as `Label(IndexName)`.

5. For `RequiredRange { dimension }`: infer loop variables as `Scalar(dimension)`.

6. **Kind matching validation** during import processing:
    - `RequiredNamed` binding: verify the bound index is `IndexKind::Named`. Error if it's `Range`.
    - `RequiredRange { dimension }` binding: verify the bound index is `IndexKind::Range` and its dimension matches. Error if it's `Named` or has a different dimension.

### Phase 5: LSP, Docs, Tests

1. LSP: hover on a required index shows "required index (must be bound via import)".
2. Diagnostics: clear error messages for unbound required indices, invalid variant references.
3. Tree-sitter: update grammar, syntax highlighting.
4. Editor extensions: update TextMate grammar for `index Foo;` syntax.
5. Tests: add fixtures for required index imports, multi-index imports, strict mode interactions.
6. Documentation: update `docs/language/indexes.md`, `docs/tutorial/`, and `README.md`.

## Resolved Questions

- **Range index injection?** Named and range indices have fundamentally different loop variable types (`Label` vs `Scalar`). **No mixing.** A required named index can only be bound to a named index. A required range index can only be bound to a range index with a matching dimension. The required index declaration must commit to a kind: `index Foo;` (named) or `index Foo: range(Dim);` (range).

- **Index constraint syntax?** Required range indices use `index Foo: range(Dim);` to declare the kind and dimension constraint. Required named indices use `index Foo;` (bare, since no additional constraint is needed). The `range(Dim)` syntax mirrors the existing `range(start, end, step: s)` syntax for concrete range indices.

## Open Questions

- **Inline index definition in binding?** Should `(Subsystem = { A, B, C })` be allowed as a shorthand that creates an anonymous index? *Recommendation: no — require a named index for explicitness. Consistent with Graphcal's safety-over-usability philosophy.*

- **CLI binding for required indices?** Can `--set` or `--input` provide index definitions for a standalone file with required indices? *Recommendation: defer. Focus on parameterized import bindings first. CLI binding can be designed later as a natural extension.*

- **Can a required index appear in a const declaration?** E.g., `const weights: Dimensionless[I] = ...;`. Since const values must be fully evaluable at compile time and a required index has unknown variants, this seems problematic. *Recommendation: disallow required indices in const declarations for now.*

## Dependencies

- **07-indexes.md:** Extended with the concept of required indices.
- **Phase 4 (multi-file):** Parameterized imports are the binding mechanism.
- **20-type-system-stratification.md:** Required index is a new kind of type-level entity.
- **21-separate-label-indexes-from-tagged-unions.md:** The `Label(IndexName)` type must handle the case where `IndexName` refers to a required (unresolved) index during compilation of the library file.
