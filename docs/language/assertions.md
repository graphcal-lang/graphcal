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

- Assert names conventionally use `lower_snake_case`, like `param` and `node`.
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
index Stage = { First, Second, Third };

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

This always means "within 10% of 0.20" (i.e., the range \[0.18, 0.22\]),
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
#[name]                                     // no arguments
#[name(arg1)]                               // one argument
#[name(arg1, arg2, arg3)]                   // multiple arguments
#[name(Index.Variant)]                     // qualified path argument
#[name((Idx.A, Idx.B), (Idx.C, Idx.D))] // tuple key arguments
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
  `const node`, or nonexistent name is a compile error (A005).
- Valid on `node` and `param` declarations. Using `#[assumes]` on `const node` is an
  error (A006) because constants do not depend on runtime values.
- Multiple assertions can be listed: `#[assumes(a, b, c)]`.
- Cross-file: you can import an assert via `import` and reference it in
  `#[assumes]`.

### `#[expected_fail]`

The `#[expected_fail]` attribute marks an assertion that is expected to fail.
This is useful for documenting known failures in engineering calculations
without causing the overall evaluation to fail.

A failing assertion marked `#[expected_fail]` is treated as a pass.
A passing assertion marked `#[expected_fail]` is treated as a failure
("unexpected pass"), since the known issue may have been resolved and the
attribute should be removed.

```
// This assertion fails (10 > 20 is false), but it's a known issue
#[expected_fail]
assert x_greater = @x > @y;
```

#### Constraints

- Valid only on `assert` declarations. Using `#[expected_fail]` on `param`,
  `node`, `const node`, etc. is a compile error (A008).
- Evaluation errors (e.g., division by zero) are never inverted -- they remain
  errors regardless of `#[expected_fail]`.
- `#[expected_fail]` without arguments is valid only on scalar assertions.
  Indexed assertions must list the exact expected-fail keys (A011).
- Per-variant keys are valid only on indexed assertions (A010).
- Each expected-fail key must be unique (A012).
- Single-index keys must belong to the assertion's index. Multi-index tuple
  keys must include every axis in the assertion's axis order (A013, A014).

#### Blanket Form

When used without arguments, the entire assertion is expected to fail:

```
#[expected_fail]
assert known_issue = @actual == @expected;
```

#### Per-Variant Form (Single Index)

For indexed assertions, specific index variants can be marked as expected
failures while other variants must still pass:

```
index Mode = { Normal, Eco, Boost };

#[expected_fail(Mode.Boost)]
assert power_ok = for m: Mode { @power_use[m] < @power_gen[m] };
```

Here, `Mode.Boost` is expected to fail (and is treated as a pass if it does),
while `Mode.Normal` and `Mode.Eco` must still pass normally.

#### Per-Tuple-Key Form (Multi Index)

For multi-indexed assertions, tuple keys identify specific index combinations:

```
index Mode = { Normal, Eco, Boost };
index Phase = { Launch, Cruise };

#[expected_fail((Mode.Normal, Phase.Cruise), (Mode.Boost, Phase.Launch))]
assert within_limits = for m: Mode, p: Phase { @actual[m, p] < @threshold[m, p] };
```

### `#[lazy]`

Recognized by the parser but not yet implemented. Will mark a node for lazy
evaluation (computed only when requested, not eagerly during graph evaluation).

## Assertions in Multi-File Projects

When a file is imported (either selectively or as a module), **all assertions
in that file are automatically evaluated**. You do not need to explicitly
import assertions for them to be checked -- they run as part of the imported
file's evaluation.

```
// checks.gcl
param limit: Dimensionless = 100.0;
assert limit_positive = @limit > 0.0;
```

```
// main.gcl
import checks.{limit};
// limit_positive is automatically evaluated and reported,
// even though it was not listed in the import braces.
```

This applies transitively: if `a.gcl` imports `b.gcl`, which imports `c.gcl`,
then assertions in all three files are evaluated and reported.

In diamond imports (where two files import the same dependency), the shared
dependency is evaluated once and its assertions are reported once.

### Using `#[assumes]` with imported assertions

To reference an imported assertion in `#[assumes(...)]`, you must explicitly
import it by name:

```
// main.gcl
import checks.{limit, limit_positive};

#[assumes(limit_positive)]
node ratio: Dimensionless = @limit / 2.0;
```

## Error Codes

| Code | Description |
|------|-------------|
| A001 | Assertion failure (LSP diagnostic) |
| A002 | Assumed assertion failed (CLI, lists affected nodes) |
| A003 | Cannot reference assert with `@` sigil |
| A004 | Assert body must evaluate to `Bool` |
| A005 | Unknown assert in `#[assumes(...)]` |
| A006 | `#[assumes]` on invalid declaration kind (e.g., `const node`) |
| A007 | Unknown attribute name |
| A008 | `#[expected_fail]` on invalid declaration kind (not `assert`) |
| A009 | Invalid argument in `#[expected_fail(...)]` |
| A010 | `#[expected_fail(...)]` with variant args on non-indexed assertion |
| A011 | `#[expected_fail]` without keys on indexed assertion |
| A012 | Duplicate key in `#[expected_fail(...)]` |
| A013 | `#[expected_fail(...)]` key has the wrong index shape |
| A014 | `#[expected_fail(...)]` key uses the wrong assertion index |
