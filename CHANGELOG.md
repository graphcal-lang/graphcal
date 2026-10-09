# Changelog

Release notes for Graphcal. Each release section becomes the body of its GitHub Release.
Add a `## v<version>` section before running the release workflow.
Earlier releases are listed on [GitHub Releases](https://github.com/graphcal-lang/graphcal/releases).

## v0.0.1-alpha.35

This release is the first to ship prebuilt `graphcal` binaries, so you no longer need a Rust toolchain to install Graphcal.
It contains no language changes.

### Prebuilt Binaries

- Each GitHub Release now includes prebuilt `graphcal` archives for five targets, shell and PowerShell installers, a `sha256.sum` checksum file, and GitHub build attestations. (_#1982_)
  - Linux x86_64 and arm64 (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`), statically linked
  - macOS x86_64 and arm64 (`x86_64-apple-darwin`, `aarch64-apple-darwin`)
  - Windows x86_64 (`x86_64-pc-windows-msvc`)
- Alpha releases are marked as the latest release, so the installer URLs under `releases/latest/download/` always point to the newest version. For example, on Linux or macOS:

  ```sh
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.sh | sh
  ```

  (_#1982_)

- The Linux binaries use the mimalloc allocator. With musl's default allocator, heavy workloads ran 20–25% slower than a glibc build; with mimalloc the musl build matches or beats it. Other targets are unchanged. (_#1982_)

### Dependency, Toolchain, and Workflow Maintenance

- The release workflow builds the binaries with [dist](https://axodotdev.github.io/cargo-dist/) v0.33.0, publishes the crates to crates.io, and creates the GitHub Release only after publishing succeeds. A dry-run mode builds every artifact without publishing, and pull requests run only `dist plan`. `RELEASING.md` documents the procedure. (_#1982_)
- Release notes now come from the hand-written `CHANGELOG.md` section for the version. The release fails early when the section is missing. (_#1982_)

### Internal

- Added the v0.0.1-alpha.34 notes to `CHANGELOG.md` and bumped the workspace version to v0.0.1-alpha.35. (_#2025_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.34...v0.0.1-alpha.35>

## v0.0.1-alpha.34

This release redesigns the interactive report UI and fixes several defects in the Vega-Lite specs generated for plots.
It also fixes bugs around included DAGs and multi-declarations in reports, evaluation output, DAG calls, and the formatter, and it validates constructor field constraints on temporary values during constant evaluation.
The English documentation now lives under `/docs/en/`.

**Some existing Graphcal source programs may need to be modified.** The breaking change rejects programs that were previously accepted only because of a bug. See [Breaking Changes](#breaking-changes).

### Breaking Changes

- A constrained constructor field is now validated even when the constructed value is temporary in constant evaluation, in declaration bounds, or in field bounds.
  Before, a value discarded by projection or pattern matching was never checked, so `const node BAD: Mass = Spec(mass: 5000.0 kg).mass;` compiled even though `Spec.mass` is declared as `Mass(max: 2000.0 kg)`.
  Such programs are now rejected with C001, which names the field and points at the offending argument.
  (_#2014_)

### Interactive Report Redesign

- A header block in interactive reports summarizes checks, lists overridden inputs with a Reset action, and shows the engine status. It replaces the corner chip and the top banner. Clicking the check summary opens the Checks tab. (_#1989_)
- Result tabs show counts. Values, plots, and checks can be pinned to a resizable pinned area. (_#1989_)
- Values changed by the last run are marked, and stale results are dimmed while edits are pending or rejected. (_#1989_)
- Each input row shows its domain hint, a slider when the input is bounded, its description, and override and draft markers. Quantity fields show the canonical SI unit tag, such as `[kg]`. An Apply button appears when Auto run is off. (_#1989_)
- Sliders appear only for literals written in their canonical SI unit. Before, `2.0 km` on a 0–5000 m slider sat at 0, and touching the slider wrote `0.0 m`. (_#1989_)
- Reports have a dark mode, all text meets WCAG AA contrast, and charts follow the report theme. (_#1989_)
- The Advanced controls and the raw-literal entry are removed. The per-input `⋯` menu remains. (_#1989_)
- The same UI is used by `graphcal report` pages and the playground's Report tab. The playground loads the report scripts on first render. (_#1989_)

### Bug Fixes

- Plots:
  - Sample points are visible again. Boolean mark properties such as `filled: true` were emitted as strings, which Vega-Lite drew as transparent points. (_#1989_)
  - Index-label axes keep declaration order instead of being sorted alphabetically. (_#1989_)
  - Quantity axes without a `->` display unit show the canonical SI unit of the plotted values, for example "Time (s)" instead of "Time". Placeholder `x` and `y` titles are removed. (_#1989_)
  - Nominal x-axis labels stay horizontal when there is enough room. (_#1989_)
- Reports list only externally bindable entry parameters under Inputs and Baseline. Parameters of included DAGs that the caller binds now appear under Values. (_#1997_)
- Plots from included DAGs have their documentation captions again in HTML and Markdown reports, including aliased projections and nested file includes. (_#1998_)
- A `///` documentation comment on a slot of a multi-declaration now attaches only to that slot. Before, undocumented slots inherited the first slot's comment in hovers and report captions. (_#2005_)
- Categorized `graphcal eval` JSON output lists only externally bindable entry parameters under `param`. Included DAG input ports are listed under `node`. (_#2011_)
- A DAG call expression such as `@facade(x: 1.0 m)::y` can project a public alias that re-exports an include output (`include ...::{ pub y }`), as a selective include already could. Before, it was rejected with G005. (_#2000_)
- `graphcal format` no longer drops the leading `pub` of a multi-declaration slot that has a comment, and it keeps per-slot documentation directly above its slot while aligning headers and table cells. (_#1999_, _#2012_)

### Documentation

- The English documentation moved from `/docs/` to `/docs/en/`, alongside the Japanese edition at `/docs/ja/`. The site root and `/docs/` redirect to `/docs/en/`, but old English deep links no longer work. The language selector keeps you on the same page. (_#2016_)
- The Index Keys and `argmax` examples use `node` instead of `param`, so they compile with a private index. (_#2013_)
- The report guide in the tutorial and the CLI reference describe the redesigned report UI. (_#1989_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated the pinned Rust toolchain to v1.99.0. The minimum supported Rust version stays at 1.95. (_#2017_)
- `rust-toolchain.toml` declares the `wasm32-unknown-unknown` target, so rustup installs it automatically for source builds. (_#2020_)
- Updated Rust crates: `jiff` to v0.2.38, `jiff-tzdb` to v0.1.9 (bundled IANA time zone database 2026e), `gix` to v0.89.0, and `toml` to v1.1.8. (_#2008_, _#2007_, _#2019_, _#2018_, _#2021_)
- Updated Zensical to v0.0.67, pnpm to v12.10.0, and `@types/node` to v26.6.4. The documentation workflows pin Zensical v0.0.68. (_#1986_, _#2015_, _#2016_, _#1987_, _#2001_, _#2022_, _#1984_)
- Updated `taiki-e/install-action` to v2.87.22, `leanprover/lean-action` to v1.6.1, and the pre-commit hooks for rumdl, typos, and zizmor. (_#2006_, _#2023_, _#1988_)
- Refreshed lock files. (_#1985_)
- The report engine toolchain action supports Linux arm64, macOS arm64, and macOS x86_64 runners, as preparation for prebuilt binaries. Workflows use GitHub's self-repository `uses: $/...` syntax for local actions. (_#1981_)

### Internal

- Simplified the bilingual documentation build: removed the custom theme overrides, the localization checkers, and the translation manifest, and left language switching to Zensical. (_#2016_)
- Bumped the workspace version to v0.0.1-alpha.34. (_#2024_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.33...v0.0.1-alpha.34>

## v0.0.1-alpha.33

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
