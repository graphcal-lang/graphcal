# Inline Graphs — Everything is a Graph

> Replace user-defined pure functions (`fn`) with inline `graph` definitions. A Graphcal file is a graph; an inline `graph` block is a graph. Built-in functions remain as the expression vocabulary. The only user-defined abstraction is the graph.

## Status

**Decision level:** Draft. Core idea accepted for exploration; syntax and scoping details need refinement.

## Motivation

Graphcal currently has two mechanisms for reusable parameterized computation:

1. **Pure functions** (`fn`): Expression-level, single-output, support generics (`Dim`, `Index`, `Nat`). Cannot access the graph (`@` prohibited).
2. **Parameterized imports**: File-level, multi-output, support injectable indexes and param bindings. Instantiate a file as a sub-DAG.

These solve the same fundamental problem — accepting inputs, computing derived values, producing outputs — but through different mechanisms with different syntax, scoping rules, type systems, and compiler paths.

**The key insight:** A Graphcal file is already a graph — a DAG of `param`, `node`, `const`, and `index` declarations. Parameterized imports already instantiate one graph into another. Pure functions are just a limited, expression-level version of the same thing. If we support defining named graphs inline (not just as separate files), user-defined functions become unnecessary, and the language collapses to a single concept: **the graph.**

## The Spreadsheet Model

The design follows a two-layer model, analogous to how spreadsheets work:

### Layer 1: Built-in Functions (Expression Vocabulary)

Built-in functions are the **atoms of computation** — primitive operations that users compose in expressions, just like `+`, `*`, and other operators. They are not user-defined, do not participate in the DAG as separate nodes, and are evaluated inline within node expressions.

```gcl
node v: Velocity = sqrt(abs(@a - @b) + lerp(@x, @y, 0.5));
```

This is analogous to spreadsheet formulas: `=SQRT(ABS(A1 - B1) + LERP(C1, D1, 0.5))`. The user composes built-in functions; the spreadsheet (Graphcal) evaluates them.

Built-in functions can be dimension-generic (e.g., `abs` works for any dimension, `lerp` interpolates any dimension), support positional arguments, and compose freely in expressions. They are provided by the language, not defined by users.

### Layer 2: Graphs (User-Defined Reusable Computation)

When users want to **name and reuse a computation pattern**, they define a `graph` — an inline sub-DAG. Graphs are the only user-defined abstraction in the language.

```gcl
graph orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node result: Velocity = sqrt(@gm / @r);
}

node v: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt);
```

### Why This Works

- **One mental model:** Users learn built-in functions (like learning spreadsheet formulas) and graphs (like defining named sub-sheets). No "function definition" concept to learn.
- **Uniform scoping:** Always `@` for graph references. No special "function scope" where `@` is forbidden.
- **Multi-output for free:** Graphs naturally export multiple values. No wrapper structs needed.
- **Reactive by default:** Graph outputs participate in the reactive computation graph.
- **Generics are covered by built-ins:** The most common dimension-generic operations (`abs`, `lerp`, `clamp`, `min`, `max`, etc.) are built-in. User graphs typically have concrete, domain-specific types.

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

This list is not exhaustive. The standard library can grow over time. The key principle is: **if a computation is generic and broadly useful, it should be a built-in function, not a user-defined graph.**

### Inline Graph Declaration

A `graph` block defines a reusable sub-DAG within a file:

```gcl
graph orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node result: Velocity = sqrt(@gm / @r);
}
```

A `graph` block can contain the same declarations as a file — `param`, `node`, `const`, `assert`, `type`, `dimension`, `unit`, `index` — because a file *is* a graph. An inline `graph` block is simply a named graph defined within another graph.

### Graph Parameterization: Not Just Params

A key design principle: **all graphs are parameterized the same way.** Whether a graph is a file or an inline block, the caller can inject not just `param` values but also `index` definitions (see [23-injectable-index-import](./23-injectable-index-import.md)). A graph can declare **required** declarations that the caller must provide:

| Declaration | With default (self-contained) | Required (caller must provide) |
| --- | --- | --- |
| `param` | `param x: Length = 5.0 m;` | `param x: Length;` |
| Named index | `index Phase = { A, B, C };` | `index Phase;` |
| Range index | `index T = linspace(0.0 s, 1.0 s, step: 0.1 s);` | `index T: Time;` |

This means **Index generics are already solved** without any special generic syntax. A graph with a required index is inherently index-generic:

```gcl
graph total_cost {
    index I;                                    // required named index
    param cost: Dimensionless[I];
    node total: Dimensionless = sum(for i: I { @cost[i] });
}

index Subsystem = { ADCS, Propulsion, Comms };
param sub_cost: Dimensionless[Subsystem] = { ... };

// Caller provides both index and param bindings:
node t: Dimensionless = total_cost(I: Subsystem, cost: @sub_cost);
```

The binding syntax distinguishes the two kinds by naming convention (same as parameterized imports of file graphs):

- **PascalCase: PascalCase** → index binding (`I: Subsystem`)
- **snake_case: expr** → param binding (`cost: @sub_cost`)

### Call-Site Syntax

Graphs are instantiated using function-call syntax with **named arguments**:

```gcl
node v: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt);
```

Named arguments are required (not positional). This aligns with Graphcal's explicitness philosophy — readers can understand what each argument means without looking up the graph definition. It also visually distinguishes graph calls (named args) from built-in function calls (positional args):

```gcl
// Built-in function: positional args
node v1: Velocity = sqrt(@gm / @r);

// Graph instantiation: named args
node v2: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt);
```

When a graph call appears in expression position, the compiler extracts the `result` node as the value. This is a convention: single-output graphs must have a node named `result`.

### Multi-Output Graphs

When a graph exports multiple values, use `import` syntax with selective imports:

```gcl
graph hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    private node v1: Velocity = sqrt(@gm / @r1);
    private node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    node total_dv: Velocity = @dv1 + @dv2;
}

import hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) { dv1, dv2, total_dv };
```

The same `import` syntax used for file graphs works for inline graphs. The only difference is that the path is a bare identifier (the graph name) rather than a string literal (a file path).

### Namespace Access

Graphs (file or inline) can be imported as a namespace:

```gcl
import hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) as h;

node total: Velocity = @h::total_dv;
node ratio: Dimensionless = @h::dv1 / @h::dv2;
```

### Expression-Position Desugaring

When a graph call appears in expression position, the compiler desugars it:

```gcl
// User writes:
node v: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) * 2.0;

// Compiler sees (conceptually):
import orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { result as __ov_1 };
node v: Velocity = @__ov_1 * 2.0;
```

Graph calls and built-in functions compose naturally in the same expression:

```gcl
node v: Velocity = sqrt(
    orbital_velocity(gm: GM_EARTH, r: @r1) *
    orbital_velocity(gm: GM_EARTH, r: @r2)
);
```

### Nested Graph Calls

Graph calls can be nested in arguments to other graph calls or built-in functions:

```gcl
graph velocity_at_alt {
    param alt: Length;
    node result: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}

graph delta_v {
    param v1: Velocity;
    param v2: Velocity;
    node result: Velocity = abs(@v2 - @v1);
}

// Nested: graph call as argument to another graph call
node dv: Velocity = delta_v(
    v1: velocity_at_alt(alt: @parking_alt),
    v2: velocity_at_alt(alt: @target_alt),
);
```

Each nested call is lifted into a separate instantiation by the compiler.

### Intermediate Values

Where functions use `let` bindings, graphs use `node` declarations:

```gcl
// Current function:
fn hohmann_dv(gm: GravParam, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}

// As a graph:
graph hohmann {
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

### Visibility Within Graphs

All declarations within a graph are public by default (importable by the instantiator). Use `private` to hide internal helpers:

```gcl
graph hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    private node v1: Velocity = sqrt(@gm / @r1);
    private node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = ...;
    node dv2: Velocity = ...;
    node total_dv: Velocity = @dv1 + @dv2;
}
```

### Scoping Rules

**Within a graph body:** The `@` sigil references declarations within the graph's own scope (its params, nodes, consts). A graph is a self-contained DAG.

**Type vocabulary from enclosing scope:** Graphs can see `const`, `dimension`, `unit`, `type`, and `index` declarations from the enclosing file. These are immutable, stateless definitions that form the type vocabulary — not graph state.

```gcl
const R_EARTH: Length = 6371.0 km;
const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

graph circular_velocity {
    param alt: Length;
    node result: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}
```

Note: `const` references use bare names (no `@`), consistent with how constants are referenced everywhere else in the language. Only `param` and `node` references within the graph use `@`.

**Params and nodes from enclosing scope:** Not accessible. A graph's `@` references resolve only within the graph. This ensures that the graph's dependency structure is explicit in its parameter list.

### Everything is a Graph

A Graphcal file is a graph. An inline `graph` block is a graph. They share the same structure (declarations), the same parameterization mechanism (required params, required indexes), and the same instantiation mechanism (`import` with bindings). The only differences are syntactic:

| Aspect | File graph | Inline graph |
| --- | --- | --- |
| Declaration | Implicit (`.gcl` file) | `graph name { ... }` |
| Path in `import` | String literal or bare path | Bare identifier |
| Expression-position | No | Yes (via `result` convention) |

An inline graph is simply a graph defined inside another graph, rather than in its own file. This is the same relationship as an inline class in Java or a nested function in Python — a convenience for co-locating related definitions.

The terminology is now uniform:

- **Graph**: A DAG of declarations (`param`, `node`, `const`, `index`, etc.). Every `.gcl` file is a graph. Every `graph` block is a graph.
- **Import**: Wires one graph into another, with optional bindings.
- **Built-in function**: An atom of computation used within node expressions. Not a graph.

### What Gets Removed

| Current feature | Replacement |
| --- | --- |
| `fn` keyword | `graph` block |
| `let` bindings in function bodies | `node` declarations in graph body |
| `@` prohibition in fn bodies | Not needed (graphs use `@` by design) |
| User-defined function registry (`FunctionRegistry`) | Graph registry |
| Recursion detection on functions | Recursion detection on graph instantiation |
| `<I: Index>` function generics | Required index declarations (`index I;`) in graph body |
| `FnGenericParam` / `FnGenericConstraint` | Required declarations + built-in functions (Dim/Nat generics deferred, see Future Extensions) |
| Function evaluation (`eval_builtin_or_user_fn` for user fns) | Graph instantiation (sub-DAG creation + wiring) |
| User-defined `abs`, `lerp`, `clamp` | Promoted to built-in functions |

### What's Preserved

| Feature | Status |
| --- | --- |
| Built-in functions (`sqrt`, `sin`, `cos`, etc.) | Unchanged |
| Built-in aggregations (`sum`, `mean`, `count`) | Unchanged |
| Parameterized file imports | Unchanged (same mechanism for file and inline graphs) |
| Injectable indexes | Unchanged (natural fit — required declarations in graphs) |

## Examples

### Single-Output Graph (Before/After)

```gcl
// BEFORE (current):
fn orbital_velocity(gm: GravParam, r: Length) -> Velocity = sqrt(gm / r);
node v: Velocity = orbital_velocity(GM_EARTH, R_EARTH + @alt);

// AFTER (proposed):
graph orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node result: Velocity = sqrt(@gm / @r);
}
node v: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt);
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
graph hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    private node v1: Velocity = sqrt(@gm / @r1);
    private node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    node total_dv: Velocity = @dv1 + @dv2;
}
import hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) { dv1, dv2, total_dv };
```

### Index-Parameterized Graph

```gcl
// Graph with required index — reusable over any label set
graph power_budget {
    index Component;                        // required: caller provides the label set
    param power_draw: Power[Component];
    node total_power: Power = sum(for c: Component { @power_draw[c] });
    node max_draw: Power = max(for c: Component { @power_draw[c] });
}

// Instantiate with different indexes:
index Avionics = { IMU, StarTracker, GPS };
index Propulsion = { Thruster, Valve, Tank };

import power_budget(Component: Avionics, power_draw: @avionics_power) as av_budget;
import power_budget(Component: Propulsion, power_draw: @prop_power) as prop_budget;

node total: Power = @av_budget::total_power + @prop_budget::total_power;
```

This is the same mechanism as injectable indexes in file graphs ([doc 23](./23-injectable-index-import.md)), applied to inline graphs. No special generic syntax needed — the `index Component;` declaration is the "type parameter."

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

An inline graph in one file can be imported by another file, just like any other declaration:

```gcl
// rocket.gcl
graph tsiolkovsky {
    param dry_mass: Mass;
    param fuel_mass: Mass;
    param isp: Time;
    node v_exhaust: Velocity = @isp * G0;
    node delta_v: Velocity = @v_exhaust * ln((@dry_mass + @fuel_mass) / @dry_mass);
}
```

```gcl
// main.gcl
import "./rocket.gcl" { tsiolkovsky };
import tsiolkovsky(dry_mass: @my_dry_mass, fuel_mass: @my_fuel, isp: @my_isp) { delta_v };
```

### Mixed Built-in and Graph Calls

```gcl
graph velocity_at_alt {
    param alt: Length;
    node result: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}

// Built-in and graph calls compose in the same expression:
node dv: Velocity = abs(
    velocity_at_alt(alt: @target_alt) - velocity_at_alt(alt: @parking_alt)
);
```

## Open Questions

### Syntax

1. **`result` convention vs. explicit output annotation?** The current design uses a `result` node name convention for expression-position usage. An alternative is explicit output annotation on the graph signature:

    ```gcl
    // Option A: Convention (current design)
    graph orbital_velocity {
        param gm: GravParam;
        param r: Length;
        node result: Velocity = sqrt(@gm / @r);
    }

    // Option B: Explicit output annotation
    graph orbital_velocity(gm: GravParam, r: Length) -> Velocity {
        node result: Velocity = sqrt(@gm / @r);
    }
    ```

    Option B makes the "callable" intent more visible and puts params in the signature (closer to function syntax), but introduces a special form that only applies to single-output graphs.

2. **Keyword: `graph` vs. `dag`?** Two candidates for the block keyword:

    - **`graph`**: Aligns with the language name — "Graphcal is composed of `graph`s." Self-explanatory branding. But creates a name collision in conversation: "a Graphcal graph" is redundant, "the graph keyword in the graph language" is confusing for documentation and tutorials.
    - **`dag`**: Technically precise — the block *is* a DAG, and acyclicity is a core language invariant. Short (3 chars, same as `fn`). Distinctive — no other language uses it. Avoids the name collision: "a Graphcal DAG" is clear. But it is jargon (though the target audience likely knows the term).

    This document uses `graph` throughout, but `dag` is a strong alternative. The choice affects the language name's relationship to its syntax.

3. **Should `graph` blocks support `import` of other graphs (file or inline)?** This enables graph composition at the definition level, not just at the call site.

3. **Nesting?** Can a graph contain another graph definition? If so, is the inner graph scoped to the outer graph, or visible to the enclosing file?

### Scoping

4. **Enclosing graph access:** If graphs can be nested, can inner graphs see the outer graph's `const` declarations? (Proposal: inner graphs see `const`/`dimension`/`unit`/`type`/`index` from all enclosing scopes, but never `param`/`node`.)

### Semantics

5. **Instantiation multiplicity:** If the same graph is called twice with the same arguments, does the compiler create one or two sub-DAGs? (Proposal: always create separate instances. Deduplication is an optimization, not a semantic guarantee.)

6. **Assertions inside graphs:** If a graph contains `assert` declarations, are they evaluated at each instantiation? (Proposal: yes. Assertions are part of the graph's contract.)

7. **Recursive graph instantiation:** A graph instantiating itself (directly or mutually) must be detected and rejected, same as current function recursion detection.

### Built-in Library Boundary

8. **Where is the line between built-in and user-defined?** The built-in library should cover generic, broadly-useful operations. Domain-specific calculations (rocket equation, orbital mechanics, thermal analysis) are user graphs. But what about operations like `normalize`, `cross`, `weighted_sum`? The boundary needs to be drawn carefully to avoid an ever-growing built-in library while ensuring users don't need graph generics for common math.

9. **Can users request new built-ins?** Should there be a mechanism for users to propose additions to the built-in function library, or is this a language-level decision only?

### Migration

10. **Clean break:** Since Graphcal is unpublished, this can be a clean break — remove `fn`, add `graph`, expand the built-in library. No coexistence period needed.

## Generics Status

The "required declarations" pattern (same mechanism as parameterized file imports) covers generics as follows:

| Generic kind | Current `fn` syntax | Inline graph equivalent | Status |
| --- | --- | --- | --- |
| Index | `<I: Index>` | `index I;` (required index) | **Solved** — same mechanism as injectable indexes |
| Dim | `<D: Dim>` | `dimension D;` (required dimension)? | **Open** — see below |
| Nat | `<N: Nat>` | No obvious required-declaration equivalent | **Deferred** — covered by built-in functions for common cases |

### Index Generics: Solved

Required indexes (`index I;`, `index T: Time;`) work identically to `<I: Index>` function generics. The compiler already has index substitution machinery from injectable indexes. No new design needed.

### Dimension Generics: Open Question

Could a "required dimension" declaration serve as `<D: Dim>` generics?

```gcl
graph weighted_sum {
    dimension D;                    // required: caller provides the dimension
    index I;                        // required: caller provides the index
    param values: D[I];
    param weights: Dimensionless[I];
    node result: D = sum(for i: I { @values[i] * @weights[i] });
}

// Caller provides dimension and index:
index Region = { NA, EU, APAC };
node total_revenue: Money = weighted_sum(D: Money, I: Region, values: @revenue, weights: @market_share);
```

This follows the same "required declaration" pattern as indexes. The dimension is declared without a definition inside the graph, and the caller binds it at instantiation.

**Arguments for:**

- Consistent with the required-index pattern — no new concept, just extending to another declaration kind
- The compiler already has dimension substitution in type expressions (analogous to `substitute_type_expr_index_names`)
- Avoids a separate generic syntax (`<D: Dim>`) that only applies to graphs

**Arguments against:**

- Dimensions are structural (products of base dimensions), not named entities like indexes. `D: Money` at the call site is a binding, but unlike index binding (which replaces a name), dimension binding replaces a structural type — the substitution semantics are different.
- Inference is important: `lerp(@x, @y, 0.5)` should infer `D = Length` from `@x: Length` without the caller writing `D: Length`. Can required dimensions support inference? (Possibly: if all required-dimension params are bound with expressions, the compiler can infer the dimension from the expression types.)
- Most dimension-generic operations are covered by built-in functions, so the need may be limited.

**Decision:** Deferred. Start with concrete-typed graphs + built-in functions. If real-world usage demonstrates the need for user-defined dimension-generic graphs, the required-dimension pattern is a natural extension.

### Nat Generics: Deferred

Nat generics (`<N: Nat>`) are used for size-polymorphic operations like `transpose`, `dot`, `drop_last`. These are covered by built-in functions. If user-defined nat-generic graphs are needed later, the mechanism could be a "required nat" declaration, though the syntax is less obvious than for indexes or dimensions.

## Future Extensions

### Required Dimensions in File Graphs

File graphs already support required indexes. Extending to required dimensions would be a natural evolution:

```gcl
// lib.gcl — a file graph with required index and dimension
dimension D;
index I;
param values: D[I];
param weights: Dimensionless[I];
node result: D = sum(for i: I { @values[i] * @weights[i] });
```

This is already how required indexes work in file graphs today. Extending to required dimensions would make file graphs and inline graphs fully equivalent in their parameterization capabilities.

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Inline graphs are sub-DAGs within the computation graph.
- **Scoping** ([08](./08-scoping.md)): `@` resolution within graph bodies.
- **Namespace & Multi-File** ([09](./09-namespace.md)): Graph imports integrate with the existing import system.
- **Indexes** ([07](./07-indexes.md)): Injectable indexes as a precedent for graph parameterization.
- **Injectable Index Import** ([23](./23-injectable-index-import.md)): Graph-level index parameterization.
- **Pure Functions** ([12](./12-pure-functions.md)): This design supersedes doc 12.
- **Type System Stratification** ([20](./20-type-system-stratification.md)): Graphs are declarations, not types.
