# Testing in Graphcal — Design Exploration

Status: exploration (no decision). Tracking issue: [#656](https://github.com/graphcal-lang/graphcal/issues/656).
Date: 2026-06-11.

This document maps the design space for user-facing testing in Graphcal:
what a "test" even means for a reactive calculation DAG, where tests
should live, what syntax (if any) they need, and how a `graphcal test`
command should behave. It ends with a recommended phasing, but every
section lists the alternatives that were considered and why.

## 1. What gap is testing filling?

Graphcal already has several verification mechanisms:

| Mechanism | When it runs | What it checks |
|---|---|---|
| `graphcal check` | on demand / LSP | static: parse, types, dimensions |
| Domain constraints `Mass(min: …, max: …)` | every eval | runtime value ranges |
| `assert` declarations | every eval, after the graph | invariants of the *current* model state |
| `#[expected_fail]` | every eval | known-bad assertions, with unexpected-pass detection |

What none of these can express: **evaluating the model under inputs
that are *not* the production inputs**. An `assert` checks the model as
configured — with default params or whatever `--set` supplied. It cannot
say:

- "When this `dag` is given *these specific* inputs, its output must be
  *this value* (within tolerance)." — scenario / regression testing.
- "When given *these invalid* inputs, evaluation must *fail*, and with
  *this* diagnostic." — negative testing, which matters for a language
  whose pitch is rejecting bad engineering inputs.
- "For *any* input in the declared domain, this invariant holds." —
  property testing.

So the unit of testing in Graphcal is not "a function returns the right
value" but **"a (sub-)DAG, instantiated with chosen bindings, produces
expected outputs / expected failures."** Everything below follows from
that framing.

A second, quieter gap: asserts run on *every* eval. Engineering models
accumulate slow or numerous checks; there is currently no way to write a
check that runs only on demand (in CI, say) without paying for it on
every interactive evaluation.

## 2. Units under test

1. **`dag` blocks.** The natural "function" of the language. Testing one
   means `include`-ing it with literal bindings and asserting on its
   outputs. Everything needed for this already exists *except* a place
   to put it that doesn't pollute the production graph.
2. **Library modules / entry files.** A file with required params is
   tested by binding those params per scenario — again an `include`
   (parameterized include of a file DAG) plus asserts.
3. **The diagnostics themselves.** "This misuse must not compile" /
   "this input must produce runtime error R0xx". This is exactly what
   the compiler's own fixture corpus (`tests/fixtures/{valid,
   runtime_error, invalid}/`) does internally; users building safety
   cases will want the same capability for their own libraries.

## 3. Design axes

- **A. Location** — tests in production files, in separate test files
  (`tests/` dir), or both.
- **B. Surface syntax** — a new `test` declaration, an attribute on
  existing declarations, or no syntax at all (pure CLI convention).
- **C. Pass/fail semantics** — what a test result is; how it relates to
  `assert`; isolation between tests.
- **D. Visibility** — whether tests can see non-`pub` items.
- **E. Negative tests** — expected runtime errors and expected compile
  errors.
- **F. Parameterization** — table-driven cases.
- **G. Snapshots** — golden-output comparison.
- **H. Property-based testing** — domain constraints as generators.
- **I. Tooling** — CLI shape, reporting, exit codes, LSP integration.

## 4. Candidate designs for the core (axes A–C)

### Design 1 — no language change: `tests/` directory convention

`graphcal test` discovers every entry-point `.gcl` under a `tests/`
directory (sibling of `src/` in a manifest package), evaluates each, and
reports the asserts as the test results.

```gcl
// tests/hohmann_test.gcl
import nasa.orbital.{ hohmann_transfer };

include hohmann_transfer(
    gm: 3.986004418e5 km^3/s^2, r1: 6571.0 km, r2: 42164.0 km,
).{ total_dv as dv_geo };

assert geo_transfer_dv = @dv_geo ~= 3.935 km/s +/- 1.0 %;
```

This is surprisingly strong because the existing machinery composes:

- Each `include` is a fresh instantiation (call-site identity), so one
  test file can probe many scenarios without interference.
- Tolerance asserts, indexed asserts, and `#[expected_fail]` all work
  unchanged.
- Fault isolation means one failing scenario poisons only its
  dependents and asserts, not the whole file.

Weaknesses:

- **No grouping.** Failures report per-assert; there is no named
  "scenario" unit bundling several asserts with the includes they
  check. Reports for a 30-scenario file become a flat assert list.
- **No negative tests.** A division-by-zero include is an evaluation
  error, not an expressible expectation.
- **Nothing co-located.** Tests can't sit next to the `dag` they test;
  unit-test-style locality is lost.

### Design 2 — a `test` declaration (recommended core)

```gcl
test geo_transfer_nominal {
    import nasa.orbital.{ hohmann_transfer };

    include hohmann_transfer(
        gm: 3.986004418e5 km^3/s^2, r1: 6571.0 km, r2: 42164.0 km,
    ).{ total_dv };

    assert dv_close = @total_dv ~= 3.935 km/s +/- 1.0 %;
}
```

Semantics, stated so that almost nothing is new:

- A `test` body is **exactly a `dag` body** (same declarations, same
  strict scope isolation, same `@` rules) with three deltas:
  1. It is **never part of the production graph**: not importable, not
     includable, not `pub`-able, ignored by `graphcal eval`.
  2. Only `graphcal test` instantiates it — each test is evaluated as
     its own anonymous entry-point DAG, fully isolated from every other
     test.
  3. Its **asserts are its pass criteria**: pass = evaluates without
     runtime error ∧ all asserts pass. A test containing no assert is a
     lint warning (it can only fail by erroring — sometimes useful, but
     usually a mistake).
- `graphcal check` still type/dimension-checks test bodies always —
  tests are never allowed to rot statically.
- Params inside a `test` body: a required param has nothing to bind it,
  so it is a compile error; params with defaults are allowed (they act
  as named scenario constants and read nicely).

Grammar cost is one production plus one contextual keyword:

```ebnf
test_decl = "test", IDENT, "{", { declaration }, "}";
```

`test` should be **contextual** (keyword only at declaration start when
followed by `IDENT {`), because `test` is a plausible user identifier
(`node test_mass` is safe either way, but `param test: …` should keep
working).

Location (axis A) becomes a non-issue: `test` declarations are legal in
any module of the package, so authors can co-locate unit tests with the
`dag` they exercise *and* put integration-style tests in dedicated
modules (e.g. a `tests` subtree under the package source dir, or a
sibling test package — see §8 Open questions for the visibility
consequences). Design 1's directory convention degenerates to "where
you happen to put files containing `test` declarations".

### Design 3 — attribute-driven: `#[test]` on `dag` or `assert`

Rejected. Two variants were considered:

- `#[test] dag foo { … }` — but a test is *not* a dag: dags are
  includable, parameterized, reusable; tests are none of those. Making
  one declaration kind mean two things depending on an attribute is
  exactly the "semantics by convention, not by type" pattern the
  project bans in its own compiler (`CLAUDE.md`); the same taste should
  apply to the language surface. Distinct concept ⇒ distinct
  declaration kind.
- `#[test] assert …` (an assert that only runs under `graphcal test`)
  — this blurs the invariant/scenario distinction that gives `assert`
  its meaning ("always checked"), and it still provides no way to bind
  scenario inputs. The on-demand-assert need is real but is better
  served by moving such checks into a `test` (possibly one that
  `include`s the production entry DAG with its defaults).

## 5. Negative tests (axis E)

### Expected runtime errors

An attribute on the `test` declaration inverts the error expectation:

```gcl
#[should_error]            // any runtime evaluation error ⇒ PASS
test zero_radius_rejected {
    import nasa.orbital.{ orbital_velocity };
    include orbital_velocity(gm: 3.986004418e5 km^3/s^2, r: 0.0 km).{ v };
}
```

Optionally with a code: `#[should_error(R012)]` pins *which* failure
(wrong code ⇒ FAIL with both codes shown). Like `#[expected_fail]`, a
test marked `#[should_error]` that evaluates cleanly fails with
"unexpected pass". Asserts inside a `#[should_error]` test are rejected
statically (they could never run meaningfully).

This mirrors `#[expected_fail]` closely enough to feel native, and the
error-code taxonomy already exists and is user-visible (A001…, M013,
P001…), so pinning codes is meaningful, not stringly.

### Expected compile errors

Harder, because the offending body must *not* survive type checking, so
it cannot live inside an otherwise-checked module. Options:

1. **Out of scope for in-language tests; directory convention instead.**
   A `tests/compile_fail/` directory of standalone `.gcl` files, each
   expected to fail `graphcal check`, with the expected code declared
   in a structured first-line annotation (e.g. `//! expect: M020`) or a
   manifest table. This is the trybuild / rustc-UI-test model and
   matches the project's own fixture corpus. Cheap, proven, and keeps
   ill-typed code quarantined at a file boundary.
2. **`#[should_not_compile(E…)] test` whose body is checked in
   isolation and required to fail.** Elegant on paper, but it forces
   the compiler to carry "this subtree is expected-ill-typed" state
   through every pass, and the formatter/LSP/tree-sitter all need to
   survive arbitrarily broken bodies. High cost, low payoff.

Option 1 is the clear winner if/when this is wanted at all; it is also
strictly additive later. Recommend deferring the whole sub-feature
behind a follow-up issue.

## 6. Table-driven tests (axis F)

No new syntax needed — indexes *are* the parameterization mechanism,
and the table/multi-decl sugar makes input+expected tables pleasant:

```gcl
test transfer_cases {
    import nasa.orbital.{ hohmann_transfer };

    index Case = { Leo, Meo, Geo };

    node r2: Length[Case], node expected: Velocity[Case] = table [Case, (_, _)] {
        :    _,          _;
        Leo: 7000.0 km,  3.972 km/s;
        Meo: 20200.0 km, 4.130 km/s;
        Geo: 42164.0 km, 3.935 km/s;
    };

    node dv: Velocity[Case] = for c: Case {
        @hohmann_transfer(gm: 3.986004418e5 km^3/s^2,
                          r1: 6571.0 km, r2: @r2[c]).total_dv
    };

    assert dv_matches = @dv ~= @expected +/- 1.0 %;
}
```

**Prerequisite found while writing this:** the docs define indexed
*boolean* asserts (per-variant pass/fail reporting), but the tolerance
form `actual ~= expected +/- tol` is not documented to broadcast over
indexed operands. Element-wise tolerance asserts over `T[Index]`
operands (reporting failing keys exactly like indexed boolean asserts,
and composing with per-variant `#[expected_fail]`) are the single
highest-leverage enabler for table-driven testing and are useful even
without any other part of this proposal. Filed as
[#809](https://github.com/graphcal-lang/graphcal/issues/809),
independent of #656.

## 7. The long tail (axes G, H)

### Snapshot / golden testing

A CLI-level feature, no grammar: `graphcal test --snapshot accept`
writes each test's (or entry point's) `--format json` output to a
checked-in `.snap.json`; later runs diff against it. It is Git-friendly
and great for "I refactored, prove nothing moved".

But it cuts against the house philosophy: a snapshot is an *implicit*
exact-equality assertion on floats, the kind of check the tolerance
syntax exists to replace. Bit-exact pinning is brittle across
platforms/libm versions; tolerance-aware snapshots reinvent `~=` with
the tolerance hidden in a sidecar file instead of explicit in source.
Verdict: defer; if added, scope it to *structural* output (plot specs,
table shapes, diagnostic rendering) rather than numeric values.

### Property-based testing

Domain constraints already define typed, dimension-checked input
domains:

```gcl
#[property(samples: 256)]
test velocity_decreases_with_radius {
    param r_lo: Length(min: 6500.0 km, max: 50000.0 km);
    // sampled: no default ⇒ the runner draws from the constraint range
    …
    assert monotone = @v_at_lo > @v_at_hi;
}
```

Semantics: in a `#[property]` test, required params *are* allowed, and
the runner samples them from their domain constraints (a required param
without constraints is a compile error there). Determinism matters for
a Git-friendly tool: fixed default seed, `--seed` to override, failing
sample printed as a ready-to-paste `--set` line (shrinking can come
later). This is a genuinely differentiating feature — generation is
unit-aware by construction — and it dovetails with the Monte
Carlo/Python-interop vision. But it is its own project; phase it last.

## 8. CLI, reporting, tooling (axis I)

```
graphcal test [PATHS]... [--filter <substr>] [--format text|json] [--seed <n>]
```

- **Discovery:** all `test` declarations in the package(s) rooted at
  PATHS (default: current directory), plus — open question below —
  entry-point asserts.
- **Result model:** `PASS` / `FAIL` / `ERROR` (unexpected evaluation
  error) / `XFAIL`-style outcomes for `#[should_error]`, with per-assert
  and per-index-key detail nested under each test, reusing the existing
  assertion diagnostics verbatim.
- **Exit codes:** mirror `eval`: 0 all pass, 1 any failure, 2 compile
  error / bad invocation.
- **`--format json`** for CI, shaped like the existing eval JSON.
- **LSP:** `test` declarations get document symbols for free; add a
  code-lens "▶ Run test" per declaration, publish failed-assert
  diagnostics into the test body after a run, and show inlay values for
  the test's nodes from the last run (the "live calculation sheet"
  experience, scoped to a test instantiation).
- **Ecosystem checklist** (per `CLAUDE.md`): grammar.ebnf, formatter,
  tree-sitter grammar, TextMate grammar, Zed/VS Code extensions all
  need the `test` keyword; docs need a `docs/language/testing.md` page
  and tutorial step.

## 9. Recommended phasing

1. **Phase 0 (enabler, independent):** indexed tolerance assertions
   (`T[I] ~= T[I] +/- tol`, per-key reporting). Useful today.
2. **Phase 1 (the core):** `test` declaration (Design 2) + `graphcal
   test` discovery/reporting + LSP code lens. This subsumes Design 1 —
   a `tests/` directory is then merely a layout convention.
3. **Phase 2:** `#[should_error(code?)]` negative tests; decide the
   open questions below.
4. **Phase 3 (separate proposals):** compile-fail corpus convention;
   property-based testing; (maybe) structural snapshots.

## 10. Open questions

Items 1, 2, 3, and 5 were decided on 2026-06-11; only item 4 (a
mechanical parser check) remains for implementation time.

1. **Visibility for tests** — DECIDED: strict-only for v1. A `test`
   body already starts from an empty scope (dag-style isolation), and
   its `import`/`include` follow normal visibility rules, so tests
   consume only `pub` surface and double as documentation of the
   public API ("explicit over implicit"). The pragmatic alternative —
   letting a test import private items from *its own* module,
   Rust-unit-test style, so helper dags can be tested without
   `pub`-leaking — is rejected for now; loosening later is
   backwards-compatible, tightening is not.
2. **Does `graphcal test` also run production asserts?** — DECIDED:
   yes. Each entry point is evaluated with default params and its
   asserts reported as a synthetic "evals" group, so CI is one
   command; `--tests-only` opts out.
3. **Plots/figures inside `test` bodies** — DECIDED: reject (lint
   error) for v1. Allowing them as a failure-debugging aid via
   `graphcal test --plot browser` can be revisited later.
4. **`test` as contextual vs reserved keyword** — contextual preferred
   (see §4); needs a parser-ambiguity check against `test` as a type
   alias-ish identifier followed by `{` in expression position.
5. **Naming** — DECIDED: `test` (matches mainstream toolchains and the
   CLI verb), not `scenario`/`case`.
