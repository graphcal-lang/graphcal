# Scoping and the `@` Sigil

> How variable references are resolved: graph scope vs local scope.

## Status

**Decision level:** Settled. The `@` sigil rule is a core design decision.

## Summary

In a multi-line node body, `@name` references the graph scope (params, nodes, consts) while bare `name` references the local scope (let bindings, function parameters). This distinction is visible at every usage site without IDE support, making PR diffs instantly readable.

## The Rule

```gcl
@name  -->  graph scope (param, node, const node, imported names)
name   -->  local scope (let bindings, function parameters)
```

## Example

```gcl
node fuel_budget: FuelBudget = {
    let r1 = @R_earth + @parking_alt;      // two graph deps, one local
    let r2 = @R_earth + @target_alt;

    let v1 = sqrt(@GM_earth / r1);         // one graph dep, one local
    let v2 = sqrt(@GM_earth / r2);

    let dv = (v1 - v2).abs();              // pure local computation

    let fuel = @dry_mass * (exp(dv / @v_exhaust) - 1);

    FuelBudget {
        delta_v: dv,
        fuel_mass: fuel,
        total_mass: @dry_mass + fuel,
    }
};
```

## PR Diff Readability

```diff
  node fuel_budget: FuelBudget = {
-     let engine = @propulsion.engines.rl10;
+     let engine = @propulsion.engines.raptor;
      let v_ex = engine.isp * @G0;
```

Instantly readable: the engine dependency changed from `rl10` to `raptor`. `engine` is local, `@propulsion.engines.raptor` and `@G0` are graph-level.

## Compiler Semantics

- `@foo` is resolved in graph scope (current file, then imports, then prelude).
- Bare `foo` is resolved in local scope only.
- No shadowing: a local `let G0 = ...` does **not** shadow `@G0`.
- The compiler can trivially extract the dependency set of a node by scanning for `@` references.

## Functions and `@`

Inside `fn` bodies, `@` is a **compile error**. Functions are pure and take all inputs as parameters:

```gcl
fn bad(r: Length) -> Velocity {
    sqrt(@GM_earth / r)
//       ^^^^^^^^^ error[F001]: graph reference not allowed in function body
}
```

This structural rule enforces purity without needing a separate type-level effect system.

## Open Questions

- **Nested `@` access:** `@transfer.dv1` accesses field `dv1` on graph node `transfer`. What if `transfer` is a module? How is `@orbit.transfer.total_dv` parsed -- is it field access on `@orbit`, or a qualified reference to `orbit.transfer.total_dv`?
- **`@` in table expressions:** In `node maneuvers.fuel = row.delta_v / @v_exhaust`, `row` is a special binding. Should `row` require `@`? Currently it does not, which is consistent (it's local to the expression), but `row` is somewhat magical.
- **`@` in `scan` / `map` lambdas:** The lambda `|prev, row| { ... }` introduces local bindings. `@` should still work for graph references inside the lambda. This is consistent but should be specified.
- **Self-reference:** Can a node reference itself (e.g., `node x: f64 = @x + 1`)? This should be a compile error (cyclic dependency), but the error message should be clear.

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): `@` defines graph edges.
- **Syntax** ([02](./02-syntax-design.md)): The `@` prefix is a syntactic element.
- **Namespace** ([09](./09-namespace.md)): `@` resolution follows import rules.
- **Pure Functions** ([12](./12-pure-functions.md)): `@` is prohibited in `fn` bodies.
