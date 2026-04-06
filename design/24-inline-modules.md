# Inline DAGs — Everything is a DAG

> Replace user-defined pure functions (`fn`) with inline `dag` definitions. A Graphcal file is a DAG; an inline `dag` block is a DAG. Built-in functions remain as the expression vocabulary. The only user-defined abstraction is the DAG.

## Status

**Decision level:** Draft. Core idea accepted for exploration; syntax details need refinement. Scoping rules and `import`/`include` semantics are defined in [doc 25](./25-const-modifier-and-import-include.md).

## Motivation

Graphcal currently has two mechanisms for reusable parameterized computation:

1. **Pure functions** (`fn`): Expression-level, single-output, support generics (`Dim`, `Index`, `Nat`). Cannot access the DAG (`@` prohibited).
2. **Parameterized imports**: File-level, multi-output, support injectable indexes and param bindings. Instantiate a file as a sub-DAG.

These solve the same fundamental problem — accepting inputs, computing derived values, producing outputs — but through different mechanisms with different syntax, scoping rules, type systems, and compiler paths.

**The key insight:** A Graphcal file is already a DAG of `param`, `node`, `const`, and `index` declarations. Parameterized imports already instantiate one DAG into another. Pure functions are just a limited, expression-level version of the same thing. If we support defining named DAGs inline (not just as separate files), user-defined functions become unnecessary, and the language collapses to a single concept: **the DAG.**

## The Spreadsheet Model

The design follows a two-layer model, analogous to how spreadsheets work:

### Layer 1: Built-in Functions (Expression Vocabulary)

Built-in functions are the **atoms of computation** — primitive operations that users compose in expressions, just like `+`, `*`, and other operators. They are not user-defined, do not participate in the DAG as separate nodes, and are evaluated inline within node expressions.

```gcl
node v: Velocity = sqrt(abs(@a - @b) + lerp(@x, @y, 0.5));
```

This is analogous to spreadsheet formulas: `=SQRT(ABS(A1 - B1) + LERP(C1, D1, 0.5))`. The user composes built-in functions; the spreadsheet (Graphcal) evaluates them.

Built-in functions can be dimension-generic (e.g., `abs` works for any dimension, `lerp` interpolates any dimension), support positional arguments, and compose freely in expressions. They are provided by the language, not defined by users.

### Layer 2: DAGs (User-Defined Reusable Computation)

When users want to **name and reuse a computation pattern**, they define a `dag` — an inline sub-DAG. DAGs are the only user-defined abstraction in the language.

```gcl
dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}

include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v };
```

### Why This Works

- **One mental model:** Users learn built-in functions (like learning spreadsheet formulas) and DAGs (like defining named sub-sheets). No "function definition" concept to learn.
- **Uniform scoping:** Always `@` for DAG references. No special "function scope" where `@` is forbidden.
- **Multi-output for free:** DAGs naturally export multiple values. No wrapper structs needed.
- **Reactive by default:** DAG outputs participate in the reactive computation DAG.
- **Generics are covered by built-ins:** The most common dimension-generic operations (`abs`, `lerp`, `clamp`, `min`, `max`, etc.) are built-in. User DAGs typically have concrete, domain-specific types.

## Design

### Built-in Function Library

The built-in function library is expanded to cover common generic operations that would otherwise require user-defined generic abstractions. These are the "spreadsheet functions" of Graphcal:

**Math (dimension-generic where applicable):**

| Function | Signature | Description |
| --- | --- | --- |
| `sqrt(x)` | `Dim -> Dim^(1/2)` | Square root |
| `abs(x)` | `D -> D` | Absolute value |
| `min(a, b)` | `(D, D) -> D` | Minimum of two values |
| `max(a, b)` | `(D, D) -> D` | Maximum of two values |
| `clamp(value, low, high)` | `(D, D, D) -> D` | Clamp value to range |
| `lerp(a, b, t)` | `(D, D, Dimensionless) -> D` | Linear interpolation |
| `floor(x)` | `D -> D` | Floor |
| `ceil(x)` | `D -> D` | Ceiling |
| `round(x)` | `D -> D` | Round to nearest |

**Trigonometry (dimensionless):**

| Function | Description |
| --- | --- |
| `sin(x)`, `cos(x)`, `tan(x)` | Trigonometric functions |
| `asin(x)`, `acos(x)`, `atan2(y, x)` | Inverse trigonometric |

**Exponential/Logarithmic (dimensionless):**

| Function | Description |
| --- | --- |
| `exp(x)`, `ln(x)`, `log2(x)`, `log10(x)` | Exponential and logarithmic |

**Aggregation (over indexes):**

| Function | Description |
| --- | --- |
| `sum(...)`, `mean(...)`, `count(...)` | Aggregation over index comprehensions |
| `min(...)`, `max(...)` | Min/max over index comprehensions |

**Linear algebra (nat-generic):**

| Function | Signature | Description |
| --- | --- | --- |
| `dot(a, b)` | `(D1[N], D2[N]) -> D1*D2` | Dot product |
| `transpose(a)` | `D[M, N] -> D[N, M]` | Matrix transpose |

This list is not exhaustive. The standard library can grow over time. The key principle is: **if a computation is generic and broadly useful, it should be a built-in function, not a user-defined DAG.**

### Inline DAG Declaration

A `dag` block defines a reusable sub-DAG within a file:

```gcl
dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
```

A `dag` block can contain the same declarations as a file — `param`, `node`, `const node`, `assert`, `type`, `dimension`, `unit`, `index` — because a file *is* a DAG. An inline `dag` block is simply a named DAG defined within another DAG.

### DAG = Module

A DAG is Graphcal's module. See [doc 25](./25-const-modifier-and-import-include.md) for the full design of the DAG-as-module model, including:

- The unified module path system (`/` navigates down, `..` navigates up)
- `import` for compile-time definitions, `include` for DAG embedding
- Scoping rules for inline DAGs
- Reference syntax (`@` for runtime, bare for compile-time)

### DAG Parameterization: Not Just Params

A key design principle: **all DAGs are parameterized the same way.** Whether a DAG is a file or an inline block, the caller can inject not just `param` values but also `index` definitions (see [23-injectable-index-import](./23-injectable-index-import.md)). A DAG can declare **required** declarations that the caller must provide:

| Declaration | With default (self-contained) | Required (caller must provide) |
| --- | --- | --- |
| `param` | `param x: Length = 5.0 m;` | `param x: Length;` |
| Named index | `index Phase = { A, B, C };` | `index Phase;` |
| Range index | `index T = linspace(0.0 s, 1.0 s, step: 0.1 s);` | `index T: Time;` |

This means **Index generics are already solved** without any special generic syntax. A DAG with a required index is inherently index-generic:

```gcl
dag total_cost {
    index I;                                    // required named index
    param cost: Dimensionless[I];
    node total: Dimensionless = sum(for i: I { @cost[i] });
}

index Subsystem = { ADCS, Propulsion, Comms };
param sub_cost: Dimensionless[Subsystem] = { ... };

// Caller provides both index and param bindings:
include total_cost(I: Subsystem, cost: @sub_cost) { total };
```

The binding syntax distinguishes the two kinds by naming convention (same as parameterized imports of file DAGs):

- **PascalCase: PascalCase** → index binding (`I: Subsystem`)
- **snake_case: expr** → param binding (`cost: @sub_cost`)

### Instantiation Syntax

DAGs are instantiated using `include` with **named arguments**:

```gcl
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v };
```

Named arguments are required (not positional). This aligns with Graphcal's explicitness philosophy — readers can understand what each argument means without looking up the DAG definition. It also visually distinguishes DAG instantiation (named args via `include`) from built-in function calls (positional args in expressions):

```gcl
// Built-in function: positional args, in expression
node v1: Velocity = sqrt(@gm / @r);

// DAG instantiation: named args, via include
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v };
```

DAG calls **cannot** appear in expression position. All instantiation goes through `include`. See [doc 25](./25-const-modifier-and-import-include.md) for the rationale.

### Multi-Output DAGs

DAGs naturally export multiple values. Use `include` with selective imports or namespace access:

```gcl
dag hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    node total_dv: Velocity = @dv1 + @dv2;
}

// Selective import:
include hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) { dv1, dv2, total_dv };

// Or namespace access:
include hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) as h;
node total: Velocity = @h::total_dv;
node ratio: Dimensionless = @h::dv1 / @h::dv2;
```

### Intermediate Values

Where functions use `let` bindings, DAGs use `node` declarations:

```gcl
// Current function:
fn hohmann_dv(gm: GravParam, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}

// As a DAG:
dag hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = ...;
    node total_dv: Velocity = @dv1 + @dv2;
}
```

Intermediate values (`v1`, `v2`) become `node` declarations with explicit type annotations. This is consistent with Graphcal's philosophy: every value has an explicit type, and every intermediate is a named node in the DAG.

### Everything is a DAG

A Graphcal file is a DAG. An inline `dag` block is a DAG. They share the same structure (declarations), the same parameterization mechanism (required params, required indexes), and the same instantiation mechanism (`include` with bindings). The only differences are syntactic:

| Aspect | File DAG | Inline DAG |
| --- | --- | --- |
| Declaration | Implicit (`.gcl` file) | `dag name { ... }` |
| Path in `import`/`include` | File path or module path | Bare identifier or module path |

An inline DAG is simply a DAG defined inside another DAG, rather than in its own file. Extracting an inline DAG into its own file (or inlining a file DAG) is a pure refactoring — the semantics are identical.

The terminology is now uniform:

- **DAG**: A directed acyclic graph of declarations (`param`, `node`, `const node`, `index`, etc.). Every `.gcl` file is a DAG. Every `dag` block is a DAG.
- **`import`**: Brings compile-time definitions into scope.
- **`include`**: Instantiates one DAG into another, with param and index bindings.
- **Built-in function**: An atom of computation used within node expressions. Not a DAG.

### What Gets Removed

| Current feature | Replacement |
| --- | --- |
| `fn` keyword | `dag` block |
| `let` bindings in function bodies | `node` declarations in DAG body |
| `@` prohibition in fn bodies | Not needed (DAGs use `@` by design) |
| User-defined function registry (`FunctionRegistry`) | DAG registry |
| Recursion detection on functions | Recursion detection on DAG instantiation |
| `<I: Index>` function generics | Required index declarations (`index I;`) in DAG body |
| `FnGenericParam` / `FnGenericConstraint` | Required declarations + built-in functions (Dim/Nat generics deferred, see Future Extensions) |
| Function evaluation (`eval_builtin_or_user_fn` for user fns) | DAG instantiation (sub-DAG creation + wiring) |
| User-defined `abs`, `lerp`, `clamp` | Promoted to built-in functions |

### What's Preserved

| Feature | Status |
| --- | --- |
| Built-in functions (`sqrt`, `sin`, `cos`, etc.) | Unchanged |
| Built-in aggregations (`sum`, `mean`, `count`) | Unchanged |
| Parameterized file imports | Unchanged (same mechanism for file and inline DAGs) |
| Injectable indexes | Unchanged (natural fit — required declarations in DAGs) |

## Examples

### Single-Output DAG (Before/After)

```gcl
// BEFORE (current):
fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);
node v: Velocity = orbital_velocity(GM_EARTH, R_EARTH + @alt);

// AFTER (proposed):
dag orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v };
```

### Multi-Output Computation (Before/After)

```gcl
// BEFORE: need a wrapper struct
type TransferResult { dv1: Velocity, dv2: Velocity, total_dv: Velocity }

fn hohmann_dv(gm: GravParam, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    let dv1 = sqrt(2.0 * gm * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * gm * r1 / (r2 * (r1 + r2)));
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}
node transfer: TransferResult = hohmann_dv(GM_EARTH, R_EARTH + @alt1, R_EARTH + @alt2);
node dv1: Velocity = @transfer.dv1;

// AFTER: multiple exports, no wrapper struct
dag hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    node total_dv: Velocity = @dv1 + @dv2;
}
include hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) { dv1, dv2, total_dv };
```

### Index-Parameterized DAG

```gcl
// DAG with required index — reusable over any label set
dag power_budget {
    index Component;                        // required: caller provides the label set
    param power_draw: Power[Component];
    node total_power: Power = sum(for c: Component { @power_draw[c] });
    node max_draw: Power = max(for c: Component { @power_draw[c] });
}

// Instantiate with different indexes:
index Avionics = { IMU, StarTracker, GPS };
index Propulsion = { Thruster, Valve, Tank };

include power_budget(Component: Avionics, power_draw: @avionics_power) as av_budget;
include power_budget(Component: Propulsion, power_draw: @prop_power) as prop_budget;

node total: Power = @av_budget::total_power + @prop_budget::total_power;
```

This is the same mechanism as injectable indexes in file DAGs ([doc 23](./23-injectable-index-import.md)), applied to inline DAGs. No special generic syntax needed — the `index Component;` declaration is the "type parameter."

### Generic Operations Use Built-in Functions

```gcl
// No user-defined generic needed — abs, lerp, clamp are all built-in:
node d: Length = abs(@a - @b);
node mid: Length = lerp(@x, @y, 0.5);
node safe: Force = clamp(@thrust, @min_thrust, @max_thrust);

// Compose freely:
node v: Velocity = sqrt(abs(@a) + lerp(@b, @c, 0.5));
```

### Reuse Across Files

An inline DAG in one file can be imported by another file, just like any other declaration:

```gcl
// rocket.gcl
dag tsiolkovsky {
    param dry_mass: Mass;
    param fuel_mass: Mass;
    param isp: Time;
    node v_exhaust: Velocity = @isp * G0;
    node delta_v: Velocity = @v_exhaust * ln((@dry_mass + @fuel_mass) / @dry_mass);
}
```

```gcl
// main.gcl
import rocket { tsiolkovsky };
include tsiolkovsky(dry_mass: @my_dry_mass, fuel_mass: @my_fuel, isp: @my_isp) { delta_v };
```

## Open Questions

### Semantics

1. **Instantiation multiplicity:** If the same DAG is `include`d twice with the same arguments, does the compiler create one or two sub-DAGs? (Proposal: always create separate instances. Deduplication is an optimization, not a semantic guarantee.)

2. **Assertions inside DAGs:** If a DAG contains `assert` declarations, are they evaluated at each instantiation? (Proposal: yes. Assertions are part of the DAG's contract.)

3. **Recursive DAG instantiation:** A DAG instantiating itself (directly or mutually) must be detected and rejected, same as current function recursion detection.

### Built-in Library Boundary

4. **Where is the line between built-in and user-defined?** The built-in library should cover generic, broadly-useful operations. Domain-specific calculations (rocket equation, orbital mechanics, thermal analysis) are user DAGs. But what about operations like `normalize`, `cross`, `weighted_sum`? The boundary needs to be drawn carefully to avoid an ever-growing built-in library while ensuring users don't need DAG generics for common math.

5. **Can users request new built-ins?** Should there be a mechanism for users to propose additions to the built-in function library, or is this a language-level decision only?

### Migration

6. **Clean break:** Since Graphcal is unpublished, this can be a clean break — remove `fn`, add `dag`, expand the built-in library. No coexistence period needed.

## Generics Status

The "required declarations" pattern (same mechanism as parameterized file imports) covers generics as follows:

| Generic kind | Current `fn` syntax | Inline DAG equivalent | Status |
| --- | --- | --- | --- |
| Index | `<I: Index>` | `index I;` (required index) | **Solved** — same mechanism as injectable indexes |
| Dim | `<D: Dim>` | `dimension D;` (required dimension)? | **Open** — see below |
| Nat | `<N: Nat>` | No obvious required-declaration equivalent | **Deferred** — covered by built-in functions for common cases |

### Index Generics: Solved

Required indexes (`index I;`, `index T: Time;`) work identically to `<I: Index>` function generics. The compiler already has index substitution machinery from injectable indexes. No new design needed.

### Dimension Generics: Open Question

Could a "required dimension" declaration serve as `<D: Dim>` generics?

```gcl
dag weighted_sum {
    dimension D;                    // required: caller provides the dimension
    index I;                        // required: caller provides the index
    param values: D[I];
    param weights: Dimensionless[I];
    node result: D = sum(for i: I { @values[i] * @weights[i] });
}

// Caller provides dimension and index:
index Region = { NA, EU, APAC };
include weighted_sum(D: Money, I: Region, values: @revenue, weights: @market_share) { result as total_revenue };
```

This follows the same "required declaration" pattern as indexes. The dimension is declared without a definition inside the DAG, and the caller binds it at instantiation.

**Arguments for:**

- Consistent with the required-index pattern — no new concept, just extending to another declaration kind
- The compiler already has dimension substitution in type expressions (analogous to `substitute_type_expr_index_names`)
- Avoids a separate generic syntax (`<D: Dim>`) that only applies to DAGs

**Arguments against:**

- Dimensions are structural (products of base dimensions), not named entities like indexes. `D: Money` at the call site is a binding, but unlike index binding (which replaces a name), dimension binding replaces a structural type — the substitution semantics are different.
- Inference is important: `lerp(@x, @y, 0.5)` should infer `D = Length` from `@x: Length` without the caller writing `D: Length`. Can required dimensions support inference? (Possibly: if all required-dimension params are bound with expressions, the compiler can infer the dimension from the expression types.)
- Most dimension-generic operations are covered by built-in functions, so the need may be limited.

**Decision:** Deferred. Start with concrete-typed DAGs + built-in functions. If real-world usage demonstrates the need for user-defined dimension-generic DAGs, the required-dimension pattern is a natural extension.

### Nat Generics: Deferred

Nat generics (`<N: Nat>`) are used for size-polymorphic operations like `transpose`, `dot`, `drop_last`. These are covered by built-in functions. If user-defined nat-generic DAGs are needed later, the mechanism could be a "required nat" declaration, though the syntax is less obvious than for indexes or dimensions.

## Future Extensions

### Required Dimensions in File DAGs

File DAGs already support required indexes. Extending to required dimensions would be a natural evolution:

```gcl
// lib.gcl — a file DAG with required index and dimension
dimension D;
index I;
param values: D[I];
param weights: Dimensionless[I];
node result: D = sum(for i: I { @values[i] * @weights[i] });
```

This is already how required indexes work in file DAGs today. Extending to required dimensions would make file DAGs and inline DAGs fully equivalent in their parameterization capabilities.

## Dependencies on Other Aspects

- **[25 — `const` Modifier, `import`/`include`](./25-const-modifier-and-import-include.md)**: Defines the `import`/`include` split, scoping rules, module path system, and reference syntax that this design depends on.
- **Computation Model** ([01](./01-computation-model.md)): Inline DAGs are sub-DAGs within the computation DAG.
- **Scoping** ([08](./08-scoping.md)): `@` resolution within DAG bodies.
- **Namespace & Multi-File** ([09](./09-namespace.md)): DAG imports integrate with the existing import system.
- **Indexes** ([07](./07-indexes.md)): Injectable indexes as a precedent for DAG parameterization.
- **Injectable Index Import** ([23](./23-injectable-index-import.md)): DAG-level index parameterization.
- **Pure Functions** ([12](./12-pure-functions.md)): This design supersedes doc 12.
- **Type System Stratification** ([20](./20-type-system-stratification.md)): DAGs are declarations, not types.
