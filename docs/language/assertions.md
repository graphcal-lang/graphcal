---
icon: material/check-decagram
---

# Assertions and Attributes

Assertions are post-evaluation checks that verify invariants and expected values.
Attributes are metadata annotations on declarations. Together, they enable
in-language testing and engineering assumption tracking.

## Assert Declarations

An `assert` declaration checks a boolean condition after the entire computation
graph has been evaluated:

```
assert fuel_positive = @fuel_mass > 0.0 kg;
```

### Key Properties

- Assert names follow `lower_snake_case`, like `param` and `node`.
- Assert bodies can reference any `@param` or `@node`, plus constants.
- Assertions are **leaf nodes** -- no declaration can reference an assert with
  `@`. Attempting `@my_assert` is a compile error (A003).
- Assertions are evaluated **after** the full graph, in declaration order.
- A failed assertion produces a non-zero exit code.

### Boolean Assertions

The simplest form evaluates an expression that must produce `Bool`:

```
param pressure: Pressure = 8.5 MPa;
param max_pressure: Pressure = 10.0 MPa;

assert pressure_safe = @pressure < @max_pressure;
```

If the body evaluates to `false`, the assertion fails with
"assertion evaluated to false".

### Indexed Boolean Assertions

When the body evaluates to `Bool[SomeIndex]`, each variant is checked
individually. Failing variants are listed in the diagnostic:

```
index Stage = First, Second, Third;

param thrust: Force[Stage] = ...;
param min_thrust: Force = 100.0 kN;

assert all_stages_ok = @thrust > @min_thrust;
// If First and Third fail:
//   FAIL  (failed at Stage: First, Third)
```

### Tolerance Assertions

For approximate equality checks, use the `~=` and `+/-` syntax:

```
assert fuel_budget = @fuel_mass ~= 2847.0 kg +/- 5.0 kg;
```

This is semantically equivalent to `abs(@fuel_mass - 2847.0 kg) <= 5.0 kg` but
produces richer failure diagnostics showing the actual value, expected value,
tolerance, and delta.

#### Absolute Tolerance

```
assert name = <actual> ~= <expected> +/- <tolerance>;
```

All three operands are arbitrary expressions. They can reference `@param`,
`@node`, constants, call functions, and use arithmetic -- anything valid in a
`node` expression. The dimension rules are:

- `actual` and `expected` must have the **same dimension**.
- `tolerance` must also have the **same dimension** as `actual` and `expected`.

The check passes when `abs(actual - expected) <= tolerance`.

Examples:

```
// All three operands can be graph references
assert mass_check = @computed_mass ~= @expected_mass +/- @mass_tolerance;

// Or mix literals, constants, and references
assert velocity_ok = @v_final ~= 3000.0 m/s +/- 10.0 m/s;
```

#### Relative Tolerance

Append `%` after the tolerance value to use relative (percentage) tolerance:

```
assert name = <actual> ~= <expected> +/- <tolerance> %;
```

The dimension rules change for relative tolerance:

- `actual` and `expected` must have the **same dimension**.
- `tolerance` must be **dimensionless** (or an integer literal).

The check passes when `abs(actual - expected) <= abs(expected) * tolerance / 100`.

**The `%` is not a unit.** The `%` in tolerance assertions is special syntax,
not a built-in unit. It is only valid immediately after the tolerance expression
in a `~= ... +/-` context. You cannot write `5 %` as a unit literal elsewhere
in the language.

This means there is no ambiguity in expressions like:

```
assert rate_check = @rate ~= 0.20 +/- 10 %;
```

This always means "within 10% of 0.20" (i.e., the range [0.18, 0.22]),
because `%` is parsed as part of the tolerance syntax, not as a unit on the
number `10`.

Examples:

```
assert efficiency = @eta ~= 0.85 +/- 5 %;
// Passes if eta is within [0.8075, 0.8925]

assert velocity_approx = @velocity ~= 49.5 m/s +/- 5 %;
// Passes if velocity is within [47.025, 51.975] m/s
```

## Attributes

Attributes are metadata annotations written before a declaration using the
`#[name]` or `#[name(args)]` syntax:

```
#[assumes(pressure_safe)]
node safety_factor: Dimensionless = 1.5;
```

### Syntax

```
#[name]                       // no arguments
#[name(arg1)]                 // one argument
#[name(arg1, arg2, arg3)]    // multiple arguments
```

Multiple attributes can be stacked:

```
#[lazy]
#[assumes(pressure_safe)]
node expensive: Dimensionless = heavy_computation(@data);
```

Unknown attribute names are compile errors (A007).

### `#[assumes(...)]`

The `#[assumes(...)]` attribute documents that a declaration's value is valid
only if the named assertion(s) hold. It does **not** create a graph dependency.

```
assert pressure_safe = @pressure < 10.0 MPa;

#[assumes(pressure_safe)]
node safety_factor: Dimensionless = 1.5;
```

When `pressure_safe` fails, the diagnostic mentions that `safety_factor` may be
invalid:

```
Assertions:
  pressure_safe  FAIL  (assertion evaluated to false)
                       affected: safety_factor
```

#### Rules

- Arguments must reference `assert` declarations. Referencing a `param`, `node`,
  `const`, or nonexistent name is a compile error (A005).
- Valid on `node` and `param` declarations. Using `#[assumes]` on `const` is an
  error (A006) because constants do not depend on runtime values.
- Multiple assertions can be listed: `#[assumes(a, b, c)]`.
- Cross-file: you can import an assert via `import` and reference it in
  `#[assumes]`.

### `#[lazy]`

Recognized by the parser but not yet implemented. Will mark a node for lazy
evaluation (computed only when requested, not eagerly during graph evaluation).

## Assertions in Multi-File Projects

Assertions can be defined in any `.gcl` file and imported via `import`:

```
// checks.gcl
assert pressure_safe = @pressure < 10.0 MPa;
```

```
// main.gcl
import "./checks.gcl" { pressure_safe };

#[assumes(pressure_safe)]
node safety_factor: Dimensionless = 1.5;
```

## Error Codes

| Code | Description |
|------|-------------|
| A001 | Assertion failure (LSP diagnostic) |
| A002 | Assumed assertion failed (CLI, lists affected nodes) |
| A003 | Cannot reference assert with `@` sigil |
| A004 | Assert body must evaluate to `Bool` |
| A005 | Unknown assert in `#[assumes(...)]` |
| A006 | `#[assumes]` on invalid declaration kind (e.g., `const`) |
| A007 | Unknown attribute name |
