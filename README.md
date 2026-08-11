# Graphcal

> [!WARNING]
> Graphcal is under active development. Expect breaking changes and bugs.

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Graphcal replaces the spreadsheets and ad-hoc scripts that engineers reluctantly depend on -- Excel mass budgets, throwaway Python notebooks -- with a single typed, version-controlled, reactive computation graph. The compiler tracks physical dimensions through every operation, so a stray `km + kg` or a missing unit conversion fails at compile time, not in flight. Remember the [Mars Climate Orbiter](https://en.wikipedia.org/wiki/Mars_Climate_Orbiter).

![Graphcal in Helix showing inline computed values for a rocket equation calculation](docs/assets/rocket-screenshot.png)

*Graphcal's language server shows computed node values inline, so a plain-text calculation file feels like a live engineering worksheet.*

```gcl
// rocket.gcl -- the Tsiolkovsky equation
// `Velocity` and `Acceleration` are prelude dimensions.

param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
const node g0: Acceleration = 9.80665 m/s^2;

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```sh
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
g0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

[Try `rocket.gcl` in the browser playground](https://graphcal.org/docs/playground/) without installing anything.

## Why Graphcal?

- **Unit-safe.** Physical dimensions—including generic field constraints, unit definitions, and runtime unit-scale expressions—are checked at compile time, conversions are explicit, and public values require matching checked type and constructor metadata instead of dropping field units.
- **Reactive.** Change a parameter and every dependent value is recomputed automatically.
- **Git-friendly.** Plain-text `.gcl` files diff and merge cleanly, with no hidden spreadsheet state.
- **Built for engineering.** Typed axes, dimension-aware linear algebra, assertions with per-index expected-failure tracking, checked structured display provenance, unit-aware plotting, and reusable computation graphs are built in.
- **Resource-bounded.** Composite eager shapes are checked before allocation, and expensive dense kernels use a cooperative, cancellation-aware work budget.
- **Live in your editor.** The language server provides source-accurate diagnostics, canonical project-wide references and safe rename, and inline computed values.

## Installation

Requires the Rust stable toolchain. Get it from [rustup.rs](https://rustup.rs/) if needed.

```sh
cargo install graphcal --locked
graphcal --version  # prints the package version and, when available, the build commit SHA
```

## Quickstart

```sh
# Evaluate a file
graphcal eval rocket.gcl

# Bind entry-file params to closed values (rest keep their defaults)
graphcal eval rocket.gcl --set 'isp=450.0 s'

# Serve bounded scalar inputs and public Bool outputs to Tenax over Arrow IPC
graphcal model serve reliability.gcl --output failure

# Bind from JSON (input errors point back to params.json) or emit JSON
graphcal eval analysis.gcl --input params.json --format json
graphcal eval analysis.gcl --output-view all
graphcal eval analysis.gcl --plot browser

# Canonically format source (with a 100-column target for breakable layouts)
graphcal format rocket.gcl

# Export the dependency graph as Graphviz DOT (experimental)
graphcal graph rocket.gcl | dot -Tsvg -o rocket.svg
```

See the [CLI reference](https://graphcal.org/docs/cli-reference/) for the full command-line interface and the [Tenax integration guide](https://graphcal.org/docs/tenax-integration/) for persistent model serving. `--set` and `--input` accept recursively closed values, not references or computations; indexed input entries deeper than their declared schema are rejected. Imported constants, static units, and prepared structured inputs retain canonical definition-site metadata, so consumers do not need to re-import backing dimensions or schema implementation dependencies they never name. Imports remain compile-time-only blueprints; self-imports and cross-file imports share the same policy, while explicit `include` instances own runtime values, assertions, visualization requests, and dynamic unit scales, with namespace-typed substitutions for selective re-exports. Validated function signatures fail explicitly if dimension-binding provenance is inconsistent. Structural finite-index widening uses typed cardinality forms and never falls back when declared-axis metadata is missing. Unknown qualified dimensions retain structured paths through diagnostics. Civil datetime and epoch literals require an explicit time; date-only strings never imply midnight. DOT exports use opaque renderer-local statement ids so human-readable name collisions cannot merge semantic graph identities. Configured `include` composition may nest to arbitrary acyclic depth while preserving isolated instance paths and rejecting inconsistent owner rebases. Quoted literals used for datetimes, timezones, plot labels, and plugin paths must begin and end on the same physical source line; Graphcal has no multiline string syntax. Package sources and plugins are read through canonical root capabilities with pre-allocation byte/count budgets; locked source trees reject symlinks and special files. Recursive algebraic types remain ordinary finite runtime values, while transports without recursive-schema support reject them only at the transport boundary.

## Editor Support

Inlay hints show computed values right next to the source -- install one of the supported integrations and your editor turns into a live notebook.

- **VS Code** -- install the published [Graphcal extension](https://marketplace.visualstudio.com/items?itemName=Graphcal.graphcal) from the Visual Studio Marketplace
- **Zed** -- extension source in [`graphcal-lang/zed-graphcal`](https://github.com/graphcal-lang/zed-graphcal) (not yet published; install as a dev extension)
- **Neovim / Helix** -- tree-sitter grammar in [`graphcal-lang/tree-sitter-graphcal`](https://github.com/graphcal-lang/tree-sitter-graphcal), plus the `graphcal lsp` server

Setup details for each editor are in the [Editor Setup guide](https://graphcal.org/docs/editor-setup/).

## Design Influences

- [Numbat](https://numbat.dev) -- dimensions as types, units as values
- [Gleam](https://gleam.run) -- unified `type` declarations for structs and union types
- [marimo](https://marimo.io) -- reactive DAG on cells, pure text files
- [Sguaba](https://github.com/helsing-ai/sguaba) -- phantom-typed coordinate frames

## License

Licensed under either of:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
