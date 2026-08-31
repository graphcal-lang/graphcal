# Graphcal

> [!WARNING]
> Graphcal is under active development. Expect breaking changes and bugs.

**A type-safe, unit-aware, Git-friendly reactive programming language for engineering calculations.**

Graphcal is for engineers who want more confidence than spreadsheets and ad-hoc scripts provide. Write plain-text calculation graphs, let the compiler check types and physical dimensions, and see dependent values update when inputs change.

![Graphcal in Helix showing inline computed values for a rocket equation calculation](docs/assets/rocket-screenshot.png)

*The language server shows computed values inline, turning a text file into a live engineering worksheet.*

## See it in action

This example calculates rocket delta-v. `Velocity` and `Acceleration` are prelude dimensions.

```gcl
param dry_mass: Mass = 1200.0 kg;
param fuel_mass: Mass = 2800.0 kg;
param isp: Time = 320.0 s;
const node g0: Acceleration = 9.80665 m/s^2;

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```sh
graphcal eval rocket.gcl
# delta_v = 3778.220768 m/s
```

[Try the example in the browser playground](https://graphcal.org/docs/playground/) without installing anything.

## Why Graphcal?

- **Type- and unit-safe:** dimensional mistakes such as `km + kg` are rejected at compile time.
- **Explicit name resolution:** Static, Term, and Unit lookups are separate; `::`, `#`, and `.` make module members, index labels, and runtime fields unambiguous.
- **Typed reusable interfaces:** `include` and direct DAG calls bind required or optional `type`, `dim`, and `index` inputs by exact category; transitive re-exports retain canonical identities, specialized ADT constructors follow their owner type, nested DAG blueprints use dotted paths, and concrete instances keep plain-unit scales isolated.
- **Reactive:** changing a parameter recomputes its dependents.
- **Git-friendly:** `.gcl` files are plain text and diff cleanly.
- **Engineering-focused:** build reusable computation graphs with dimensions, units, assertions, and visualization.
- **Editor-friendly:** the LSP provides diagnostics, references, rename, and inline computed values.
- **Sandboxed extensibility:** call pinned pure WebAssembly kernels.

## Quickstart

Install the CLI with [Rust](https://rustup.rs/):

```sh
cargo install graphcal --version '^0.0.1-alpha' --locked
```

Save the example above as `rocket.gcl`, then run:

```sh
graphcal eval rocket.gcl
# Try a different input:
graphcal eval rocket.gcl --param 'isp=450.0 s'
# Or bind several params from inline JSON:
graphcal eval rocket.gcl --params-json '{"dry_mass":"1500.0 kg","isp":"450.0 s"}'
# Build a shareable, self-contained interactive HTML report (experimental):
graphcal report build rocket.gcl
# Or omit controls and the embedded browser engine:
graphcal report build rocket.gcl --static
```

For the full CLI, see the [CLI reference](https://graphcal.org/docs/cli-reference/). For a guided introduction, start with the [tutorial](https://graphcal.org/docs/tutorial/).

## Editor support

- **VS Code:** install the [Graphcal extension](https://marketplace.visualstudio.com/items?itemName=Graphcal.graphcal).
- **Zed:** use the [Zed extension](https://github.com/graphcal-lang/zed-graphcal) as a development extension.
- **Neovim / Helix:** use the [tree-sitter grammar](https://github.com/graphcal-lang/tree-sitter-graphcal) with `graphcal lsp`.

See the [editor setup guide](https://graphcal.org/docs/editor-setup/) for details.

## Explore further

- [Language reference](https://graphcal.org/docs/language/)
- [Tenax integration](https://graphcal.org/docs/tenax-integration/)
- [Documentation home](https://graphcal.org/docs/)

## Design influences

- [Numbat](https://numbat.dev) — dimensions as types, units as values
- [Gleam](https://gleam.run) — unified type declarations
- [marimo](https://marimo.io) — reactive graphs in plain-text files
- [Sguaba](https://github.com/helsing-ai/sguaba) — typed coordinate frames

## License

Licensed under either the [MIT License](LICENSE-MIT) or [Apache License, Version 2.0](LICENSE-APACHE), at your option.
