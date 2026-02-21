# Assertions and Testing

> `assert` declarations for regression testing and invariant checking.
> `#[...]` attribute system for declaration metadata. `#[assumes(...)]`
> for documenting engineering assumptions that break circular dependencies.

## Status

**Decision level:** Design in progress. Core approach (assert as declaration kind, attribute system) is agreed. Detailed syntax for tolerance expressions and attribute grammar needs finalization.

## Summary

Graphcal adds testing and validation through three features:

1. **`assert` declaration kind** — a new top-level declaration (alongside `param`, `node`, `const`) that checks a boolean condition or value tolerance after evaluation.
2. **`#[...]` attribute system** — a general-purpose metadata annotation on declarations, using the `#` sigil (since `//` is used for comments and `@` for graph references).
3. **`#[assumes(...)]` attribute** — documents that a node's computation relies on an engineering assumption expressed as an `assert`, without creating a graph dependency. This enables engineers to break circular dependencies while maintaining traceability.

## Motivation

### Why in-language assertions?

The Phase 6 design originally placed assertions in `.scenario` YAML files:

```yaml
assertions:
  fuel_mass: "2847 kg +/- 1 kg"
```

This approach has drawbacks:
- Assertions in YAML don't get type checking, dimension checking, or LSP support.
- Tolerance syntax must be designed in two places (YAML and `.gcl`).
- Engineers must context-switch between two languages.

By making `assert` a declaration kind, assertions participate in the full toolchain: the parser, type checker, dimension checker, formatter, and LSP all understand them natively. This aligns with Graphcal's safety-first philosophy — untested code is unsafe code, so testing should be as well-supported as computation.

### Why `#[assumes(...)]`?

Engineering calculations frequently involve assumptions that would create circular dependencies if modeled as graph edges. For example:

```gcl
// This creates a cycle:
//   safety_factor → wall_thickness → pressure → safety_factor
node safety_factor: Dimensionless = if @pressure < 10.0 MPa { 1.5 } else { 2.0 };
node wall_thickness: Length = @safety_factor * @base_thickness;
node pressure: Pressure = compute_pressure(@wall_thickness);
```

The engineering practice is to assume a value, compute forward, then verify the assumption holds. `#[assumes(...)]` documents this pattern: which nodes were designed under which assumptions, and what needs revisiting if an assumption is violated.

## Design Decisions

### Assert Declaration

- [ ] **`assert` as a declaration kind:** `assert name = <expr>;` where the body must evaluate to `Bool`. No type annotation (it's always `Bool`).

  ```gcl
  assert mass_positive = @mass > 0.0 kg;
  ```

- [ ] **Tolerance syntax:** `assert name = <expr> ~= <expr> +/- <expr>;` for approximate equality. The `~=` operator is only valid in `assert` bodies — it is not a general-purpose expression operator.

  ```gcl
  // Absolute tolerance
  assert fuel_budget = @fuel_mass ~= 2847.0 kg +/- 1.0 kg;

  // Relative tolerance
  assert efficiency = @eta ~= 0.85 +/- 5%;
  ```

  The `~=` form is syntactic sugar. `a ~= b +/- tol` is semantically equivalent to `abs(a - b) <= tol`, but produces richer failure diagnostics (showing actual value, expected value, tolerance, and delta).

- [ ] **Assertions are leaf nodes:** Assert declarations may reference `@` graph values but **no other declaration may reference an assert**. This means assertions are always evaluated last, after the entire graph is computed. They do not participate in the DAG — they are post-evaluation checks.

- [ ] **Assertion naming:** Assert names follow `lower_snake_case`, same as `param` and `node`.

- [ ] **Assertions in any file:** Assertions can appear in any `.gcl` file — the main computation file, a dedicated test file, or spread across multiple files. The convention of `test.gcl` as an entry point for tests is encouraged but not enforced.

- [ ] **Assertions cannot be imported:** Assert declarations are local to the file graph that contains them. `import` cannot import assertions from other files. This prevents assertions from leaking across project boundaries. (Open question: revisit if needed.)

### Attribute System

- [ ] **`#[name]` and `#[name(...)]` syntax:** Attributes are prefixed with `#` and enclosed in square brackets. They appear on the line(s) before a declaration.

  ```gcl
  #[lazy]
  node expensive: Report = heavy_computation(@all_data);

  #[assumes(pressure_safe, temp_bounded)]
  node safety_factor: Dimensionless = 1.5;
  ```

- [ ] **Multiple attributes:** A declaration may have multiple attributes, each on its own line.

  ```gcl
  #[lazy]
  #[assumes(pressure_safe)]
  node expensive_check: Report = heavy_computation(@data);
  ```

- [ ] **Attribute arguments:** Attribute arguments are comma-separated identifiers or key-value pairs. The exact grammar depends on the attribute.

  ```ebnf
  Attribute     = "#[" AttrName AttrArgs? "]"
  AttrName      = IDENT
  AttrArgs      = "(" AttrArgList ")"
  AttrArgList   = AttrArg ("," AttrArg)* ","?
  AttrArg       = IDENT                          // positional (e.g., assumes)
                | IDENT "=" Literal              // key-value (future use)
  ```

- [ ] **Unknown attributes are errors:** The compiler rejects unrecognized attribute names. This prevents silent typos and ensures forward compatibility is explicit.

- [ ] **AST representation:** Each declaration AST node gains a `Vec<Attribute>` field. Attributes are parsed before the declaration keyword and attached to the declaration node.

### `#[assumes(...)]` Attribute

- [ ] **Semantics:** `#[assumes(assertion_name)]` documents that the annotated node's computation is valid only if the named assertion holds. It does NOT create a graph edge.

  ```gcl
  assert pressure_safe = @pressure < 10.0 MPa;

  #[assumes(pressure_safe)]
  node safety_factor: Dimensionless = 1.5;
  ```

- [ ] **Name resolution:** The argument(s) to `#[assumes(...)]` must refer to `assert` declarations. Referencing a `node`, `param`, `const`, or nonexistent name is a compile error.

- [ ] **Multiple assumptions:** A single `#[assumes(...)]` can list multiple assertions:

  ```gcl
  #[assumes(pressure_safe, temp_bounded)]
  node safety_factor: Dimensionless = 1.5;
  ```

- [ ] **Failure reporting:** When an assumed assertion fails, the diagnostic lists all nodes that assume it:

  ```
  Assertions:
    pressure_safe:  FAIL  (pressure = 12.3 MPa, expected < 10.0 MPa)

    ⚠ The following nodes assume `pressure_safe` and may be invalid:
      - safety_factor (main.gcl:8)
      - material_grade (main.gcl:15)
  ```

- [ ] **Valid on `node` and `param`:** `#[assumes(...)]` is valid on `node` and `param` declarations. It is an error on `const` (constants don't depend on runtime values), `assert` (circular assumption), `fn` (functions are pure and don't reference the graph), and type/dimension/unit/index declarations.

### `#[lazy]` Attribute

- [ ] **Semantics:** `#[lazy]` marks a node for lazy evaluation — it is only computed when its value is requested, not eagerly during graph evaluation.

  ```gcl
  #[lazy]
  node detailed_report: Report = expensive_analysis(@all_data);
  ```

- [ ] **Valid on `node` only:** `param` values must always be available (they're inputs). `const` values are compile-time. Only `node` evaluation can be deferred.

### CLI Changes

- [ ] **Rename `check` to `typecheck`:** The current `check` command does parse + typecheck without evaluation. The name `typecheck` is more precise.

  ```
  graphcal typecheck <path>     — parse + typecheck only
  ```

- [ ] **`eval` checks assertions by default:** When the graph contains `assert` declarations, `eval` checks them after evaluation. A failed assertion produces diagnostic output and a non-zero exit code.

  ```
  graphcal eval <path>           — parse + typecheck + eval + assert
  ```

- [ ] **`--no-assert` flag:** Opt out of assertion checking during eval. The graph is evaluated but assertions are not checked and do not affect the exit code.

  ```
  graphcal eval <path> --no-assert   — eval without assertion checking
  ```

- [ ] **Exit codes:**

  | Code | Meaning |
  | --- | --- |
  | 0 | Success, all assertions pass |
  | 1 | Assertion failure or evaluation error |
  | 2 | Compile error (parse or typecheck) |

## Syntax Supported

Everything from Phase 5, plus:

```ebnf
// Attribute (before any declaration)
Attribute     = "#[" IDENT ( "(" IdentList ")" )? "]"
IdentList     = IDENT ("," IDENT)* ","?

// Assert declaration
AssertDecl    = "assert" LOWER_IDENT "=" AssertBody ";"
AssertBody    = Expr                                    // must evaluate to Bool
              | Expr "~=" Expr "+/-" Expr               // absolute tolerance
              | Expr "~=" Expr "+/-" NUMBER "%"          // relative tolerance
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **Attribute parser** | Parse `#[name(...)]` before declarations, attach to AST nodes |
| **`assert` parser** | Parse `assert name = expr;` and `~=` tolerance syntax |
| **`assert` in name resolution** | Register assert declarations, validate they are not referenced by `@` |
| **`assert` in dimension checker** | Verify assert body is Bool (or `~=` operands have matching dimensions) |
| **`assert` evaluator** | Evaluate assert bodies post-graph-evaluation, report pass/fail |
| **`#[assumes]` resolver** | Validate `assumes` arguments refer to `assert` declarations |
| **`#[assumes]` reporter** | On assertion failure, list nodes that assume the failed assertion |
| **`#[lazy]` evaluator** | Skip lazy-annotated nodes during eager evaluation |
| **CLI `typecheck` rename** | Rename `check` subcommand to `typecheck` |
| **CLI `--no-assert` flag** | Add flag to `eval` to skip assertion checking |
| **Formatter update** | Format `assert` declarations and `#[...]` attributes |
| **LSP update** | Syntax highlighting, go-to-definition for assert names in `#[assumes]` |
| **Error codes** | `A0xx` prefix for assertion errors (A001 assertion failed, A002 assumed assertion failed) |

## Out of Scope

- Scenario files (`.scenario` YAML) — assertions are now in-language; scenario files remain purely for parameter overrides. See [Phase 6](./phases/phase-6-scenarios.md).
- Property-based testing (checking assertions across parameter ranges)
- Snapshot testing (comparing full output to saved baselines)
- Test discovery / test runner beyond `graphcal eval`
- `#[deprecated]`, `#[doc]`, or other attributes beyond `#[assumes]` and `#[lazy]`

## Milestone Test

```gcl
dimension Velocity = Length / Time;
dimension SpecificImpulse = Time;

const G0: Acceleration = 9.80665 m/s^2;
param dry_mass: Mass = 1200.0 kg;
param isp: SpecificImpulse = 320.0 s;
param delta_v: Velocity = 3000.0 m/s;

node v_exhaust: Velocity = @isp * G0;
node mass_ratio: Dimensionless = exp(@delta_v / @v_exhaust);
node fuel_mass: Mass = @dry_mass * (@mass_ratio - 1.0);
node pressure: Pressure = 8.5 MPa;

// Regression test: expected fuel mass for default params
assert fuel_budget = @fuel_mass ~= 1523.0 kg +/- 5.0 kg;

// Invariant: fuel mass must be positive
assert fuel_positive = @fuel_mass > 0.0 kg;

// Engineering assumption: pressure stays within safe range
assert pressure_safe = @pressure < 10.0 MPa;

// This design choice was made assuming pressure is safe
#[assumes(pressure_safe)]
node safety_factor: Dimensionless = 1.5;

node wall_thickness: Length = @safety_factor * 0.02 m;
```

```text
$ graphcal eval rocket.gcl
dry_mass        = 1200 kg
isp             = 320 s
delta_v         = 3000 m/s
v_exhaust       = 3138.128 m/s
mass_ratio      = 2.602
fuel_mass       = 1922.337 kg
pressure        = 8.5 MPa
safety_factor   = 1.5
wall_thickness  = 0.03 m

Assertions:
  fuel_budget:     FAIL  (1922.337 kg, expected 1523.0 kg +/- 5.0 kg, off by 399.337 kg)
  fuel_positive:   PASS
  pressure_safe:   PASS
```

```text
$ graphcal eval rocket.gcl --no-assert
dry_mass        = 1200 kg
...
wall_thickness  = 0.03 m
```

### Assumed assertion failure

When an assumed assertion fails, the output additionally shows affected nodes:

```text
$ graphcal eval rocket.gcl
...
Assertions:
  fuel_budget:     PASS
  fuel_positive:   PASS
  pressure_safe:   FAIL  (pressure = 12.3 MPa, expected < 10.0 MPa)

  ⚠ The following nodes assume `pressure_safe` and may be invalid:
    - safety_factor (rocket.gcl:24)
```

### Error cases that must work

```gcl
// error: referencing an assert with @
node bad = @fuel_budget;
//  error: `fuel_budget` is an assert declaration and cannot be referenced with @

// error: assert body is not Bool
assert bad_assert = @fuel_mass;
//  error: assert body must evaluate to Bool, got Mass

// error: #[assumes] references a non-assert
#[assumes(fuel_mass)]
node x: Dimensionless = 1.0;
//  error: `fuel_mass` is a node, not an assert. #[assumes] can only reference assert declarations.

// error: unknown attribute
#[unknown_attr]
node x: Dimensionless = 1.0;
//  error: unknown attribute `unknown_attr`

// error: dimension mismatch in ~= tolerance
assert bad_tol = @fuel_mass ~= 2847.0 m/s +/- 1.0 kg;
//  error: dimension mismatch in assertion: left side is Mass, expected value is Velocity
```

## Open Questions

- [ ] **Tolerance syntax details:** Is `+/-` parsed as a single token or three tokens (`+`, `/`, `-`)? Recommendation: single token `+/-` to avoid ambiguity.
- [ ] **Relative tolerance base:** For `a ~= b +/- 5%`, is the 5% relative to `b` (the expected value) or `a` (the actual value)? Recommendation: relative to `b` (the expected value), which is the convention in engineering.
- [ ] **Assertion output verbosity:** Should passing assertions be shown by default, or only failures? Recommendation: show all assertions for visibility, with a `--quiet` flag to show only failures.
- [ ] **`~=` and `+/-` as keywords or operators:** Should `~=` be a keyword-level construct in assert bodies, or an operator in the expression grammar? Recommendation: keyword-level, restricted to assert bodies, to prevent misuse in computations.
- [ ] **Assertion ordering:** Are assertions evaluated/reported in declaration order? Recommendation: yes, for deterministic output.
- [ ] **Cross-file `#[assumes]`:** Can `#[assumes(x)]` reference an assert in a different file? This requires assertions to be importable, which is currently out of scope. Recommendation: defer, require same-file-graph for now.

## Prior Art

- **Rust `#[test]`:** Tests as first-class constructs discovered by the toolchain. Graphcal's `assert` achieves similar first-class status as a declaration kind.
- **Rust attributes (`#[...]`):** General-purpose metadata system. Graphcal adopts the same syntax with `#` instead of Rust's `#` (no collision since Graphcal uses `//` for comments).
- **Gleam tests:** In-language test declarations run by the standard toolchain.
- **Engineering design reviews:** The `#[assumes(...)]` pattern mirrors the "assumptions" section of engineering design documents, where engineers list what must be true for their analysis to be valid.
- **Design Assurance:** In aerospace and safety-critical engineering, assumptions must be explicitly documented and verified. `#[assumes]` provides machine-checkable assumption tracking.

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Assertions are evaluated after the graph, not as part of it.
- **Syntax Design** ([02](./02-syntax-design.md)): New `assert` keyword, `#[...]` attribute syntax, `~=` and `+/-` tokens.
- **Error Messages** ([17](./17-error-messages.md)): `A0xx` error codes for assertion failures. Rich failure diagnostics for `~=` tolerance assertions.
- **Git Workflow** ([16](./16-git-workflow.md)): Assertions replace the assertion section of `.scenario` files.
- **Phase 6** ([phases/phase-6-scenarios.md](./phases/phase-6-scenarios.md)): Scenarios keep parameter overrides; assertions move to in-language `assert`.
