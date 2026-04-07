# Injectable Index in Parameterized Import

> Allow indexes to be passed as bindings in parameterized imports, making Graphcal files usable as reusable, index-generic libraries.

## Status

**Decision level:** Draft.

## Motivation

Parameterized imports currently allow injecting `param` values into a dependency, enabling reusable computation templates (e.g., the rocket equation with different masses). However, **indexes** — the finite label sets used as table axes — are always hardcoded in the imported file. This prevents a file from being truly reusable when it needs to operate over a caller-defined set of labels.

Consider a power budget library:

```gcl
// power_budget.gcl — NOT reusable: Subsystem is hardcoded
index Subsystem = { ADCS, Propulsion, Comms, Payload };

param power_draw: Power[Subsystem];
node total_power: Power = sum(for s: Subsystem { @power_draw[s] });
```

A different mission may have different subsystems. Without injectable indexes, the library cannot be parameterized over the label set. The user must copy and modify the file, defeating the purpose of multi-file organization.

**Injectable indexes** solve this by allowing the caller to supply the index definition, just as they supply param values.

## Design

### Principle: Indexes Follow the Same Required/Default Pattern as Params

| Concept | Param | Named Index (`index`) | Range Index |
| --- | --- | --- | --- |
| Has default | `param x: Length = 5.0 m;` | `index Phase = { Design, Build, Test };` | `range TimeStep(0.0 s, 1.0 s, step: 0.1 s)` |
| Required (no default) | `param x: Length;` | `index Subsystem;` | `index TimeStep: Time;` |
| Bound via import | `(x = 5.0 m)` | `(Subsystem = MySubsystems)` | `(TimeStep = MyTimeStep)` |

### Required Index Declaration

A required index declaration has no variants but must declare its **kind** — named or range — because the two kinds have fundamentally different semantics:

| Kind | Loop variable type | Semantics |
| --- | --- | --- |
| Named (`index`) | `Label(IndexName)` | Categorical labels — identity, not arithmetic |
| Range | `Scalar(Dimension)` | Numeric steps — supports arithmetic, has physical dimension |

Code written for a named index (comparing labels, matching on variants) cannot work with a range index (doing arithmetic on scalar values), and vice versa. Therefore, a required index must commit to a kind so the compiler can type-check the library in isolation.

**Named required index:**

```gcl
index Subsystem;  // required named index — variants provided by importer
```

**Range required index:**

```gcl
index TimeStep: Time;  // required range index over the Time dimension
```

The dimension constraint (e.g., `Time`) is mandatory — range index loop variables are `Scalar(Dimension)`, so the compiler needs the dimension to type-check expressions like `@velocity[t] * dt` inside the library.

Both forms are analogous to `param x: Length;` (required param with no default value). A file containing required indexes cannot be evaluated standalone, just like a file with required params.

### Binding Syntax: Reuse the Same Parentheses

Index bindings and param bindings coexist in the same `(...)` list of a parameterized import:

```gcl
index MySubsystems = { ADCS, Propulsion, Comms };

import "./power_budget.gcl"(
    Subsystem = MySubsystems,
    power_draw = @my_power,
) { total_power };
```

The two kinds of bindings are visually distinguishable by Graphcal's existing naming conventions:

- **PascalCase = PascalCase** → index binding (`Subsystem = MySubsystems`)
- **snake_case = expr** → param binding (`power_draw = @my_power`)

No new syntax beyond what parameterized imports already support. The parser distinguishes the two by checking whether the LHS name refers to a `index` declaration or a `param` declaration in the dependency.

### Semantics: Index Substitution

During instantiation, an index binding performs a **name substitution**: every reference to the bound index name in the dependency is replaced with the provided index name.

Given `Subsystem = MySubsystems`:

| Location | Before | After |
| --- | --- | --- |
| Type annotation | `Power[Subsystem]` | `Power[MySubsystems]` |
| `for` binding | `for s: Subsystem` | `for s: MySubsystems` |
| Param default (map literal key) | `Subsystem::ADCS` | `MySubsystems::ADCS` (only valid if variant exists) |

The dependency's index registry entry for the bound index is **not** registered in the importer's scope — the importer already has the target index. Non-bound indexes from the dependency are registered as usual.

### Variant Literal Restriction

To ensure every `.gcl` file is reusable as a library, **variant literals are banned in non-rebindable contexts**. This rule applies to all `index` indexes — both required and those with defaults.

#### The Problem

If a `node` expression uses a variant literal like `Phase::Design`, and the importer overrides `Phase` with a different set of variants, the node is broken — and unlike `param` defaults, **nodes cannot be rebound by the importer**.

```gcl
index Phase = { Design, Build, Test };

// This node is NOT reusable — hardcodes Phase::Design
node design_cost: Money = @cost[Phase::Design];  // BANNED
```

#### The Rule

| Context | Variant literal allowed? | Reason |
| --- | --- | --- |
| `param` default expression | **Yes** | Importer can rebind the param |
| `node` expression | **No** | Not rebindable by importer |
| `const node` expression | **No** | Not rebindable |
| `assert` expression | **No** | Not rebindable |
| `fn` body | **No** | Internal logic, not rebindable |
| `#[expected_fail(...)]` attribute | **Yes** | Handled via import-site override (see below) |

This includes all forms of variant usage:

- **Index access with variant literal**: `@cost[Phase::Design]` in a node
- **Variant comparison**: `if m == Phase::Design { ... }` in a node
- **Match on variants**: `match m { Phase::Design => ..., ... }` in a node
- **Map/table literal with variant keys**: `{ Phase::Design: 1.0 }` in a node

#### The Correct Pattern

Variant-specific behavior must be expressed through **params** (which the importer can rebind):

```gcl
index Phase = { Design, Build, Test };
param cost: Money[Phase] = table[Phase] {
    Design: 1000.0 USD;
    Build: 5000.0 USD;
    Test: 2000.0 USD;
};

// BEFORE (banned — variant literal in node):
node design_cost: Money = @cost[Phase::Design];

// AFTER (correct — extract to param):
param design_cost: Money = @cost[Phase::Design];  // rebindable by importer
node double_design: Money = @design_cost * 2.0;   // no variant literal
```

```gcl
// BEFORE (banned — variant comparison in node):
node weighted: Velocity[Maneuver] = for m: Maneuver {
    if m == Maneuver::Departure { @dv[m] * 2.0 } else { @dv[m] }
};

// AFTER (correct — express variant-specific behavior via param):
param weight: Dimensionless[Maneuver] = {
    Maneuver::Departure: 2.0,
    Maneuver::Correction: 1.0,
    Maneuver::Insertion: 1.0,
};
node weighted: Velocity[Maneuver] = for m: Maneuver { @dv[m] * @weight[m] };
```

This constraint is a feature: it forces library code to be truly generic over the index. The compiler enforces this universally so that **any** `.gcl` file can be imported with a different index without breaking.

### What a Library Can/Cannot Do with an Index

Since variant literals are banned in non-rebindable contexts for **all** indexes, the allowed operations are the same for required and default indexes in `node`/`const node`/`assert`/`fn` contexts:

| Operation | Named (`index`) | Range | Example |
| --- | --- | --- | --- |
| Type annotation (axis) | Yes | Yes | `param x: Velocity[Subsystem]` |
| `for` comprehension | Yes | Yes | `for s: Subsystem { @x[s] }` |
| Index access with loop var | Yes | Yes | `@x[s]` where `s: Subsystem` |
| Aggregation | Yes | Yes | `sum(for s: Subsystem { @x[s] })` |
| Arithmetic on loop var | **No** | Yes | `t * 2.0` where `t: TimeStep` (scalar) |
| `unfold` | **No** | Yes | `unfold(@x0, \|prev, curr\| { ... })` |
| Variant literal (in node/const node/assert/fn) | **No** | N/A | `Subsystem::ADCS` — banned |
| Map literal (in node/const node/assert/fn) | **No** | N/A | `{ Subsystem::ADCS: ... }` — banned |
| Variant literal (in param default) | Yes | N/A | `{ Subsystem::ADCS: 1.0 }` — allowed, rebindable |
| Map literal (in param default) | Yes | N/A | `table[Subsystem] { ... }` — allowed, rebindable |

### Kind Matching: No Mixing Named and Range

When binding an index, the bound index must match the declared kind:

| Declaration | Can bind to | Cannot bind to |
| --- | --- | --- |
| `index Subsystem;` (required named) | Named index | Range index |
| `index Phase = { A, B };` (default named) | Named index | Range index |
| `index TimeStep: Time;` (required range) | Range index with dimension `Time` | Named index, range with wrong dimension |
| `range T(0.0 s, 1.0 s, step: 0.1 s)` (default range) | Range index with same dimension | Named index, range with wrong dimension |

**Named → Named:** The bound index must be a named index (one with explicit label variants).

```gcl
// OK: Color is a named index
index Color = { Red, Green, Blue };
import "./lib.gcl"(I = Color) { ... };

// ERROR: TimeStep is a range index, but I is a required named index
index TimeStep = linspace(0.0 s, 1.0 s, step: 0.1 s);
import "./lib.gcl"(I = TimeStep) { ... };
//  error: index kind mismatch: `I` requires a named index, but `TimeStep` is a range index
```

**Range → Range with matching dimension:** The bound index must be a range index whose dimension matches the declared constraint.

```gcl
// OK: MyTimeStep is a range index over Time
index MyTimeStep = linspace(0.0 s, 10.0 s, step: 0.5 s);
import "./sim.gcl"(TimeStep = MyTimeStep) { ... };

// ERROR: DistStep is a range index over Length, not Time
index DistStep = linspace(0.0 m, 100.0 m, step: 1.0 m);
import "./sim.gcl"(TimeStep = DistStep) { ... };
//  error: dimension mismatch: `TimeStep` requires range(Time), but `DistStep` has dimension Length
```

This rule is essential for type safety. A library that does `@velocity[t] * (t_next - t_prev)` relies on the loop variable having a specific physical dimension. Allowing a length-dimensioned range index would produce dimensionally incorrect results — exactly the kind of error Graphcal is designed to prevent.

### Strict Binding Mode: Extends to Indexes

The existing strict binding rule ("when any binding is provided, ALL params with defaults must be explicitly bound unless `#[allow_defaults]`") naturally extends to indexes:

- **Required indexes** (`cat X;`, `range X: Dim;`) must always be bound when the import has any bindings.
- **Indexes with defaults** (`cat X { A, B }`, `range X(...)`) must also be explicitly bound under strict mode, unless `#[allow_defaults]`.

```gcl
// ERROR: Phase has defaults but isn't bound (strict mode)
import "./lib.gcl"(cost = @my_cost) { total };

// OK: all bindings provided
import "./lib.gcl"(Phase = MyPhase, cost = @my_cost) { total };

// OK: #[allow_defaults] lets Phase use its declared variants
#[allow_defaults]
import "./lib.gcl"(cost = @my_cost) { total };
```

### Overriding Default Indexes

An index with declared variants can be overridden by an import binding:

```gcl
// lib.gcl
index Phase = { Design, Build, Test };
param cost: Money[Phase] = {
    Phase::Design: 1000.0 USD,
    Phase::Build: 5000.0 USD,
    Phase::Test: 2000.0 USD,
};
```

```gcl
index MyPhases = { Concept, Dev, Validation, Production };

import "./lib.gcl"(Phase = MyPhases, cost = @my_cost) { ... };
```

When `Phase` is overridden, the default value of `cost` (which references `Phase::Design` etc.) becomes invalid. This is safe because strict mode requires `cost` to also be explicitly bound. If `#[allow_defaults]` is used without binding `Phase`, the original variants and dependent defaults remain valid.

### `#[expected_fail]` Handling for Overridden Indexes

#### Library-side: variant literals in `#[expected_fail]` are allowed

The `#[expected_fail(Mode::Boost)]` attribute is testing metadata, not computation logic. It is valid in the library's own context where `Mode::Boost` exists, so it is **exempt** from the variant literal ban.

#### Behavior when the index is overridden

When importing with an index binding that overrides the index:

1. **Library's variant-specific `expected_fail` annotations are dropped.** If the library has `#[expected_fail(Mode::Boost)]` and `Mode` is overridden with `MyModes`, the `Mode::Boost` annotation no longer applies because `Boost` doesn't exist in `MyModes`.
2. **Library's non-variant `expected_fail` annotations carry over.** `#[expected_fail]` without variant arguments (meaning "the whole assertion is expected to fail") is preserved.

#### Import items can have attributes

The importing file can attach `#[expected_fail]` to individual import items to declare expected failures for the new variants:

```gcl
// lib.gcl
index Mode = { Normal, Boost, Emergency };
param limit: Power[Mode];
param actual: Power[Mode];

#[expected_fail(Mode::Boost)]
assert within_limit = for m: Mode { @actual[m] <= @limit[m] };
```

```gcl
// main.gcl
index MyModes = { Idle, Active, Overdrive };

import "./lib.gcl"(
    Mode = MyModes,
    limit = @my_limits,
    actual = @my_actual,
) {
    #[expected_fail(MyModes::Overdrive)]
    within_limit,
};
```

Here:

- The library's `#[expected_fail(Mode::Boost)]` is dropped (Mode is overridden; `Boost` doesn't exist in `MyModes`).
- The importer's `#[expected_fail(MyModes::Overdrive)]` takes effect.
- The assertion `within_limit` is evaluated over `MyModes` variants, with `Overdrive` expected to fail.

The import item syntax is extended to support attributes:

```
ImportItem = Attribute* IDENT ("as" IDENT)?
```

Only `#[expected_fail(...)]` is valid on import items initially; other attributes could be added later.

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
index Color = { Red, Green, Blue };

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
index Payload = { Camera, Antenna, Computer, Battery };
index Mode = { Idle, Science, Downlink };

// ... param definitions for payload_power and payload_conductance ...

import "./thermal_analysis.gcl"(
    Component = Payload,
    OpMode = Mode,
    power_dissipation = @payload_power,
    thermal_conductance = @payload_conductance,
) { temperature_rise, max_temp_rise };

// @temperature_rise has type Temperature[Payload, Mode]
```

### Multiple Instantiations with Different Indexes

```gcl
index Avionics = { IMU, StarTracker, GPS };
index Propulsion = { Thruster, Valve, Tank };

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
index TimeStep: Time;  // required range index over Time

param x0: Length;
param velocity: Velocity[TimeStep];

node position: Length[TimeStep] = unfold(@x0, |prev_t, t| {
    let dt = t - prev_t;
    @position[prev_t] + @velocity[t] * dt
});
```

```gcl
// mission.gcl
index FlightTime = linspace(0.0 s, 100.0 s, step: 1.0 s);

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
index Phases = { Launch, Coast, Entry };
import "./integrator.gcl"(TimeStep = Phases, ...) { position };
//  error: index kind mismatch: `TimeStep` requires a range index (dimension: Time),
//         but `Phases` is a named index
```

### Mixed Required and Default Indexes

```gcl
// budget.gcl
index Phase = { A, B, C, D };  // default set (mission phases are usually standard)
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

### `expected_fail` with Overridden Index

```gcl
// lib.gcl — library with an assertion that has expected failures
index Mode = { Normal, Boost, Emergency };
param limit: Power[Mode];
param actual: Power[Mode];

#[expected_fail(Mode::Boost)]
assert within_limit = for m: Mode { @actual[m] <= @limit[m] };
```

```gcl
// main.gcl — overrides Mode, provides new expected failures
index MyModes = { Idle, Active, Overdrive };

import "./lib.gcl"(
    Mode = MyModes,
    limit = @my_limits,
    actual = @my_actual,
) {
    #[expected_fail(MyModes::Overdrive)]
    within_limit,
};

// The library's #[expected_fail(Mode::Boost)] is dropped.
// The importer's #[expected_fail(MyModes::Overdrive)] takes effect.
```

## Implementation Plan

### Phase 0: Variant Literal Ban (prerequisite)

Enforce the variant literal ban before implementing injectable indexes. This is a breaking change but the software is unpublished.

1. **Compiler check**: During IR lowering or TIR type-checking, reject variant literals in `node`, `const node`, `assert`, and `fn` contexts. Variant literals in `param` defaults and `#[expected_fail]` attributes remain allowed.
2. **Update test fixtures**: Refactor all affected test files to use the param-extraction pattern.
3. **Error message**: "variant literal `Phase::Design` cannot be used in a node expression; extract it into a `param` default instead"
4. **LSP diagnostic**: Same error, shown inline.

### Phase 1: AST and Parser

1. Add required variants to `IndexDeclKind`:

    ```rust
    pub enum IndexDeclKind {
        Named { variants: Vec<Spanned<VariantName>> },
        Range { start: Box<Expr>, end: Box<Expr>, step: Box<Expr> },
        RequiredNamed,                             // NEW: `index Foo;`
        RequiredRange { dimension: DimExpr },       // NEW: `index Foo: Time;`
    }
    ```

2. Parse `index Foo;` as `IndexDeclKind::RequiredNamed`.

3. Parse `index Foo: DimExpr;` as `IndexDeclKind::RequiredRange`.

4. Extend import item syntax to support attributes: `Attribute* IDENT ("as" IDENT)?`.

5. In the tree-sitter grammar, add the `cat IDENT ;` and `range IDENT : DimExpr ;` productions, and the attributed import item production.

6. In the import param binding parser, no changes needed — bindings are already `name = expr`. The distinction between index binding and param binding is made during semantic analysis, not parsing. For the RHS of an index binding, the "expression" will parse as an identifier which is then validated to be an index name.

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

4. Validate kind matching: named → named, range → range with matching dimension.

5. Extend the strict binding check to include indexes with default variants.

6. Process `#[expected_fail]` on import items: when the index is overridden, drop the library's variant-specific annotations for that index and apply the importer's annotations.

### Phase 3: IR Merge — Index Substitution

1. In `merge_dependency`, add an `index_bindings` parameter.

2. Before prefixing expressions, perform index name substitution:
    - In type annotations (`TypeExpr`): replace bound index names.
    - In `for`-comprehension bindings: replace the index name.
    - In param default map literal keys: replace bound index names (only relevant for default overrides with `#[allow_defaults]`).

3. When merging the registry, skip registering indexes that are bound (the importer already has the target index).

### Phase 4: Type Checking

1. `IndexKind::RequiredNamed` and `IndexKind::RequiredRange` in the index registry indicate unresolved indexes. Type-checking a file with unresolved required indexes produces a clear error (unless they are being resolved via parameterized import).

2. Allow required indexes in type annotations, `for` bindings, and generic constraints.

3. For `RequiredNamed`: infer loop variables as `Label(IndexName)`.

4. For `RequiredRange { dimension }`: infer loop variables as `Scalar(dimension)`.

5. **Kind matching validation** during import processing:
    - `RequiredNamed` binding: verify the bound index is `IndexKind::Named`. Error if it's `Range`.
    - `RequiredRange { dimension }` binding: verify the bound index is `IndexKind::Range` and its dimension matches. Error if it's `Named` or has a different dimension.

### Phase 5: LSP, Docs, Tests

1. LSP: hover on a required index shows "required named index (must be bound via import)" or "required range index (dimension: Time)".
2. Diagnostics: clear error messages for unbound required indexes, kind mismatches, dimension mismatches, and the variant literal ban.
3. Tree-sitter: update grammar, syntax highlighting for `index Foo;`, `index Foo: Time;`, and attributed import items.
4. Editor extensions: update TextMate grammars for Zed and VS Code.
5. Tests: add fixtures for required index imports, multi-index imports, strict mode interactions, `expected_fail` on import items, and variant literal ban enforcement.
6. Documentation: update `docs/language/indexes.md`, `docs/language/multi-file.md`, and `README.md`.

## Resolved Questions

- **Range index injection?** Named and range indexes have fundamentally different loop variable types (`Label` vs `Scalar`). **No mixing.** A required named index can only be bound to a named index. A required range index can only be bound to a range index with a matching dimension. The required index declaration must commit to a kind: `index Foo;` (named) or `index Foo: Dim;` (range).

- **Required range syntax?** `index Foo: Dim;` — the dimension after the colon declares the constraint. This parallels `param x: Length;` (name, colon, type constraint). Examples: `index TimeStep: Time;`, `index Altitude: Length;`.

- **Variant literals in non-rebindable contexts?** Banned for all indexes (required and default) in `node`, `const node`, `assert`, and `fn` contexts. Allowed in `param` defaults (rebindable) and `#[expected_fail]` attributes (handled via import-site override). This ensures every file is reusable as a library.

- **`#[expected_fail]` with overridden indexes?** Library's variant-specific annotations are dropped when the index is overridden. The importing file can attach `#[expected_fail]` to import items to declare expected failures for the new variants. Import item syntax extended to `Attribute* IDENT ("as" IDENT)?`.

## Open Questions

- **Inline index definition in binding?** Should `(Subsystem = { A, B, C })` be allowed as a shorthand that creates an anonymous index? *Recommendation: no — require a named index for explicitness. Consistent with Graphcal's safety-over-usability philosophy.*

- **CLI binding for required indexes?** Can `--set` or `--input` provide index definitions for a standalone file with required indexes? *Recommendation: defer. Focus on parameterized import bindings first. CLI binding can be designed later as a natural extension.*

- **Can a required index appear in a const node declaration?** E.g., `const node weights: Dimensionless[I] = ...;`. Since const node values must be fully evaluable at compile time and a required index has unknown variants, this seems problematic. *Recommendation: disallow required indexes in const node declarations for now.*

## Dependencies

- **07-indexes.md:** Extended with the concept of required indexes and the variant literal restriction.
- **Phase 4 (multi-file):** Parameterized imports are the binding mechanism.
- **20-type-system-stratification.md:** Required index is a new kind of type-level entity.
- **21-separate-label-indexes-from-tagged-unions.md:** The `Label(IndexName)` type must handle the case where `IndexName` refers to a required (unresolved) index during compilation of the library file.
