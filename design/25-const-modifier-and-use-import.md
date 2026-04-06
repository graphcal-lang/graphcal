# `const` Modifier and `use` vs `import` — Compile-Time vs Runtime Declarations

> Redefine `const` from a standalone declaration kind to a **modifier** on declarations (`const node`, `const unit`), and introduce a `use` mechanism distinct from `import` for bringing compile-time definitions into scope. This clarifies the compile-time / runtime boundary and provides principled scoping rules for inline graphs.

## Status

**Decision level:** Draft. Emerged from discussion around [issue #334](https://github.com/shunichironomura/graphcal/issues/334) ("We don't need `const`") and the inline graph scoping rules in [doc 24](./24-inline-modules.md).

## Motivation

### The original question: do we need `const`?

Issue #334 asks whether `const` can be removed in favor of `node`. On the surface, both declare named values with type annotations and expressions. Removing `const` would simplify the language.

However, `const` carries real semantic weight: it marks a value as **compile-time-known** and independent of runtime `param`s. This distinction matters for:

- **Scoping in inline graphs** ([doc 24](./24-inline-modules.md)): The design proposes that graphs can see `const`, `dimension`, `unit`, `type`, and `index` from enclosing scope, but not `param` or `node`. This implicit visibility rule is the mechanism that lets graphs reference fixed values without threading them as params.
- **Evaluation phases**: Consts are evaluated before the runtime DAG, enabling optimizations like durability-based incremental recomputation.
- **Static guarantees**: A `const` cannot depend on a `param`, and the compiler enforces this.

### The deeper issue: implicit visibility is an anti-pattern

Graphcal's philosophy is explicitness over implicitness. The rule "graphs implicitly see certain declarations from enclosing scope" is at odds with this. Which declarations leak in? Today's answer — `const`, `dimension`, `unit`, `type`, `index` — is a somewhat ad-hoc list.

Furthermore, `unit` blurs the boundary. A unit can depend on a runtime `param` (e.g., currency exchange rates injected at runtime), making it a runtime value. But a unit can also be a fixed conversion factor (SI units), making it compile-time. The current design treats all units as compile-time for scoping purposes, which is incorrect for runtime-dependent units.

### The solution: `const` as a modifier, `use` as explicit import

Rather than removing `const` or keeping it as a separate declaration kind, we redefine it as a **modifier** that can be applied to any declaration to mark it as compile-time. Combined with a `use` mechanism (distinct from `import`), this gives users explicit control over what crosses scope boundaries.

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

`param` is **always runtime** — it is, by definition, a user-adjustable input.

### `const` as a Modifier

The `const` keyword becomes a modifier that the user places on a declaration to assert "this is compile-time." The compiler verifies the assertion — if a `const`-marked declaration transitively depends on a `param`, it is a compile error.

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

This is not just a syntactic change. It unifies the concept: a `const node` is a node that happens to be compile-time-known. It uses the same reference syntax (`@` inside graphs, or bare name — see Open Questions), participates in the same dependency tracking, and is declared with the same `node` keyword. The `const` modifier simply adds a static guarantee.

### `use` vs `import` — Two Kinds of Cross-Scope Reference

The current `import` mechanism serves dual purposes: it both brings names into scope and instantiates sub-DAGs. This conflation is the root cause of the scoping confusion in inline graphs.

We propose separating these into two mechanisms:

| Mechanism | Purpose | What it does | What can be referenced |
| --- | --- | --- | --- |
| `use` | **Definition import** | Brings a compile-time name into scope. No instantiation, no sub-DAG, no wiring. | `dimension`, `type`, `index`, `const node`, `const unit`, `graph` (the definition, not an instance) |
| `import` | **Graph embedding** | Instantiates a graph as a sub-DAG, wires params, produces runtime node values. | Runtime `node` values from the instantiated graph |

#### `use` Examples

```gcl
// Use compile-time definitions from another file
use "./constants.gcl" { R_EARTH, GM_EARTH };
use "./dimensions.gcl" { GravParam };

// Use an inline graph definition (not an instance)
use "./lib.gcl" { orbital_velocity };

// Use inside an inline graph
graph circular_velocity {
    use GM_EARTH, R_EARTH;  // from enclosing scope or file-level use

    param alt: Length;
    node result: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}
```

#### `import` Examples (Unchanged)

```gcl
// Embed a graph — instantiate and wire
import orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt) { result as v };
import "./thermal.gcl"(material: @mat, area: @panel_area) as thermal;
```

### Scoping Rules for Inline Graphs

With `use` and `import` separated, the scoping rules for inline graphs become simple and explicit:

1. **Prelude**: Always available (SI dimensions, units, builtin functions, builtin constants like `PI`). This is a well-defined, documented set.
2. **`use` declarations**: Compile-time items explicitly brought into scope.
3. **Own declarations**: The graph's own `param`, `node`, `const node`, etc.
4. **Nothing else implicit**: No declarations from enclosing scope leak in unless explicitly `use`d.

```gcl
const node GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;
const node R_EARTH: Length = 6371.0 km;
param alt: Length = 400.0 km;

graph circular_velocity {
    use GM_EARTH, R_EARTH;  // explicit: compile-time values from enclosing scope

    param alt: Length;
    // GravParam, Length, Velocity — from prelude, no `use` needed
    // GM_EARTH, R_EARTH — from `use` above
    // @alt — own param
    node result: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}

node v: Velocity = circular_velocity(alt: @alt);
```

### Why `use` Only Allows Compile-Time Items

The restriction is principled: `use` brings a **name** into scope, not a **value**. Compile-time items have a single, fixed value (or are purely type-level), so "bringing the name into scope" is unambiguous — there is exactly one `GM_EARTH`, and it never changes.

Runtime items (`param`, non-const `node`) are reactive — their values change based on inputs. "Using" a runtime node from enclosing scope would create a hidden dependency, violating the principle that a graph's runtime dependencies are explicit in its parameter list. Runtime values must be wired through `param`s via `import`.

### Prelude

The prelude provides the baseline vocabulary available everywhere without explicit `use`. It contains:

- **SI base and derived dimensions**: `Length`, `Mass`, `Time`, `Velocity`, `Force`, etc.
- **SI base and derived units**: `m`, `kg`, `s`, `km`, `N`, `J`, `W`, etc.
- **Builtin constants**: `PI`, `E` (Euler's number), etc.
- **Builtin functions**: `sqrt`, `abs`, `sin`, `cos`, `min`, `max`, `sum`, etc.

Everything in the prelude is compile-time by nature. The prelude is the only source of implicit visibility — and it is a fixed, documented set, not a scoping rule that depends on context.

User-defined compile-time items (custom dimensions, units, `const node`s) are never in the prelude. They must be explicitly `use`d.

## Summary of Changes

| Current | Proposed | Rationale |
| --- | --- | --- |
| `const X: T = expr;` (separate declaration kind) | `const node X: T = expr;` (modifier on `node`) | `const` is a property, not a category |
| All units are implicitly compile-time for scoping | `const unit` vs `unit` — explicit | Units can depend on runtime params |
| Implicit visibility of `const`/`dimension`/`unit`/`type`/`index` in inline graphs | Explicit `use` declarations + prelude | No implicit scoping beyond the prelude |
| `import` serves dual purpose (definition import + graph embedding) | `use` for compile-time definitions, `import` for graph embedding | Principled separation of concerns |

## Reference Syntax

The `@` sigil and bare-name conventions need revisiting in light of this design. Today:

- `@name` — references `param`, `node`, or `const` within graph scope
- `BARE_NAME` — references `const` (uppercase convention) or builtin

With `const` as a modifier on `node`, `const node GM_EARTH` is both a `node` (suggesting `@GM_EARTH`) and compile-time (suggesting bare `GM_EARTH`). The reference syntax should be decided in conjunction with [doc 08 (Scoping)](./08-scoping.md).

Options include:

1. **All values use `@`**: `const node` values are referenced as `@GM_EARTH`. Simple, uniform. The `const` modifier only affects compile-time checking and `use` eligibility, not reference syntax.
2. **`const` values use bare names**: Preserves the current convention where compile-time values don't need `@`. Readers can tell at a glance whether a reference is compile-time or runtime.
3. **`use`d items use bare names**: Any item brought into scope via `use` is referenced by bare name. This ties the syntax to the import mechanism rather than the compile-time property.

## Open Questions

1. **Should `use` be required inside inline graphs for type-level items too?** Requiring `use Length, Velocity;` in every graph is verbose. The prelude handles common types, but what about user-defined dimensions? Options:
   - Prelude only: user-defined dimensions must be `use`d. (Maximally explicit.)
   - Type-level items (`dimension`, `type`) always visible; value-level items (`const node`, `const unit`) require `use`. (Less explicit, but pragmatic — you need types to even write declarations.)

2. **Syntax for `use` from enclosing scope vs from a file**: Is `use GM_EARTH;` (bare, from enclosing scope) different from `use "./constants.gcl" { GM_EARTH };` (from a file)? Or does `use` always name a source?

3. **Can `const` be inferred?** If a `node` happens not to depend on any `param`, should the compiler automatically treat it as compile-time (allowing it to be `use`d)? Or must the user explicitly write `const`? The explicitness philosophy suggests requiring the modifier, but inference reduces boilerplate.

4. **`const index`?** Indexes are currently always compile-time. With injectable indexes ([doc 23](./23-injectable-index-import.md)), an index could be "provided at instantiation time" — is that compile-time or runtime? If indexes can be runtime-dependent, should `const index` exist?

5. **Reference syntax**: See the Reference Syntax section above. The choice affects readability and consistency.

6. **`const` on `graph` definitions?** Should a graph itself be markable as `const` (meaning all its outputs are compile-time)? This would allow `use`ing the graph's outputs directly without instantiation, and could be useful for reusable compile-time computations (e.g., unit conversion tables).

## Dependencies

- **[01 — Computation Model](./01-computation-model.md)**: Redefines the `const` evaluation phase as applying to `const`-modified declarations rather than a separate declaration kind.
- **[04 — Dimensions & Units](./04-dimensions-and-units.md)**: `const unit` vs runtime `unit` distinction.
- **[08 — Scoping](./08-scoping.md)**: `use` interacts with `@` sigil rules. Reference syntax for `const node` values.
- **[09 — Namespace & Multi-File](./09-namespace.md)**: `use` is a new import mechanism alongside `import`.
- **[24 — Inline Graphs](./24-inline-modules.md)**: `use` replaces the implicit visibility rules for inline graph scoping. This design directly addresses the scoping open questions in doc 24.
- **[23 — Injectable Index Import](./23-injectable-index-import.md)**: Index injection interacts with compile-time/runtime classification.
