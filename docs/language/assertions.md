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

assert all_stages_ok = for stage: Stage {
    @thrust[stage] > @min_thrust
};
// If First and Third fail:
//   FAIL  (failed at Stage#First, Stage#Third)
```

Comparison operators require unindexed operands and never broadcast. Use an
explicit `for` comprehension, as above, to produce `Bool[I]` one element at a
time. Passing an indexed collection directly to a comparison operator is a
compile error (`D019`). The explicit spelling makes the assertion's axes and
element pairing visible.

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
- `tolerance` must be **non-negative**. A literal negative tolerance is a
  compile error (`A015`); a tolerance computed at runtime that turns out
  negative makes the assertion report an `ERROR`. Zero tolerance is legal and
  means exact-match semantics.

The check passes when `abs(actual - expected) <= tolerance`.

Examples:

```
// All three operands can be graph references
assert mass_check = @computed_mass ~= @expected_mass +/- @mass_tolerance;

// Or mix literals, constants, and references
assert velocity_ok = @v_final ~= 3000.0 m/s +/- 10.0 m/s;
```

#### Expressing Relative Tolerance

Tolerance assertions have absolute semantics only. Express a relative
percentage by calculating its absolute tolerance explicitly:

```gcl
assert efficiency = @eta ~= 0.85 +/- abs(0.85) * 0.05;
// Passes if eta is within [0.8075, 0.8925]

assert velocity_approx = @velocity ~= 49.5 m/s +/- abs(49.5 m/s) * 0.05;
// Passes if velocity is within [47.025, 51.975] m/s
```

The expression after `+/-` is a full ordinary expression. `%` is exclusively
the binary modulo operator; it has no assertion-specific trailing form.

#### Indexed Tolerance Assertions

Tolerance assertions have their own element-wise semantics; unlike ordinary
comparison operators, they accept indexed operands. The assertion's index
shape comes from `actual`; `expected` and `tolerance` are each either
unindexed (applied to every key) or indexed by exactly the same axes in the
same order (`D011` otherwise):

```
index Case = { A, B };

node actual: Length[Case] = { Case#A: 1.0 m, Case#B: 2.0 m };
node expected: Length[Case] = { Case#A: 1.0 m, Case#B: 2.5 m };
node tol: Length[Case] = { Case#A: 0.01 m, Case#B: 0.6 m };

// Unindexed tolerance applied to every key:
assert close = @actual ~= @expected +/- 0.1 m;
//   FAIL  (failed at Case#B (actual 2, expected 2.5 +/- 0.1, off by 0.5))

// Per-key tolerance over the same axes:
assert per_key = @actual ~= @expected +/- @tol;

// Derive per-key relative tolerances explicitly as absolute quantities:
node relative_tol: Length[Case] = for case: Case {
    abs(@expected[case]) * 0.25
};
assert relative = @actual ~= @expected +/- @relative_tol;
```

Each failing key is reported with its own actual/expected/delta detail.
Per-variant `#[expected_fail(Case#B)]` works on indexed tolerance assertions
exactly as on indexed boolean ones.

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
#[name(Index#Variant)]                     // qualified path argument
#[name((Idx#A, Idx#B), (Idx#C, Idx#D))] // tuple key arguments
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
- Each `#[assumes(...)]` must contain at least one assertion name (A020).
- Multiple distinct assertions can be listed once: `#[assumes(a, b, c)]`.
  Repeating a name in the list is an error (A021), and stacking a second
  `#[assumes(...)]` on the same declaration is an error (A019).
- Cross-file assertions must be selected from an explicit `include` instance;
  the resulting instance-local alias may then be referenced in `#[assumes]`.
  A pure `import` cannot expose an assertion (`M024`).

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
- `#[expected_fail]` without arguments is valid only on unindexed assertions.
  Indexed assertions must list the exact expected-fail keys (A011).
- Per-variant keys are valid only on indexed assertions (A010).
- Each assertion or selective include item accepts at most one
  `#[expected_fail]` attribute (A019). Put every per-key expectation in that
  single attribute; later attributes never override earlier metadata.
- Each expected-fail key within the attribute must be unique (A012).
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

#[expected_fail(Mode#Boost)]
assert power_ok = for m: Mode { @power_use[m] < @power_gen[m] };
```

Here, `Mode#Boost` is expected to fail (and is treated as a pass if it does),
while `Mode#Normal` and `Mode#Eco` must still pass normally.

#### Per-Tuple-Key Form (Multi Index)

For multi-indexed assertions, tuple keys identify specific index combinations:

```
index Mode = { Normal, Eco, Boost };
index Phase = { Launch, Cruise };

#[expected_fail((Mode#Normal, Phase#Cruise), (Mode#Boost, Phase#Launch))]
assert within_limits = for m: Mode, p: Phase { @actual[m, p] < @threshold[m, p] };
```

#### Structural Finite Axes (`#N` Keys)

Assertions indexed by `Fin(N)` axes use positional `#N` keys, matching the
slice-label syntax used by `table` expressions:

```
#[expected_fail(#1)]
assert steps_ok = for i: Fin(3) { @residual[i] < @tolerance };
```

In multi-axis tuple keys, each component uses its axis's key form — named
axes use `Index#Variant`, while `Fin` axes use `#N` — in assertion axis order:

```
index Mode = { Normal, Boost };

#[expected_fail((Mode#Boost, #2))]
assert grid_ok = for m: Mode, i: Fin(4) { @value[m, i] < @limit };
```

The position must be in bounds: `#N` on a `Fin(size)` axis requires
`N < size` (A016). A bare integer (`#[expected_fail(1)]`) is a parse error;
positional keys always include the `#` prefix.

### `#[lazy]`

Reserved syntax, but lazy evaluation is not implemented. Semantic checking
rejects `#[lazy]` on every declaration and selective include item (A023), with
or without arguments. The parser and formatter continue to recognize it so a
future implementation can define an exact target and argument shape before
carrying typed lazy metadata into the execution plan.

## Assertions in Multi-File Projects

An `import` loads a module blueprint for compile-time name resolution; it does
not create or evaluate a runtime instance. Consequently, importing a file does
not run its assertions, and assertions cannot be imported as values (`M024`).

Assertions run for each explicit `include` instance. This keeps their outcomes
attached to the same parameter bindings and runtime values that they validate.
An `#[expected_fail(Index#Variant)]` written on the included assertion resolves
`Index` in that assertion's defining module, and diagnostics point to that
module's source. An `#[expected_fail(...)]` written on an include brace item
instead resolves in the including module.

```
// checks.gcl
param limit: Dimensionless = 100.0;
pub assert limit_positive = @limit > 0.0;
```

```
// main.gcl
include checks(limit: 50.0)::{ limit, limit_positive };

#[assumes(limit_positive)]
node ratio: Dimensionless = @limit / 2.0;
```

Selecting an assertion in the include braces exposes its instance-local name
for `#[assumes(...)]`; the assertion is not referenced with `@`. Assertions
inside nested includes also run as part of each outer concrete instance. Two
separate includes therefore produce separate outcomes, even when their
bindings happen to be equal.

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
| A015 | Literal tolerance in `~=` assertion is negative |
| A016 | `#[expected_fail(...)]` Fin position is outside the indexed assertion axis |
| A017 | `#[hidden]` on an invalid declaration kind |
| A018 | `#[hidden]` on a non-plot include item |
| A019 | Repeated singleton `#[assumes]` or `#[expected_fail]` attribute |
| A020 | Empty `#[assumes]` argument list |
| A021 | Duplicate assertion name in `#[assumes(...)]` |
| A022 | Non-identifier argument in `#[assumes(...)]` |
| A023 | Reserved `#[lazy]` syntax is not supported |
