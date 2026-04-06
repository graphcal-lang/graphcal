# Inline DAGs, `const` Modifier, and the DAG-as-Module Model

> Replace user-defined pure functions (`fn`) with inline `dag` definitions. Redefine `const` as a modifier (`const node`, `const unit`). Separate `import` (compile-time definitions) from `include` (DAG embedding). Unify around the principle that **a DAG is a module** — files and inline `dag` blocks are the same abstraction, navigated with a single path system.

## Status

**Decision level:** Draft. Core idea accepted for exploration; details being refined. Emerged from discussion around [issue #334](https://github.com/shunichironomura/graphcal/issues/334) ("We don't need `const`") and the need to unify pure functions and parameterized imports.

## Motivation

### Two mechanisms for the same thing

Graphcal currently has two mechanisms for reusable parameterized computation:

1. **Pure functions** (`fn`): Expression-level, single-output, support generics (`Dim`, `Index`, `Nat`). Cannot access the DAG (`@` prohibited).
2. **Parameterized imports**: File-level, multi-output, support injectable indexes and param bindings. Instantiate a file as a sub-DAG.

These solve the same fundamental problem — accepting inputs, computing derived values, producing outputs — but through different mechanisms with different syntax, scoping rules, type systems, and compiler paths.

**The key insight:** A Graphcal file is already a DAG of `param`, `node`, `const`, and `index` declarations. Parameterized imports already instantiate one DAG into another. Pure functions are just a limited, expression-level version of the same thing. If we support defining named DAGs inline (not just as separate files), user-defined functions become unnecessary, and the language collapses to a single concept: **the DAG.**

### The `const` question

Issue #334 asks whether `const` can be removed in favor of `node`. However, `const` carries real semantic weight: it marks a value as **compile-time-known** and independent of runtime `param`s. This matters for evaluation phases, static guarantees, and — critically — scoping in inline DAGs.

### Implicit visibility is an anti-pattern

The original inline DAG design proposed that DAGs implicitly see `const`, `dimension`, `unit`, `type`, and `index` from enclosing scope. This is at odds with Graphcal's explicitness philosophy. Furthermore, `unit` blurs the boundary — a unit can depend on a runtime `param` (e.g., currency exchange rates), making the ad-hoc visibility list incorrect.

### The unified solution

Rather than three separate fixes, this document addresses all three problems together:

1. **`dag` blocks** replace `fn` — one user-defined abstraction
2. **`const` as a modifier** — principled compile-time/runtime distinction
3. **`import`/`include` split** — explicit scoping with no implicit visibility beyond the prelude

## The Spreadsheet Model

The design follows a two-layer model, analogous to how spreadsheets work:

### Layer 1: Built-in Functions (Expression Vocabulary)

Built-in functions are the **atoms of computation** — primitive operations that users compose in expressions, just like `+`, `*`, and other operators. They are not user-defined, do not participate in the DAG as separate nodes, and are evaluated inline within node expressions.

```gcl
node v: Velocity = sqrt(abs(@a - @b) + lerp(@x, @y, 0.5));
```

This is analogous to spreadsheet formulas: `=SQRT(ABS(A1 - B1) + LERP(C1, D1, 0.5))`. Built-in functions can be dimension-generic, support positional arguments, and compose freely in expressions. They are provided by the language, not defined by users.

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

- **One mental model:** Users learn built-in functions (like spreadsheet formulas) and DAGs (like named sub-sheets). No "function definition" concept to learn.
- **Uniform scoping:** Always `@` for DAG references. No special "function scope" where `@` is forbidden.
- **Multi-output for free:** DAGs naturally export multiple values. No wrapper structs needed.
- **Reactive by default:** DAG outputs participate in the reactive computation DAG.
- **Generics are covered by built-ins:** The most common dimension-generic operations (`abs`, `lerp`, `clamp`, `min`, `max`, etc.) are built-in. User DAGs typically have concrete, domain-specific types.

## Design

### The Compile-Time / Runtime Distinction

Every declaration in Graphcal falls into one of two categories:

| Category | Meaning | Examples |
| --- | --- | --- |
| **Compile-time** | Value/definition is fully determined at compile time. Does not transitively depend on any `param`. | `dimension Length;`, `const unit km: Length = 1000.0 m;`, `const node PI_SQUARED: Dimensionless = PI * PI;` |
| **Runtime** | Value depends (directly or transitively) on `param`s. Participates in reactive evaluation. | `param mass: Mass;`, `node force: Force = @mass * 9.81 m/s^2;`, `unit USD: Money = @usd_to_eur;` |

Some declarations are **inherently compile-time** — they can never depend on runtime values:

- `dimension` — defines a dimension (a type-level concept)
- `type` — defines a type (a type-level concept)
- `index` — defines a label set (a type-level concept, though values are indexed by it)

Other declarations **can be either**, depending on whether their definition references `param`s:

- `node` — compile-time if its expression only references other compile-time values; runtime otherwise
- `unit` — compile-time if its scale factor is a compile-time expression; runtime if it depends on a `param`

`param` is **always runtime** — it is, by definition, a user-adjustable input. `const param` is a compile error.

### `const` as a Modifier

The `const` keyword becomes a modifier that the user places on a declaration to **assert** "this is compile-time." The compiler verifies the assertion — if a `const`-marked declaration transitively depends on a `param`, it is a compile error.

`const` must be explicit. Even if a `node` happens not to depend on any `param`, the compiler does **not** automatically treat it as compile-time. The user must write `const` to opt in. This ensures that adding a `param` dependency later produces a compile error at the declaration site, rather than silently changing the node's phase and breaking downstream `import` statements.

```gcl
// Compile-time node (replaces today's `const` declaration)
const node R_EARTH: Length = 6371.0 km;
const node GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

// Compile-time unit (fixed conversion factor)
const unit km: Length = 1000.0 m;

// Runtime unit (depends on param)
param usd_to_eur: Dimensionless = 0.92;
unit USD: Money = @usd_to_eur EUR;

// Runtime node (depends on param)
param alt: Length = 400.0 km;
node velocity: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));

// ERROR: const node depends on a param
// const node bad: Length = @alt + 1.0 m;

// ERROR: const param is a contradiction
// const param x: Length = 5.0 m;
```

**Constness propagates through unit references in expressions.** A literal like `1000.0 USD` implicitly multiplies by the unit's scale factor. If `USD` is a runtime unit, the expression is runtime. The compiler must trace constness through unit scale factors, not just `@`-references.

```gcl
// ERROR: const node uses a runtime unit
// const node budget: Money = 1000.0 USD;
//                                   ^^^ USD depends on param usd_to_eur
```

Today's standalone `const` declaration (`const GM_EARTH: GravParam = ...;`) becomes `const node GM_EARTH: GravParam = ...;`. This unifies the concept: a `const node` is a node that happens to be compile-time-known, participating in the same dependency tracking with the `const` modifier adding a static guarantee.

### DAG = Module

**A DAG is Graphcal's module.** This is the core organizational principle:

- A `.gcl` file is a DAG (and therefore a module).
- An inline `dag` block is a DAG (and therefore a module).
- There is **no semantic difference** between a file DAG and an inline DAG. If you extract an inline DAG into its own file (or inline a file DAG), the semantics are identical — only the path used by importers changes.

This means the module tree is a tree of DAGs, with the file system providing one possible physical layout:

```
myproject/
  constants.gcl              → module: constants
    dag earth { ... }        → module: constants/earth
    dag mars { ... }         → module: constants/mars
  orbital/
    mechanics.gcl            → module: orbital/mechanics
      dag inner { ... }      → module: orbital/mechanics/inner
```

File boundaries and inline DAG boundaries are interchangeable. The path system navigates the module tree uniformly.

### Why `dag`, Not `graph`

The keyword is `dag`, not `graph`, for two reasons:

1. **Precision.** Acyclicity is a core language invariant — it is what makes reactive evaluation well-defined. When something is known to be a DAG, calling it a "graph" under-communicates this guarantee. In practice, people say "DAG" when they mean DAG (build DAGs, git DAGs, dependency DAGs).
2. **Disambiguation.** "A Graphcal graph" is redundant; "a Graphcal DAG" is clear. Documentation, tutorials, and conversations benefit from the keyword being distinct from the language name.

### `import` vs `include`

The current `import` mechanism serves dual purposes: it both brings names into scope and instantiates sub-DAGs. This conflation is the root cause of the scoping confusion in inline DAGs.

We separate these into two mechanisms:

| Mechanism | Purpose | What it does | What can be referenced |
| --- | --- | --- | --- |
| `import` | **Definition import** | Brings a compile-time name into scope. No instantiation, no sub-DAG, no wiring. | `dimension`, `type`, `index`, `const node`, `const unit`, `dag` (the definition, not an instance) |
| `include` | **DAG embedding** | Instantiates a DAG as a sub-DAG, wires params, produces runtime node values. | Runtime `node` values from the instantiated DAG |

The keyword choice follows [Typst's convention](https://typst.app/docs/reference/scripting/#modules): `import` for bringing names into scope, `include` for embedding content.

`import` brings a **name** into scope, not a **value**. Compile-time items have a single, fixed value (or are purely type-level), so "bringing the name into scope" is unambiguous. Runtime items are reactive — "importing" a runtime node would create a hidden dependency, violating the principle that a DAG's runtime dependencies are explicit in its parameter list. Runtime values must be wired through `param`s via `include`.

`import` of compile-time values does **not** create per-instance copies. When a DAG is instantiated multiple times via `include`, the `import`ed compile-time values are shared.

### Module Path System

A single path system navigates the module tree, whether the target is in another file, an inline DAG, or the enclosing scope:

| Path form | Meaning | Example |
| --- | --- | --- |
| `name/name` | Absolute path within a package | `constants/earth` |
| `"./path.gcl"` | Relative file path (unpackaged files) | `"./constants.gcl"` |
| `"../path.gcl"` | Relative file path going up (unpackaged, must stay within compilation root) | `"../shared/constants.gcl"` |
| `..` | Parent scope (enclosing DAG) | `..` |
| `../..` | Grandparent scope | `../..` |
| `path/dag_name` | Sub-DAG within a file or module | `constants/earth`, `"./constants.gcl"/earth` |

For **packaged code**, all paths are bare (no quotes) and resolved from the package root. `..` navigates up the module tree — whether crossing file or inline DAG boundaries:

```gcl
// In package myproject, file orbital/mechanics.gcl:
import constants/earth { GM, R };                // absolute: another module
import ../constants/earth { GM, R };             // relative: up to orbital/, then into constants/earth
import .. { X };                                 // parent scope (inline DAG context)
```

For **unpackaged code**, file references use quoted paths. `..` inside inline DAGs still works for scope traversal:

```gcl
// Unpackaged file:
import "./constants.gcl" { GM, R };              // file reference
import "../shared/constants.gcl" { X };          // file reference going up (within compilation root)
import "./constants.gcl"/earth { GM, R };        // sub-DAG within a file

dag my_dag {
    import .. { GM_EARTH };                      // parent scope (file-level)
}
```

The compiler rejects any path that escapes the compilation root directory.

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

### DAG Parameterization: Not Just Params

**All DAGs are parameterized the same way.** Whether a DAG is a file or an inline block, the caller can inject not just `param` values but also `index` definitions (see [doc 23](./23-injectable-index-import.md)). A DAG can declare **required** declarations that the caller must provide:

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

The binding syntax distinguishes the two kinds by naming convention:

- **PascalCase: PascalCase** → index binding (`I: Subsystem`)
- **snake_case: expr** → param binding (`cost: @sub_cost`)

### Instantiation with `include`

DAGs are instantiated using `include` with **named arguments**. Named arguments are required (not positional), aligning with Graphcal's explicitness philosophy. This also visually distinguishes DAG instantiation (named args via `include`) from built-in function calls (positional args in expressions).

`include` supports two forms: **selective import** (bind specific nodes into the current scope) and **namespace import** (bind the entire instance under a name, access members with `::`).

```gcl
// Selective import — bind specific nodes:
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v };
include hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) { dv1, dv2, total_dv };

// Selective import with renaming:
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v as orbital_v };

// Namespace import — access members with :::
include thermal(material: @mat, area: @panel_area) as thermal;
node q: Power = @thermal::heat_flux * @panel_area;
node t: Temperature = @thermal::surface_temp;

// From another module:
include rocket/tsiolkovsky(dry_mass: @my_dry, fuel_mass: @my_fuel, isp: @my_isp) as r;
node dv: Velocity = @r::delta_v;
```

DAG calls **cannot** appear in expression position. All DAG instantiation goes through `include` — there is no implicit "return value," no `result` convention, and no `output` keyword. This keeps a single, explicit mechanism for DAG instantiation.

### Scoping Rules for Inline DAGs

With `import` and `include` separated, the scoping rules become simple and explicit:

1. **Prelude**: Always available (SI dimensions, units, builtin functions, builtin constants like `PI`). This is a well-defined, documented set.
2. **`import` declarations**: Compile-time items explicitly brought into scope.
3. **Own declarations**: The DAG's own `param`, `node`, `const node`, etc.
4. **Nothing else implicit**: No declarations from enclosing scope leak in unless explicitly `import`ed.

```gcl
const node GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;
const node R_EARTH: Length = 6371.0 km;
param alt: Length = 400.0 km;

dag circular_velocity {
    import .. { GM_EARTH, R_EARTH };  // explicit: compile-time values from enclosing scope

    param alt: Length;
    // GravParam, Length, Velocity — from prelude, no `import` needed
    // GM_EARTH, R_EARTH — from `import` above
    // @alt — own param
    node v: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}

include circular_velocity(alt: @alt) { v };
```

### Reference Syntax

The `@` sigil convention:

- **`@name`** — references runtime values (`param`, non-const `node`) within DAG scope.
- **Bare `name`** — references compile-time values (`const node`, `const unit`, dimensions, types, indexes, builtin constants, builtin functions).

This preserves the current convention and provides instant visual feedback about the compile-time/runtime boundary at every use site:

```gcl
const node GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;
param alt: Length = 400.0 km;

// GM_EARTH is bare (compile-time), @alt has @ (runtime)
node velocity: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
```

The `const` modifier determines the reference syntax: `const node GM_EARTH` is compile-time → bare `GM_EARTH`. Non-const `node velocity` is runtime → `@velocity`.

### Prelude

The prelude provides the baseline vocabulary available everywhere without explicit `import`:

- **SI base and derived dimensions**: `Length`, `Mass`, `Time`, `Velocity`, `Force`, etc.
- **SI base and derived units**: `m`, `kg`, `s`, `km`, `N`, `J`, `W`, etc.
- **Builtin constants**: `PI`, `E` (Euler's number), etc.
- **Builtin functions**: `sqrt`, `abs`, `sin`, `cos`, `min`, `max`, `sum`, etc.

Everything in the prelude is compile-time by nature. The prelude is the only source of implicit visibility — a fixed, documented set, not a scoping rule that depends on context. User-defined compile-time items must be explicitly `import`ed.

### Built-in Function Library

The built-in function library is expanded to cover common generic operations that would otherwise require user-defined generic abstractions:

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

This list is not exhaustive. The key principle is: **if a computation is generic and broadly useful, it should be a built-in function, not a user-defined DAG.**

### Extracting and Inlining: A Pure Refactoring

Because a DAG is a module regardless of whether it's a file or an inline block, extraction and inlining are mechanical refactorings:

```gcl
// Before: rocket.gcl (separate file)
param dry_mass: Mass;
param fuel_mass: Mass;
param isp: Time;
node v_exhaust: Velocity = @isp * G0;
node delta_v: Velocity = @v_exhaust * ln((@dry_mass + @fuel_mass) / @dry_mass);
```

```gcl
// After: inlined into the importing file
dag tsiolkovsky {
    param dry_mass: Mass;
    param fuel_mass: Mass;
    param isp: Time;
    node v_exhaust: Velocity = @isp * G0;
    node delta_v: Velocity = @v_exhaust * ln((@dry_mass + @fuel_mass) / @dry_mass);
}
```

The only change for importers is the path. The DAG's semantics are identical.

## Summary of Changes

| Current | Proposed | Rationale |
| --- | --- | --- |
| `fn` keyword for user-defined functions | `dag` block — the only user-defined abstraction | One concept instead of two |
| `const X: T = expr;` (separate declaration kind) | `const node X: T = expr;` (modifier on `node`) | `const` is a property, not a category |
| All units are implicitly compile-time for scoping | `const unit` vs `unit` — explicit | Units can depend on runtime params |
| Implicit visibility of `const`/`dimension`/`unit`/`type`/`index` in inline DAGs | Explicit `import` declarations + prelude | No implicit scoping beyond the prelude |
| `import` serves dual purpose (definition import + DAG embedding) | `import` for compile-time definitions, `include` for DAG embedding | Principled separation of concerns |
| `graph` keyword | `dag` keyword | Precision (acyclicity is a core invariant) and disambiguation from language name |
| File DAGs and inline DAGs have different semantics | DAG = Module — uniform semantics | Extraction/inlining is a pure refactoring |
| Ad-hoc path syntax | Unified module path system (`/` navigates down, `..` navigates up) | One path system for all references |

### What Gets Removed

| Current feature | Replacement |
| --- | --- |
| `fn` keyword | `dag` block |
| `let` bindings in function bodies | `node` declarations in DAG body |
| `@` prohibition in fn bodies | Not needed (DAGs use `@` by design) |
| User-defined function registry (`FunctionRegistry`) | DAG registry |
| Recursion detection on functions | Recursion detection on DAG instantiation |
| `<I: Index>` function generics | Required index declarations (`index I;`) in DAG body |
| `FnGenericParam` / `FnGenericConstraint` | Required declarations + built-in functions (Dim/Nat generics deferred) |
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
dag power_budget {
    index Component;                        // required: caller provides the label set
    param power_draw: Power[Component];
    node total_power: Power = sum(for c: Component { @power_draw[c] });
    node max_draw: Power = max(for c: Component { @power_draw[c] });
}

index Avionics = { IMU, StarTracker, GPS };
index Propulsion = { Thruster, Valve, Tank };

include power_budget(Component: Avionics, power_draw: @avionics_power) as av_budget;
include power_budget(Component: Propulsion, power_draw: @prop_power) as prop_budget;

node total: Power = @av_budget::total_power + @prop_budget::total_power;
```

### Scoping with `import` and `include`

```gcl
const node GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;
const node R_EARTH: Length = 6371.0 km;

dag circular_velocity {
    import .. { GM_EARTH, R_EARTH };  // explicit compile-time import

    param alt: Length;
    node v: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}

dag outer {
    const node B: Dimensionless = 2.0;

    dag inner {
        import .. { B };              // from outer
        import ../.. { GM_EARTH };    // from file scope
    }
}
```

### Reuse Across Files

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

### `const` and `import` Interaction

```gcl
// earth.gcl
dag earth {
    const node GM: GravParam = 3.986004418e5 km^3/s^2;
    const node R: Length = 6371.0 km;
    param surface_temp: Temperature = 288.0 K;
}

// main.gcl — import const values without instantiation
import earth { GM, R };           // OK: const values, no instantiation needed
// import earth { surface_temp }; // ERROR: runtime value, must use include
```

## Settled Design Decisions

### `private` keyword

Deferred. Visibility control will be designed separately. For now, all declarations are public.

### Re-exports

Allowed. If a DAG imports a compile-time item, that item becomes part of the DAG's public scope and can be imported by others through that DAG's path:

```gcl
// facade.gcl — re-exports from multiple sources
import constants/earth { GM_EARTH, R_EARTH };
import constants/mars { GM_MARS, R_MARS };
// Consumers can now: import facade { GM_EARTH, GM_MARS };
```

### Glob imports

Prohibited. All imports must name their items explicitly.

### Namespace access syntax

`::` is used at use sites to access members of a namespaced `include` (e.g., `@r::delta_v`). `/` is used in `import`/`include` paths to navigate the module tree. `/` is for *finding* a module, `::` is for *accessing* its members.

### Declaration ordering

Does not matter. A DAG is a set of declarations — the compiler resolves the topological order. Forward references are valid.

### Importing `const` values from inside a DAG

Allowed without instantiation. Evaluating `const` values is not "instantiation." This works even if the DAG has required params — `const` values are independent of params by definition.

### `const index`

Not introduced. All indexes are inherently compile-time. Injectable indexes are resolved at compile time (monomorphization).

### `const` on `dag` definitions

Deferred. Minimal gain over separate files/DAGs of `const node` declarations.

### `include` without param bindings

Deferred. Empty parens required for now: `include my_dag() { x }`.

### Type-level items in inline DAGs

User-defined type-level items (`dimension`, `type`) must be explicitly `import`ed. Only the prelude is implicit:

```gcl
dimension GravParam = Length^3 * Time^-2;

dag circular_velocity {
    import .. { GravParam };  // required — user-defined dimension
    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
```

### Circular `const` dependencies

Compile error. The compiler topologically sorts all compile-time declarations and rejects cycles.

### Prelude shadowing

Compile error. Silently overriding `m` or `kg` could cause subtle calculation errors.

### No expression-position DAG calls

All DAG instantiation goes through `include`. No implicit return value, no `result` convention, no `output` keyword.

```gcl
// NOT allowed:
// node v: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) * 2.0;

// Instead:
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v };
node result: Velocity = @v * 2.0;
```

## Open Questions

### Semantics

1. **Instantiation multiplicity:** If the same DAG is `include`d twice with the same arguments, does the compiler create one or two sub-DAGs? (Proposal: always create separate instances. Deduplication is an optimization, not a semantic guarantee.)

2. **Assertions inside DAGs:** If a DAG contains `assert` declarations, are they evaluated at each instantiation? (Proposal: yes. Assertions are part of the DAG's contract.)

3. **Recursive DAG instantiation:** A DAG instantiating itself (directly or mutually) must be detected and rejected.

### Built-in Library Boundary

4. **Where is the line between built-in and user-defined?** Domain-specific calculations are user DAGs. But what about `normalize`, `cross`, `weighted_sum`?

5. **Can users request new built-ins?** Language-level decision only, or is there a proposal mechanism?

### Migration

6. **Clean break:** Since Graphcal is unpublished, this can be a clean break — remove `fn`, add `dag`, expand the built-in library. No coexistence period needed.

## Generics Status

| Generic kind | Current `fn` syntax | Inline DAG equivalent | Status |
| --- | --- | --- | --- |
| Index | `<I: Index>` | `index I;` (required index) | **Solved** — same mechanism as injectable indexes |
| Dim | `<D: Dim>` | `dimension D;` (required dimension)? | **Open** — see below |
| Nat | `<N: Nat>` | No obvious required-declaration equivalent | **Deferred** — covered by built-in functions |

### Index Generics: Solved

Required indexes (`index I;`, `index T: Time;`) work identically to `<I: Index>` function generics. No new design needed.

### Dimension Generics: Open Question

Could a "required dimension" declaration serve as `<D: Dim>` generics?

```gcl
dag weighted_sum {
    dimension D;
    index I;
    param values: D[I];
    param weights: Dimensionless[I];
    node result: D = sum(for i: I { @values[i] * @weights[i] });
}

index Region = { NA, EU, APAC };
include weighted_sum(D: Money, I: Region, values: @revenue, weights: @market_share) { result as total_revenue };
```

**Decision:** Deferred. Start with concrete-typed DAGs + built-in functions. The required-dimension pattern is a natural extension if needed.

### Nat Generics: Deferred

Covered by built-in functions for common cases (`transpose`, `dot`, `drop_last`).

## Dependencies

- **[01 — Computation Model](./01-computation-model.md)**: Redefines the `const` evaluation phase. Inline DAGs are sub-DAGs within the computation DAG.
- **[04 — Dimensions & Units](./04-dimensions-and-units.md)**: `const unit` vs runtime `unit` distinction.
- **[07 — Indexes](./07-indexes.md)**: Injectable indexes as a precedent for DAG parameterization.
- **[08 — Scoping](./08-scoping.md)**: Reference syntax — bare names for compile-time, `@` for runtime.
- **[09 — Namespace & Multi-File](./09-namespace.md)**: `import`/`include` split replaces the current single `import`. Unified module path system supersedes current conventions.
- **[12 — Pure Functions](./12-pure-functions.md)**: This design supersedes doc 12.
- **[20 — Type System Stratification](./20-type-system-stratification.md)**: DAGs are declarations, not types.
- **[23 — Injectable Index Import](./23-injectable-index-import.md)**: DAG-level index parameterization. All indexes are inherently compile-time.
