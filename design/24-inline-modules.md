# Inline Modules — Unifying Functions and Modules

> Replace pure functions (`fn`) with inline module definitions, making every reusable computation a sub-DAG. Combined with call-site syntax sugar, this preserves expression ergonomics while collapsing the language into a single abstraction: the DAG.

## Status

**Decision level:** Draft. Core idea accepted for exploration; syntax, generics, and scoping details need refinement.

## Motivation

Graphcal currently has two mechanisms for reusable parameterized computation:

1. **Pure functions** (`fn`): Expression-level, single-output, support generics (`Dim`, `Index`, `Nat`). Cannot access the graph (`@` prohibited).
2. **Parameterized imports**: Module-level, multi-output, support injectable indexes and param bindings. Instantiate a file as a sub-DAG.

These solve the same fundamental problem — accepting inputs, computing derived values, producing outputs — but through different mechanisms with different syntax, scoping rules, type systems, and compiler paths. This redundancy adds conceptual and implementation complexity.

**The key insight:** Every pure function can be expressed as a parameterized module instantiation. A function's parameters become `param` declarations; its body becomes `node` declarations; its return value becomes an exported node. If we support defining modules inline (not just in separate files), functions become unnecessary.

### What We Gain

- **One mental model:** Everything is a DAG. Users learn one abstraction, not two.
- **Uniform scoping:** Always `@` for graph references. No special "function scope" where `@` is forbidden.
- **Multi-output for free:** Modules naturally export multiple values. No wrapper structs needed.
- **Reactive by default:** Module outputs participate in the reactive graph. Function calls are opaque to the DAG.

### What We Must Preserve

- **Expression-position usage:** `node v: Velocity = lerp(a: @x, b: @y, t: 0.5)` must remain concise.
- **Generics:** Dimension, index, and nat polymorphism are essential for engineering calculations.
- **Composability:** Nested calls like `sqrt(abs(x: @a))` must work.

## Design

### Inline Module Declaration

A `module` block defines a reusable sub-DAG within a file:

```gcl
module orbital_velocity {
    param gm: GravParam;
    param r: Length;
    node result: Velocity = sqrt(@gm / @r);
}
```

A module block can contain the same declarations as a file: `param`, `node`, `const`, `assert`, `type`, `dimension`, `unit`, `index`. It is a self-contained scope.

### Generic Modules

Modules support the same generic constraints as functions today:

```gcl
// Dim generic
module lerp<D: Dim> {
    param a: D;
    param b: D;
    param t: Dimensionless;
    node result: D = @a + (@b - @a) * @t;
}

// Nat generic
module transpose<M: Nat, N: Nat, D: Dim> {
    param a: D[M, N];
    node result: D[N, M] = for j: range(N), i: range(M) { @a[i, j] };
}

// Index generic
module total_cost<I: Index> {
    param cost: Dimensionless[I];
    node total: Dimensionless = sum(for i: I { @cost[i] });
}
```

Generic resolution works the same way as current function generics:

- **Dim**: Inferred from argument types at the call site, or specified via turbofish.
- **Nat**: Inferred from argument shapes (with linear equation solving for expressions like `N + 1`), or specified via turbofish.
- **Index**: Bound explicitly (like injectable indexes) or inferred from argument types.

The compiler already has all the necessary machinery for this (turbofish parsing, dimension inference, nat equation solving, index substitution). The difference is that these are applied to module instantiation rather than function evaluation.

### Call-Site Syntax

Modules are instantiated using function-call syntax with **named arguments**:

```gcl
node v: Velocity = orbital_velocity(gm: GM_EARTH, r: R_EARTH + @alt);
node mid: Length = lerp(a: @x, b: @y, t: 0.5);
node I3: Dimensionless[3, 3] = eye<3>();
```

Named arguments are required (not positional). This aligns with Graphcal's explicitness philosophy — readers can understand what each argument means without looking up the module definition.

When a module call appears in expression position, the compiler extracts the `result` node as the value. This is a convention: single-output modules must have a node named `result`.

Turbofish syntax works identically to current function generics:

```gcl
module eye<N: Nat> {
    node result: Dimensionless[N, N] =
        for i: range(N), j: range(N) { if i == j { 1.0 } else { 0.0 } };
}

node I3: Dimensionless[3, 3] = eye<3>();
```

### Multi-Output Modules

When a module exports multiple values, use `import` syntax with selective imports:

```gcl
module hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = sqrt(2.0 * @gm * @r2 / (@r1 * (@r1 + @r2))) - @v1;
    node dv2: Velocity = @v2 - sqrt(2.0 * @gm * @r1 / (@r2 * (@r1 + @r2)));
    node total_dv: Velocity = @dv1 + @dv2;
}

import hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) { dv1, dv2, total_dv };
```

The same `import` syntax used for file modules works for inline modules. The only difference is that the path is a bare identifier (the module name) rather than a string literal.

### Module Imports (Namespace Access)

Like file modules, inline modules can be imported as a namespace:

```gcl
import hohmann(gm: GM_EARTH, r1: R_EARTH + @alt1, r2: R_EARTH + @alt2) as h;

node total: Velocity = @h::total_dv;
node ratio: Dimensionless = @h::dv1 / @h::dv2;
```

### Expression-Position Desugaring

When a module call appears in expression position, the compiler desugars it:

```gcl
// User writes:
node v: Velocity = sqrt(abs(x: @a) + lerp(a: @b, b: @c, t: 0.5));

// Compiler sees (conceptually):
import abs(x: @a) { result as __abs_1 };
import lerp(a: @b, b: @c, t: 0.5) { result as __lerp_2 };
node v: Velocity = sqrt(@__abs_1 + @__lerp_2);
```

Each module call in expression position is lifted into a separate instantiation. The `result` node is extracted and wired into the expression. This is analogous to how SQL CTEs desugar subqueries.

### Nested Composition

Nested calls compose naturally because each call is lifted independently:

```gcl
// This works:
node v: Velocity = lerp(a: abs(x: @x), b: abs(x: @y), t: 0.5);

// Desugars to:
import abs(x: @x) { result as __abs_1 };
import abs(x: @y) { result as __abs_2 };
import lerp(a: @__abs_1, b: @__abs_2, t: 0.5) { result as __lerp_3 };
node v: Velocity = @__lerp_3;
```

### Intermediate Values

Where functions use `let` bindings, modules use `node` declarations:

```gcl
// Current function:
fn hohmann_dv(gm: GravParam, r1: Length, r2: Length) -> TransferResult {
    let v1 = sqrt(gm / r1);
    let v2 = sqrt(gm / r2);
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}

// As a module:
module hohmann {
    param gm: GravParam;
    param r1: Length;
    param r2: Length;
    node v1: Velocity = sqrt(@gm / @r1);
    node v2: Velocity = sqrt(@gm / @r2);
    node dv1: Velocity = ...;
    node dv2: Velocity = ...;
    node total_dv: Velocity = @dv1 + @dv2;
}
```

Intermediate values (`v1`, `v2`) become private nodes. They require type annotations, which is consistent with Graphcal's "explicit types everywhere" philosophy. The `let` keyword and type-inference-in-function-bodies are eliminated.

### Visibility Within Modules

All declarations within a module are public by default (importable by the instantiator). Use `private` to hide internal helpers:

```gcl
module hohmann {
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

**Within a module body:** The `@` sigil references declarations within the module's own scope (its params, nodes, consts). A module is a self-contained DAG.

**Constants from enclosing scope:** Modules can reference `const` declarations from the enclosing file without explicit import. Constants are immutable and have no dependency on graph state, so this does not violate the module's self-containment.

```gcl
const R_EARTH: Length = 6371.0 km;
const GM_EARTH: GravParam = 3.986004418e5 km^3/s^2;

module circular_velocity {
    param alt: Length;
    node result: Velocity = sqrt(GM_EARTH / (R_EARTH + @alt));
}
```

Note: `const` references use bare names (no `@`), consistent with how constants are referenced everywhere else in the language. Only `param` and `node` references within the module use `@`.

**Params and nodes from enclosing scope:** Not accessible. A module's `@` references resolve only within the module. This ensures that the module's dependency graph is explicit in its parameter list.

### Relationship to File Modules

Every file is a module. Inline modules are modules defined within a file. The module system is unified:

| Aspect | File module | Inline module |
| --- | --- | --- |
| Declaration | Implicit (file existence) | `module name { ... }` |
| Path in `import` | String literal or bare module path | Bare identifier |
| Params | `param` declarations in the file | `param` declarations in the block |
| Generics | Injectable indexes only | Full generics (`Dim`, `Index`, `Nat`) |
| Multi-output | Yes (multiple exports) | Yes (multiple exports) |
| Expression-position | No | Yes (via `result` convention) |

**Unification opportunity:** File modules could eventually support the same generic syntax as inline modules. A file with `<D: Dim>` in a header would be a dimension-generic file module. This is a natural extension but not required for the initial design.

### Built-in Functions

Built-in functions (`sqrt`, `sin`, `cos`, `ln`, `exp`, `abs`, `min`, `max`, `floor`, `ceil`, `round`, `atan2`) and aggregations (`sum`, `count`, `mean`) remain as functions. They are primitive operations — atoms of computation, like `+` and `*`. They are not user-defined and do not participate in the DAG as separate nodes.

The "everything is a DAG" principle applies to **user-defined** reusable computation. Built-in functions are leaf-level operations within node expressions.

### What Gets Removed

| Current feature | Replacement |
| --- | --- |
| `fn` keyword | `module` block |
| `let` bindings in function bodies | `node` declarations in module body |
| `@` prohibition in fn bodies | Not needed (modules use `@` by design) |
| Function registry (`FunctionRegistry`) | Module registry |
| Recursion detection on functions | Recursion detection on module instantiation graph |
| `FnGenericParam` / `FnGenericConstraint` | Module-level generics (same constraint kinds) |
| Function evaluation (`eval_builtin_or_user_fn`) | Module instantiation (sub-DAG creation + wiring) |

## Examples

### Simple Generic Function (Before/After)

```gcl
// BEFORE (current):
fn abs<D: Dim>(x: D) -> D = if x < 0 { -x } else { x };
node d: Length = abs(@a - @b);

// AFTER (proposed):
module abs<D: Dim> {
    param x: D;
    node result: D = if @x < 0 { -@x } else { @x };
}
node d: Length = abs(x: @a - @b);
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
module hohmann {
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

### Nat Generics with Arithmetic

```gcl
module drop_last<N: Nat, D: Dim> {
    param v: D[N + 1];
    node result: D[N] = for i: range(N) { @v[i] };
}

param data: Length[4] = [1.0 m, 2.0 m, 3.0 m, 4.0 m];
node trimmed: Length[3] = drop_last(v: @data);
// Compiler solves N + 1 = 4 → N = 3
```

### Nested Composition

```gcl
module clamp<D: Dim> {
    param value: D;
    param low: D;
    param high: D;
    node result: D = if @value < @low { @low }
                     else if @value > @high { @high }
                     else { @value };
}

node safe_thrust: Force = clamp(
    value: lerp(a: @min_thrust, b: @max_thrust, t: @throttle),
    low: @min_thrust,
    high: @max_thrust,
);
```

### Reuse Across Files

An inline module in one file can be imported by another file, just like any other declaration:

```gcl
// math_utils.gcl
module lerp<D: Dim> {
    param a: D;
    param b: D;
    param t: Dimensionless;
    node result: D = @a + (@b - @a) * @t;
}
```

```gcl
// main.gcl
import "./math_utils.gcl" { lerp };
node mid: Length = lerp(a: @x, b: @y, t: 0.5);
```

## Open Questions

### Syntax

1. **Named vs. positional arguments?** This design requires named arguments for clarity and explicitness. Should positional arguments also be supported as a convenience for single-param modules? e.g., `abs(@x)` instead of `abs(x: @x)`.

2. **`result` convention vs. explicit output annotation?** The current design uses a `result` node name convention for expression-position usage. An alternative is explicit output annotation on the module signature:

    ```gcl
    // Option A: Convention (current design)
    module abs<D: Dim> {
        param x: D;
        node result: D = if @x < 0 { -@x } else { @x };
    }

    // Option B: Explicit output annotation
    module abs<D: Dim>(x: D) -> D {
        node result: D = if @x < 0 { -@x } else { @x };
    }
    ```

    Option B makes the "function-like" intent more visible but introduces a special syntax that only applies to single-output modules.

3. **Should `module` blocks support `import` of other modules (file or inline)?** This enables module composition at the definition level, not just at the call site.

4. **Nesting?** Can a module contain another module definition? If so, is the inner module scoped to the outer module, or visible to the enclosing file?

### Scoping

5. **Constant visibility:** The current design allows modules to see enclosing `const` declarations. Should they also see enclosing `dimension`, `unit`, and `type` declarations? These are also immutable and stateless. (Likely yes — they are part of the type vocabulary, not graph state.)

6. **Enclosing module access:** If modules can be nested, can inner modules see the outer module's `const` declarations? What about the outer module's `param` and `node` declarations? (Proposal: inner modules see `const`/`dimension`/`unit`/`type` from all enclosing scopes, but never `param`/`node`.)

### Semantics

7. **Instantiation multiplicity:** If the same module is called twice with the same arguments, does the compiler create one or two sub-DAGs? (Proposal: always create separate instances. Deduplication is an optimization, not a semantic guarantee.)

8. **Assertions inside modules:** If a module contains `assert` declarations, are they evaluated at each instantiation? (Proposal: yes. Assertions are part of the module's contract.)

9. **Recursive module instantiation:** A module instantiating itself (directly or mutually) must be detected and rejected, same as current function recursion detection.

### Migration

10. **Coexistence period:** Should `fn` and `module` coexist during a transition period, or is this a clean break? Since Graphcal is unpublished, a clean break is feasible.

11. **Standard library:** Current built-in functions like `abs`, `clamp`, `lerp` — should these become built-in modules, or stay as built-in functions alongside the user-facing module system?

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Modules are sub-DAGs within the computation graph.
- **Scoping** ([08](./08-scoping.md)): `@` resolution within module bodies.
- **Namespace & Multi-File** ([09](./09-namespace.md)): Inline modules integrate with the import system.
- **Dimensions & Units** ([04](./04-dimensions-and-units.md)): `<D: Dim>` generics on modules.
- **Indexes** ([07](./07-indexes.md)): `<I: Index>` generics and injectable indexes.
- **Injectable Index Import** ([23](./23-injectable-index-import.md)): Module-level index parameterization is a precedent.
- **Pure Functions** ([12](./12-pure-functions.md)): This design supersedes doc 12.
- **Type System Stratification** ([20](./20-type-system-stratification.md)): Modules are declarations, not types.
