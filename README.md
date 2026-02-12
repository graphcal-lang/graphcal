# Kasuri

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Kasuri replaces the spreadsheets and simulation tools that engineers reluctantly depend on -- Excel mass budgets, Vensim logistics models, ad-hoc Python scripts -- with a single typed, version-controlled, reactive computation graph.

## Current Status: Phase 0 (Scalar Graph)

Phase 0 is implemented and working. It supports `f64` arithmetic with `param`, `node`, and `const` declarations, a reactive computation DAG, and a CLI that evaluates `.ksr` files.

```
// rocket.ksr
param dry_mass = 1200.0;
param fuel_mass = 2800.0;
param isp = 320.0;
const G0 = 9.80665;

node v_exhaust = @isp * G0;
node mass_ratio = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v = @v_exhaust * ln(@mass_ratio);
```

```
$ kasuri eval rocket.ksr
dry_mass   = 1200
fuel_mass  = 2800
isp        = 320
G0         = 9.80665
v_exhaust  = 3138.128
mass_ratio = 3.333333
delta_v    = 3778.220768
```

### Phase 0 features

- Three declaration kinds: `param` (inputs), `node` (computed), `const` (compile-time)
- `@` sigil for graph references, bare `UPPER_SNAKE_CASE` for const references
- Arithmetic (`+`, `-`, `*`, `/`, `^`), comparison, boolean (`&&`, `||`, `!`), `if`/`else`
- 14 built-in functions (`sqrt`, `sin`, `cos`, `ln`, `exp`, `atan2`, `min`, `max`, etc.)
- Built-in constants (`PI`, `E`)
- Two-phase evaluation: compile-time (const) then runtime (param/node DAG)
- Casing enforcement: `const` must be `UPPER_SNAKE_CASE`, `param`/`node` must be `lower_snake_case`
- Rich error diagnostics via [miette](https://github.com/zkat/miette) with error codes and source spans
- JSON output (`--format json`)

## Installation

```sh
cargo build -p kasuri
```

## Usage

```sh
# Evaluate a .ksr file (text output)
kasuri eval path/to/file.ksr

# JSON output
kasuri eval path/to/file.ksr --format json
```

## Project Structure

```
kasuri/
  crates/
    kasuri-syntax/   # lexer (logos) + recursive descent parser + AST
    kasuri-eval/     # name resolution, const eval, DAG (petgraph), runtime eval
    kasuri-cli/      # CLI binary (clap + miette)
  design/            # language design documents
  tests/fixtures/    # .ksr test files
```

## Vision

Kasuri is designed to eventually support:

- **Physical dimensions and units** -- the compiler catches `km + kg` before you run anything
- **Coordinate frame safety** -- prevents mixing vectors from different reference frames at compile time
- **Multi-line nodes** -- full function bodies with local variables
- **Tables with autofill** -- add a row, computed columns extend automatically
- **Dynamic simulation** -- `scan` over a time axis for system dynamics
- **Multi-file projects** -- explicit imports, namespaces, prelude
- **Python interop** -- parameter sweeps and Monte Carlo at native speed
- **Spreadsheet import/export** -- maintain `.ksr` source, domain experts keep their Excel

See the [design documents](design/README.md) and [phase roadmap](design/phases/README.md) for details.

## Design Influences

- [Numbat](https://numbat.dev) -- dimensions as types, units as values
- [Sguaba](https://github.com/helsing-ai/sguaba) -- phantom-typed coordinate frames
- [Gleam](https://gleam.run) -- unified `type` for structs and enums
- [marimo](https://marimo.io) -- reactive DAG on cells, pure text files

## License

TBD
