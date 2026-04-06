# `const` Modifier, `import` / `include`, and the DAG-as-Module Model

> Redefine `const` from a standalone declaration kind to a **modifier** on declarations (`const node`, `const unit`). Introduce an `import` / `include` distinction: `import` brings compile-time definitions into scope, `include` embeds a DAG as a sub-DAG. Unify scoping around the principle that **a DAG is a module** — files and inline `dag` blocks are the same abstraction, navigated with a single path system.

## Status

**Decision level:** Draft. Emerged from discussion around [issue #334](https://github.com/shunichironomura/graphcal/issues/334) ("We don't need `const`") and the inline DAG scoping rules in [doc 24](./24-inline-modules.md).

## Motivation

### The original question: do we need `const`?

Issue #334 asks whether `const` can be removed in favor of `node`. On the surface, both declare named values with type annotations and expressions. Removing `const` would simplify the language.

However, `const` carries real semantic weight: it marks a value as **compile-time-known** and independent of runtime `param`s. This distinction matters for:

- **Scoping in inline DAGs** ([doc 24](./24-inline-modules.md)): The design proposes that DAGs can see `const`, `dimension`, `unit`, `type`, and `index` from enclosing scope, but not `param` or `node`. This implicit visibility rule is the mechanism that lets DAGs reference fixed values without threading them as params.
- **Evaluation phases**: Consts are evaluated before the runtime DAG, enabling optimizations like durability-based incremental recomputation.
- **Static guarantees**: A `const` cannot depend on a `param`, and the compiler enforces this.

### The deeper issue: implicit visibility is an anti-pattern

Graphcal's philosophy is explicitness over implicitness. The rule "DAGs implicitly see certain declarations from enclosing scope" is at odds with this. Which declarations leak in? Today's answer — `const`, `dimension`, `unit`, `type`, `index` — is a somewhat ad-hoc list.

Furthermore, `unit` blurs the boundary. A unit can depend on a runtime `param` (e.g., currency exchange rates injected at runtime), making it a runtime value. But a unit can also be a fixed conversion factor (SI units), making it compile-time. The current design treats all units as compile-time for scoping purposes, which is incorrect for runtime-dependent units.

### The solution: `const` as a modifier, `import` vs `include`

Rather than removing `const` or keeping it as a separate declaration kind, we redefine it as a **modifier** that can be applied to any declaration to mark it as compile-time. Combined with a clear separation between `import` (bring compile-time names into scope) and `include` (embed a DAG as a sub-DAG), this gives users explicit control over what crosses scope boundaries.

The keyword choice follows [Typst's convention](https://typst.app/docs/reference/scripting/#modules): `import` for bringing names into scope, `include` for embedding content.

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

### What the Current `const` Becomes

Today's standalone `const` declaration:

```gcl
// Current syntax
const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;
```

becomes:

```gcl
// New syntax
const node GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;
```

This is not just a syntactic change. It unifies the concept: a `const node` is a node that happens to be compile-time-known. It participates in the same dependency tracking and is declared with the same `node` keyword. The `const` modifier simply adds a static guarantee.

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

### `import` vs `include` — Two Kinds of Cross-Scope Reference

The current `import` mechanism serves dual purposes: it both brings names into scope and instantiates sub-DAGs. This conflation is the root cause of the scoping confusion in inline DAGs.

We separate these into two mechanisms:

| Mechanism | Purpose | What it does | What can be referenced |
| --- | --- | --- | --- |
| `import` | **Definition import** | Brings a compile-time name into scope. No instantiation, no sub-DAG, no wiring. | `dimension`, `type`, `index`, `const node`, `const unit`, `dag` (the definition, not an instance) |
| `include` | **DAG embedding** | Instantiates a DAG as a sub-DAG, wires params, produces runtime node values. | Runtime `node` values from the instantiated DAG |

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

### `import` Examples

```gcl
// Import compile-time definitions from another module
import constants/earth { R_EARTH, GM_EARTH };
import dimensions { GravParam };

// Import a DAG definition (not an instance)
import lib { orbital_velocity };

// Import inside an inline DAG — from enclosing scope
dag circular_velocity {
    import .. { GM_EARTH, R_EARTH };

    param alt: Length;
    node v: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}

// Nested DAGs
dag outer {
    const node B: Dimensionless = 2.0;

    dag inner {
        import .. { B };              // from outer
        import ../.. { GM_EARTH };    // from file scope
    }
}
```

### `include` Examples

`include` instantiates a DAG as a sub-DAG. It supports two forms: **selective import** (bind specific nodes into the current scope) and **namespace import** (bind the entire instance under a name, access members with `::`).

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

DAG calls **cannot** appear in expression position. All DAG instantiation goes through `include` — there is no implicit "return value" or `result` convention. This keeps a single, explicit mechanism for DAG instantiation and avoids the need for an `output` keyword or magic node names.

### Why `import` Only Allows Compile-Time Items

The restriction is principled: `import` brings a **name** into scope, not a **value**. Compile-time items have a single, fixed value (or are purely type-level), so "bringing the name into scope" is unambiguous — there is exactly one `GM_EARTH`, and it never changes.

Runtime items (`param`, non-const `node`) are reactive — their values change based on inputs. "Importing" a runtime node from enclosing scope would create a hidden dependency, violating the principle that a DAG's runtime dependencies are explicit in its parameter list. Runtime values must be wired through `param`s via `include`.

`import` of compile-time values does **not** create per-instance copies. When a DAG is instantiated multiple times via `include`, the `import`ed compile-time values are shared — they have one fixed value.

### Scoping Rules for Inline DAGs

With `import` and `include` separated, the scoping rules for inline DAGs become simple and explicit:

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
    node result: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}

include circular_velocity(alt: @alt) { v };
```

### Prelude

The prelude provides the baseline vocabulary available everywhere without explicit `import`. It contains:

- **SI base and derived dimensions**: `Length`, `Mass`, `Time`, `Velocity`, `Force`, etc.
- **SI base and derived units**: `m`, `kg`, `s`, `km`, `N`, `J`, `W`, etc.
- **Builtin constants**: `PI`, `E` (Euler's number), etc.
- **Builtin functions**: `sqrt`, `abs`, `sin`, `cos`, `min`, `max`, `sum`, etc.

Everything in the prelude is compile-time by nature. The prelude is the only source of implicit visibility — and it is a fixed, documented set, not a scoping rule that depends on context.

User-defined compile-time items (custom dimensions, units, `const node`s) are never in the prelude. They must be explicitly `import`ed.

Prelude units are `const unit`s. A user-defined unit with the same name in file scope shadows the prelude unit (consistent with resolution order: own scope → imports → prelude). Whether a `const unit` inside an inline DAG can shadow a prelude unit follows the same rule.

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

With `const` as a modifier on `node`, `const node GM_EARTH` is compile-time and therefore referenced as bare `GM_EARTH`. A non-const `node velocity` is runtime and therefore referenced as `@velocity`. The `const` modifier determines the reference syntax.

### Extracting and Inlining: A Pure Refactoring

Because a DAG is a module regardless of whether it's a file or an inline block, extraction and inlining are mechanical refactorings:

**Inlining a file DAG:**

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

The only change for importers is the path: `import rocket { tsiolkovsky }` → `import .. { tsiolkovsky }` (or wherever the DAG now lives). The DAG's semantics are identical.

## Summary of Changes

| Current | Proposed | Rationale |
| --- | --- | --- |
| `const X: T = expr;` (separate declaration kind) | `const node X: T = expr;` (modifier on `node`) | `const` is a property, not a category |
| All units are implicitly compile-time for scoping | `const unit` vs `unit` — explicit | Units can depend on runtime params |
| Implicit visibility of `const`/`dimension`/`unit`/`type`/`index` in inline DAGs | Explicit `import` declarations + prelude | No implicit scoping beyond the prelude |
| `import` serves dual purpose (definition import + DAG embedding) | `import` for compile-time definitions, `include` for DAG embedding | Principled separation of concerns |
| `graph` keyword | `dag` keyword | Precision (acyclicity is a core invariant) and disambiguation from language name |
| File DAGs and inline DAGs have different semantics | DAG = Module — uniform semantics | Extraction/inlining is a pure refactoring |
| Ad-hoc path syntax | Unified module path system (`/` navigates down, `..` navigates up) | One path system for all references |

## Settled Design Decisions

The following questions were raised during design review and have been resolved:

### `private` keyword

Deferred. Visibility control (`private` on declarations) will be designed separately. For now, all declarations are public.

### Re-exports

Allowed. If a DAG imports a compile-time item, that item becomes part of the DAG's public scope and can be imported by others through that DAG's path. This enables "facade" modules that re-export curated sets of definitions:

```gcl
// facade.gcl — re-exports from multiple sources
import constants/earth { GM_EARTH, R_EARTH };
import constants/mars { GM_MARS, R_MARS };
// Consumers can now: import facade { GM_EARTH, GM_MARS };
```

### Glob imports

Prohibited. Glob imports (`import constants/earth { * }`) are an anti-pattern — they obscure where names come from and create fragile implicit dependencies. All imports must name their items explicitly.

### Namespace access syntax

`::` is used at use sites to access members of a namespaced `include`, consistent with the existing variant access syntax (`Phase::Launch`). `/` is used in `import`/`include` paths to navigate the module tree. The two do not conflict — `/` is for *finding* a module, `::` is for *accessing* its members:

```gcl
// / in paths (where you find things)
include rocket/tsiolkovsky(dry_mass: @my_dry, fuel_mass: @my_fuel, isp: @my_isp) as r;

// :: at use sites (accessing members)
node dv: Velocity = @r::delta_v;
node mr: Dimensionless = @r::mass_ratio;
```

### Declaration ordering

Declaration order does not matter. A DAG is a set of declarations forming a dependency graph — the compiler resolves the topological order. Forward references are valid:

```gcl
node velocity: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
param alt: Length = 400.0 km;  // declared after use — OK
```

This applies to `import`, `include`, `const node`, `node`, `param`, `dag`, and all other declarations.

### Importing `const` values from inside a DAG

You can `import` compile-time items from inside a DAG without instantiating it via `include`. Evaluating `const` values is not "instantiation" — there is no sub-DAG creation, no param wiring, no runtime computation. The compiler simply evaluates the compile-time expression:

```gcl
// earth.gcl
dag earth {
    const node GM: GravParam = 3.986004418e5 km^3/s^2;
    const node R: Length = 6371.0 km;
    param surface_temp: Temperature = 288.0 K;  // runtime — not importable
}

// main.gcl
import earth { GM, R };           // OK: const values, no instantiation needed
// import earth { surface_temp }; // ERROR: runtime value, must use include
```

This works even if the DAG has required params — the `const` values are independent of params by definition.

### `const index`

Not introduced. All indexes are inherently compile-time. Injectable indexes ([doc 23](./23-injectable-index-import.md)) are "unbound until instantiation," which is resolved at compile time (monomorphization), not a runtime dependency.

### `const` on `dag` definitions

Deferred. This is isomorphic to a file/DAG of `const node` declarations and adds complexity for minimal gain.

### `include` without param bindings

Deferred. For now, empty parens are required when all params have defaults: `include my_dag() { x }`. Whether to allow omitting the parens (`include my_dag { x }`) will be decided later.

### Type-level items in inline DAGs

User-defined type-level items (`dimension`, `type`) must be explicitly `import`ed into inline DAGs, same as value-level items. Only the prelude is implicit. This is maximally explicit and consistent — there is one rule for all cross-scope references.

```gcl
dimension GravParam = Length^3 * Time^-2;

dag circular_velocity {
    import .. { GravParam };  // required — user-defined dimension
    // Length, Velocity — from prelude, no import needed

    param gm: GravParam;
    param r: Length;
    node v: Velocity = sqrt(@gm / @r);
}
```

### Circular `const` dependencies

Circular `const` dependencies are a compile error. The compiler topologically sorts all compile-time declarations (`const node`, `const unit`) and rejects cycles:

```gcl
// ERROR: circular dependency
const node a: Dimensionless = b + 1.0;
const node b: Dimensionless = a + 1.0;
```

Non-circular cross-references between `const node` and `const unit` are valid — the compiler resolves the evaluation order.

### Prelude shadowing

Shadowing prelude names is a compile error. Graphcal prioritizes safety — silently overriding a prelude unit like `m` or `kg` could cause subtle calculation errors. If a user needs a different definition, they should use a different name.

```gcl
// ERROR: cannot shadow prelude unit
// const unit m: Length = 0.001 km;

// OK: different name
const unit meter_custom: Length = 0.001 km;
```

### No expression-position DAG calls

DAG calls cannot appear in expression position. All DAG instantiation goes through `include`. There is no implicit "return value," no `result` convention, and no `output` keyword. This keeps a single, explicit mechanism for DAG instantiation.

```gcl
// NOT allowed:
// node v: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) * 2.0;

// Instead:
include orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { v };
node result: Velocity = @v * 2.0;
```

Doc 24's expression-position desugaring, `result` convention, and nested graph calls sections are superseded by this decision.

## Dependencies

- **[01 — Computation Model](./01-computation-model.md)**: Redefines the `const` evaluation phase as applying to `const`-modified declarations rather than a separate declaration kind.
- **[04 — Dimensions & Units](./04-dimensions-and-units.md)**: `const unit` vs runtime `unit` distinction.
- **[08 — Scoping](./08-scoping.md)**: Reference syntax — bare names for compile-time, `@` for runtime.
- **[09 — Namespace & Multi-File](./09-namespace.md)**: `import` / `include` split replaces the current single `import` mechanism. The unified module path system supersedes the current path conventions.
- **[24 — Inline DAGs](./24-inline-modules.md)**: `import` replaces the implicit visibility rules for inline DAG scoping. Expression-position desugaring uses `include`. This design directly addresses the scoping open questions in doc 24.
- **[23 — Injectable Index Import](./23-injectable-index-import.md)**: Index injection interacts with compile-time/runtime classification. All indexes are inherently compile-time.
