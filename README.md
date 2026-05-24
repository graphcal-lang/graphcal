# Graphcal

> [!WARNING]
> Graphcal is under active development. Expect breaking changes and bugs.

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Graphcal replaces the spreadsheets and ad-hoc scripts that engineers reluctantly depend on -- Excel mass budgets, throwaway Python notebooks -- with a single typed, version-controlled, reactive computation graph. The compiler tracks physical dimensions through every operation, so a stray `km + kg` or a missing unit conversion fails at compile time, not in flight. Remember the [Mars Climate Orbiter](https://en.wikipedia.org/wiki/Mars_Climate_Orbiter).

```gcl
// rocket.gcl -- the Tsiolkovsky equation
dim Velocity = Length / Time;
dim Acceleration = Length / Time^2;

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

## Why Graphcal?

- **Dimensions as types.** Every `Float` carries a physical dimension. The compiler catches `km + kg` and demands explicit unit conversions.
- **Reactive by design.** `param`, `node`, and `const node` declarations form a DAG that evaluates in dependency order. Override any input from the CLI and dependents recompute automatically.
- **Git-friendly.** Plain text `.gcl` files diff and merge cleanly. No binary spreadsheets, no hidden state.
- **Algebraic types and generics.** Structs, tagged unions with `match`, and generic types with phantom parameters for things like reference frames (`Vec3<Length, Eci>` vs `Vec3<Length, Body>`).
- **Indexed values.** First-class index sets, `for` comprehensions, aggregations (`sum`, `mean`, ...), and `unfold` for time-series and recurrences.
- **Reusable computation.** `dag` blocks parameterize sub-graphs and instantiate them as expressions or via `include`. Multi-file projects compose with `import` and a two-axis (`pub` / `pub(bind)`) visibility system.
- **Built-in plotting.** `plot` and `figure` declarations render to interactive [Vega-Lite](https://vega.github.io/vega-lite/) charts.
- **Live editor experience.** The LSP server provides diagnostics, symbols, hover, go-to-definition, and inlay hints that show computed values inline -- your editor becomes a live calculation sheet.

## Installation

Requires the Rust stable toolchain. Get it from [rustup.rs](https://rustup.rs/) if needed.

```sh
cargo install --git https://github.com/graphcal-lang/graphcal --locked
```

## Quickstart

```sh
# Evaluate a file
graphcal eval rocket.gcl

# Override params (rest keep their defaults)
graphcal eval rocket.gcl --set 'isp=450.0 s'

# Override from JSON, emit JSON, or open an interactive plot
graphcal eval analysis.gcl --input params.json --format json
graphcal eval analysis.gcl --plot browser
```

See the [CLI reference](docs/cli-reference.md) for the full surface, including `format`, `check`, and `lsp`.

## Editor Support

Inlay hints show computed values right next to the source -- install one of the supported integrations and your editor turns into a live notebook.

- **VS Code** -- extension in [`graphcal-lang/vscode-graphcal`](https://github.com/graphcal-lang/vscode-graphcal)
- **Zed** -- extension in [`graphcal-lang/zed-graphcal`](https://github.com/graphcal-lang/zed-graphcal)
- **Neovim / Helix** -- tree-sitter grammar in [`graphcal-lang/tree-sitter-graphcal`](https://github.com/graphcal-lang/tree-sitter-graphcal), plus the `graphcal lsp` server

Setup details for each editor are in the [Editor Setup guide](docs/editor-setup.md).

## Documentation

The [`docs/`](docs/) directory contains the full tutorial, language reference, and CLI/editor guides.

- **[Tutorial](docs/tutorial/index.md)** -- learn Graphcal step by step
- **[Language Reference](docs/language/index.md)** -- every feature, formally
- **[Built-in Reference](docs/language/built-ins.md)** -- constants, math, type conversions, aggregations, prelude dimensions and units
- **[Multi-file Projects](docs/language/multi-file.md)** -- `import`, `include`, and the `pub(bind)` visibility model
- **[CLI Reference](docs/cli-reference.md)** -- `eval`, `format`, `check`, `lsp`

You can serve the docs locally with `zensical serve` and open `http://localhost:8000`.

## Vision

Graphcal is heading toward:

- **Dynamic simulation** -- `scan` over a time axis for system dynamics
- **Python interop** -- parameter sweeps and Monte Carlo at native speed
- **Spreadsheet bridges** -- keep `.gcl` as the source of truth, let domain experts stay in Excel

Track progress and discussion on [GitHub Issues](https://github.com/graphcal-lang/graphcal/issues).

## Project Structure

```
graphcal/
  crates/
    graphcal-compiler/  # lexer (logos) + parser + AST + IR + TIR + registry
    graphcal-eval/      # const eval, runtime eval, project loader, exec plan
    graphcal-fmt/       # code formatter
    graphcal-io/        # filesystem abstraction (real, in-memory, overlay)
    graphcal-cli/       # CLI binary -- `eval`, `format`, `check`, `lsp`
    graphcal-lsp/       # LSP server (tower-lsp) -- diagnostics, symbols, hover, inlay hints
  grammar.ebnf          # formal grammar (source of truth for tree-sitter / TextMate)
  docs/                 # user-facing documentation (Zensical site)
  tests/fixtures/       # .gcl test files: valid/, runtime_error/, invalid/

Editor extensions and tree-sitter grammar live in separate repositories under github.com/graphcal-lang/.
```

## Design Influences

- [Numbat](https://numbat.dev) -- dimensions as types, units as values
- [Sguaba](https://github.com/helsing-ai/sguaba) -- phantom-typed coordinate frames
- [Gleam](https://gleam.run) -- `type` declarations for structs and union types
- [marimo](https://marimo.io) -- reactive DAG on cells, pure text files

## License

Licensed under either of:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
