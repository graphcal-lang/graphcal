# Phase 3: Pure Functions

> `fn` keyword for reusable computation, enforced pure by the `@` prohibition.

## Goal

Prove that calculation logic can be factored into reusable functions.
Functions are not graph nodes -- they are computation templates that
take all inputs as parameters. Purity is enforced structurally:
`@` is a compile error inside `fn` bodies.

## Prerequisites

Phase 2 (Structs) must be complete. Functions operate on dimensioned
values (Phase 1) and can accept/return structs (Phase 2).

## Design Decisions to Lock

### From [12-pure-functions](../12-pure-functions.md)

- [ ] **Two forms confirmed:**
      Block: `fn name(params) -> Type { body }`
      Short: `fn name(params) -> Type = expr;`
      Block form has no trailing `;`. Short form has trailing `;`.
- [ ] **`@` prohibition:** `@` inside `fn` body is a compile error `[F001]`. Confirm
      the error message and suggested fix ("pass as a parameter instead").
- [ ] **Return type:** Required or optional? The "Language for Agents" insight
      recommends explicit return types. Decide: required on `fn`, optional on `let`.
- [ ] **Dimension generics:** `<D: Dim>` syntax. `D` can appear in parameter types,
      return type, and body. How does the compiler instantiate generics --
      monomorphization or erased?
- [ ] **Multiple generic parameters:** `<A: Dim, B: Dim>` for functions like
      `fn convert<A: Dim, B: Dim>(a: A, b: B) -> A * B`?
- [ ] **Non-dimension generics:** `<T>` without constraint. Needed for
      `fn identity<T>(x: T) -> T`? Or defer unconstrained generics?
- [ ] **Recursion:** Explicitly forbidden. Compile error if a function calls itself
      (directly or indirectly). Defer `rec fn` to post-MVP.
- [ ] **Higher-order functions:** Explicitly not supported. Functions cannot be
      passed as arguments. Lambda syntax only exists in `scan`/`map`.
- [ ] **Overloading:** Not supported. Use generics instead.
- [ ] **Zero-argument functions:** Warning suggesting `const` instead?
- [ ] **Local `fn` inside node bodies:** Allowed or forbidden? Recommendation: forbidden
      for simplicity. All `fn` declarations are top-level.

### Standard Library Functions

- [ ] **Built-in math functions to formalize as `fn`:** In Phase 0 these were built-in.
      Now they should be expressible in the `fn` syntax (even if still implemented
      as compiler intrinsics):
      `sqrt`, `exp`, `ln`, `log2`, `log10`, `abs`,
      `sin`, `cos`, `tan`, `asin`, `acos`, `atan2`,
      `min`, `max`, `clamp`, `floor`, `ceil`, `round`.
- [ ] **Which are generic over dimensions?** `abs<D: Dim>(x: D) -> D`,
      `clamp<D: Dim>(val: D, lo: D, hi: D) -> D`,
      `min<D: Dim>(a: D, b: D) -> D`, `max<D: Dim>(a: D, b: D) -> D`.
      Others require dimensionless: `exp(x: f64) -> f64`, `sin(x: Angle) -> f64`.

### From [08-scoping](../08-scoping.md) (functions)

- [ ] **Function calls use bare names:** `orbital_velocity(gm, r)`, not `@orbital_velocity`.
      Functions are not graph nodes and not referenced with `@`. Confirm.
- [ ] **Function name resolution:** In Phase 3 (single-file), functions are resolved
      in file scope. Must be forward-compatible with Phase 4 imports.

## Syntax Supported in Phase 3

Everything from Phase 2, plus:

```ebnf
// Function declaration
FnDecl       = "fn" IDENT GenericParams? "(" ParamList ")" "->" TypeExpr FnBody
FnBody       = "=" Expr ";"              // short form
             | "{" Statement* Expr "}"   // block form (no trailing ;)
GenericParams = "<" GenericParam ("," GenericParam)* ">"
GenericParam  = IDENT (":" "Dim")?
ParamList    = (FnParam ("," FnParam)*)? ","?
FnParam      = IDENT ":" TypeExpr

// Function call (in expressions)
FnCall       = IDENT ("<" TypeExpr ("," TypeExpr)* ">")? "(" ArgList ")"
ArgList      = (Expr ("," Expr)*)? ","?
```

Note: Generic type arguments at call sites (turbofish syntax) are usually
inferred and rarely written explicitly.

## Implementation Scope

| Component | Description |
| --- | --- |
| **Function registry** | Store function signatures, generic parameters |
| **`@` prohibition checker** | Scan `fn` bodies for `@` references, emit `[F001]` |
| **Generic instantiation** | Resolve `<D: Dim>` at call sites via dimension inference |
| **Function evaluator** | Evaluate function body with argument substitution |
| **Recursion detector** | Detect direct and indirect recursion, emit error |

## Out of Scope

- Higher-order functions, closures
- Recursion
- Overloading
- Functions as values
- Multi-file (function import via `use`)

## Milestone Test

```gcl
fn orbital_velocity(gm: Length^3 / Time^2, r: Length) -> Velocity {
    sqrt(gm / r)
}

fn hohmann_dv(gm: Length^3 / Time^2, r1: Length, r2: Length) -> TransferResult {
    let v1 = orbital_velocity(gm, r1);
    let v2 = orbital_velocity(gm, r2);
    let dv1 = sqrt(2.0 * gm * r2 / (r1 * (r1 + r2))) - v1;
    let dv2 = v2 - sqrt(2.0 * gm * r1 / (r2 * (r1 + r2)));
    TransferResult { dv1, dv2, total_dv: dv1 + dv2 }
}

fn clamp<D: Dim>(value: D, low: D, high: D) -> D = {
    if value < low { low }
    else if value > high { high }
    else { value }
};

node transfer = hohmann_dv(@GM_earth, @R_earth + @parking_alt, @R_earth + @target_alt);
node clamped_dv: Velocity = clamp(@transfer.total_dv, 0.0 m/s, 10.0 km/s);
```

### Error cases that must work

```gcl
// error: @ in function body
fn bad(r: Length) -> Velocity {
    sqrt(@GM_earth / r)
//       ^^^^^^^^^ error[F001]: graph reference not allowed in function body
//                 help: pass `GM_earth` as a parameter instead
}

// error: dimension mismatch in generic call
node x = clamp(100 km, 0.0 s, 200.0 km);
//  error: arguments to `clamp` must have matching dimensions
//         got Length, Time, Length
```

## Open Questions

- [ ] Should the compiler print which generic type was inferred?
      E.g., `clamp<Length>(...)` in verbose/debug mode.
- [ ] Should there be a `Scalar` constraint alongside `Dim` for "dimensionless numeric"?
- [ ] Can functions return `f64` (dimensionless) explicitly, or must they always
      use a dimension type?
