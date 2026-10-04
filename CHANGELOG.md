# Changelog

Release notes for Graphcal. Each release section becomes the body of its GitHub Release.
Add a `## <version>` section before running the release workflow.
Earlier releases are listed on [GitHub Releases](https://github.com/graphcal-lang/graphcal/releases).

## 0.0.1-alpha.33

This release finishes the internal refactor of the compiler and evaluator started in v0.0.1-alpha.32.
It also accepts `Dimensionless` as the identity term of dimension expressions, fixes several soundness and runtime bugs in rebound and included DAGs, and gives many diagnostics their own error codes and more accurate locations.

**Some existing Graphcal source programs may need to be modified.** The breaking changes reject programs that were previously accepted only because of a bug or a missing check. See [Breaking Changes](#breaking-changes).

### Breaking Changes

- An import inside a `dag` body whose module path does not resolve is now rejected with the same error as a file-root import (M017, M013, M002, or M008).
  Before, an unused one compiled silently and a used one was reported as N002.
  (_#1934_)
- A non-`const` `unit` declared over a defaulted bindable dimension, and a `(min: …, max: …)` bound on a value of such a dimension, are now rejected with V007, as node bodies already were.
  Before, a rebound instance kept the default dimension for such a unit, so `1.0 inst::qu` was typed as `Length` after `dim Q: Mass`, and its bounds compared values across dimensions, such as 4 kg against 10 m.
  (_#1937_)

### Newly Accepted Programs

- `Dimensionless` is now accepted as the identity term of dimension expressions.
  `Dimensionless / Time` denotes the same dimension as `Time^-1`, and `dim Ratio = Dimensionless;` is valid.
  This works in type annotations, `dim` and `unit` declarations, generic arguments, record fields, indexed types, and extern signatures.
  (_#1967_)

### Bug Fixes

- Using an instance's dynamic unit from the including DAG no longer fails at runtime with "undefined graph reference". (_#1937_)
- `graphcal eval` no longer crashes with internal error X001 when an included DAG has an `#[assumes]` whose assumer or assertion is not exposed to the root. (_#1939_)
- Errors raised while resolving modules are now rendered against the file that contains them. Before, a bad import or duplicate in a non-root file had its labels drawn over the root file's text. (_#1978_)
- V001 for a private item in a selective import now points at the item instead of the whole file. (_#1978_)

### Clearer Diagnostics

- Many compile-time diagnostics that were reported under the catch-all code E001 now carry the code of their family: I011–I043 (indexes), S011–S033 (structure), N019–N036 (names), M033 and M034 (module resolution), G008 and G009 (DAG recursion), A024 (attributes), and D037–D042 (time scales, unit scales, and constant expressions). Messages and labels are unchanged. (_#1933_)
- New codes replace E001 in other places: C008 for a domain bound integer too large for exact quantity comparison, M035 for a module path naming an unlocked dependency, and O005–O012 for external parameter binding errors. (_#1933_, _#1934_)
- Binding a map literal to a non-indexed value now reports O007 instead of I003. (_#1934_)
- Diagnostics for `--param` values and editor fields point into the value text (`<--param NAME>`) instead of the entry file. (_#1934_)
- Module-resolution diagnostics use Graphcal category words instead of Rust type names. An unknown applied generic type is now N019 instead of M033 "unknown StructTypeName". A module path naming an undeclared inline DAG is now G008 at the path instead of M033 spanning the root file. (_#1935_, _#1939_)
- An overflowing closed part after a name in an include binding now reports I038 instead of I001. (_#1937_)
- Wording fixes: V003 says "private index" instead of "private cat/range", and D001 says "has type Int" for a type operand instead of "has dimension Int". (_#1978_)
- A024 (arguments on `#[hidden]`) has a precise label and help text, and attribute errors are reported in attribute order. (_#1964_)
- Unavailability reasons name values by their source names:
  - An assertion exposed through an include reports `dependency failed: other::bad` instead of `un1.<include@154>.bad`. (_#1939_)
  - Declarations of invoked modules are named by the module path importers write, for example `pipeline.lib.d::pending` instead of `src.pipeline.lib.d.pending`. (_#1966_)

### Documentation

- The documentation site uses the Graphcal icon as its favicon. (_#1976_)
- The minimum supported Rust version for building from source is now Rust 1.95. The installation guides are updated. (_#1958_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated Rust crates: `insta` to v1.49.0, `libc` to v0.2.190, `tokio` to v1.53.2, and `wat` to v1.261.0. (_#1960_, _#1957_, _#1977_, _#1955_)
- Updated Zensical to v0.0.65 and `crate-ci/typos` to v1.50.3. (_#1927_, _#1956_)
- Fixed Cargo warnings from the deprecated `unsafe-code` lint key and the deprecated atomic `fetch_update` method. (_#1959_, _#1958_)

### Internal

- Completed the handle-redesign follow-up and the final sweep of the invariant/complexity refactor:
  - DAG check facts, declaration bodies, instances, constant pools, loaded modules, and plan DAGs are addressed by typed positions and handles instead of fallible lookups. (_#1928_, _#1932_, _#1934_, _#1935_, _#1939_)
  - Typed trees carry their axes and constant keys, and the evaluator reads operands, layouts, and extern results by their checked type. (_#1931_, _#1938_)
  - Diagnostic payloads, runtime failures, binding errors, and domain violations are typed and rendered only at the boundary. `EvaluationError::Failed` was deleted. (_#1931_, _#1933_, _#1934_, _#1938_)
  - Specializations resolve dimension targets once through `CompleteSubstitution`, and one closed-Nat evaluator replaces three. (_#1937_)
  - The refactor-metrics script lists every counted site, and small runtime checks were replaced by types. Internal-error construction sites went from 276 to 161. (_#1929_, _#1939_)
- Made reserved `#[lazy]` unrepresentable in the attribute model and centralized the `#[hidden]` arity check. (_#1964_)
- Keyed LSP extern signature help by a typed callee instead of a formatted string. (_#1963_)
- Removed a duplicated multi-declaration table axis parser. (_#1961_)
- Made the constructor rename regression test unconditional. (_#1962_)
- Bumped the workspace version to v0.0.1-alpha.33. (_#1979_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.32...v0.0.1-alpha.33>
