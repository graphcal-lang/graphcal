---
icon: material/console
---

# CLI Reference

The `graphcal` command-line tool provides subcommands for evaluating, formatting, and checking `.gcl` files, as well as starting the LSP server.

## Global Options

```bash
graphcal [OPTIONS] <COMMAND>
```

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print version |

## Commands Overview

| Command | Description |
|---------|-------------|
| [`eval`](#graphcal-eval) | Evaluate a `.gcl` file |
| [`format`](#graphcal-format) | Format `.gcl` files |
| [`check`](#graphcal-check) | Check `.gcl` files for errors without evaluation |
| [`graph`](#graphcal-graph) | Export the dependency graph of a `.gcl` file (experimental) |
| [`deps lock`](#graphcal-deps-lock) | Resolve exact-rev Git dependencies and write `graphcal.lock` |
| [`lsp`](#graphcal-lsp) | Start the LSP server |

---

## `graphcal deps lock`

Resolve a package's exact-rev Git dependencies, materialize them in the local
Graphcal cache, verify their source hashes, and write `graphcal.lock`.

```bash
graphcal deps lock [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--root <ROOT>` | Project root directory (overrides automatic `graphcal.toml` detection) |

`graphcal deps lock` is the only public package-management command in the MVP.
It reads `[dependencies]` from `graphcal.toml`, accepts only Git dependencies
with a full commit-hash `rev`, fetches any missing sources, records a
graph-shaped package-instance lockfile, and writes nothing when the deterministic
lockfile contents are already up to date.

Dependency-consuming commands (`check`, `eval`, `graph`, and the LSP) are
read-only with respect to packages: they read `graphcal.lock` and cached
sources, but they do not fetch, create, or update lockfile entries. If the
lockfile is missing, stale, uses a different Graphcal or standard-library
version, or references a missing or hash-mismatched cache entry, they fail and
ask you to run `graphcal deps lock`.

Private Git repositories are supported only when the underlying Git fetch can
obtain credentials from the current environment. This is intentionally not a
portable guarantee: SSH may work with a configured key/agent, while HTTPS may
fail unless a compatible credential helper or non-interactive credential
provider is available. Do not place credentials directly in `git` URLs in
`graphcal.toml`.

**Examples:**

```bash
# Resolve dependencies from the discovered project root
$ graphcal deps lock
wrote /path/to/project/graphcal.lock
```

```bash
# Resolve dependencies for an explicit project root
$ graphcal deps lock --root mission
up to date: /path/to/mission/graphcal.lock
```

See [Package Dependencies](language/multi-file.md#package-dependencies) for
manifest syntax and source-resolution rules.

---

## `graphcal eval`

Evaluate a `.gcl` file and print results.

```bash
graphcal eval [OPTIONS] <FILE>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<FILE>` | Path to the `.gcl` file (required) |

**Options:**

| Option | Description |
|--------|-------------|
| `--format <FORMAT>` | Output format: `text` (default) or `json` |
| `--set <SET>` | Override or provide a param value: `--set 'name=expr'` (repeatable) |
| `--input <INPUT>` | JSON input file for param values |
| `--plot <MODE>` | Plot output mode: `browser` (open in browser), `json` (print only plot JSON to stdout), or a path ending in `.html` (write a self-contained HTML page) |

When both `--set` and `--input` are provided, `--set` takes precedence.

Override names are unqualified parameter names in the entry file (or the local
alias of a selectively included/imported parameter). Qualified strings such as
`module.x=...` are rejected at the CLI boundary instead of being interpreted as
leaf names; the override key never carries module identity.

Params not given via `--set` or `--input` keep their declared defaults.
Params declared without a default (required params) must be provided.

Output entries (text and JSON) are keyed by the full alias-qualified path for
declarations instantiated through `include ... as alias` (e.g. `good.out`,
`good.v_positive`), so multiple instantiations of the same dag never collide
and JSON output never silently drops an instance. This qualification applies
only to output names — the `--set` override surface is unaffected.

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Success, all assertions pass |
| `1` | Assertion failure or evaluation error |
| `2` | Compile error (parse or type check), invalid `--set`/`--input` argument, or internal I/O error |

**Examples:**

```bash
# Basic evaluation
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
g0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.220768 m/s
```

```bash
# Override all parameters
$ graphcal eval rocket.gcl --set 'dry_mass=1200.0 kg' --set 'fuel_mass=3500.0 kg' --set 'isp=320.0 s'
dry_mass   = 1200 kg
fuel_mass  = 3500 kg
isp        = 320 s
g0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.916667
delta_v    = 4284.300858 m/s
```

```bash
# Provide a required param (param declared without a default value)
$ graphcal eval engine.gcl --set 'dry_mass=800.0 kg'
```

The JSON parser preserves module-qualified constructor and index paths inside
structured value payloads. The top-level JSON keys are still parameter names,
and type/dimension checking reports whether the value is valid for that target:

```json
{
  "phase": { "index": "mission.Phase", "entries": { "Burn": 1, "Coast": 0 } },
  "choice": { "variant": "mission.Pick", "fields": { "distance": "2.0 m" } }
}
```

```bash
# JSON output
$ graphcal eval rocket.gcl --format json
{
  "const": {
    "g0": {
      "display_value": 9.80665,
      "si_value": 9.80665,
      "unit": "m/s^2"
    }
  },
  "node": {
    "delta_v": {
      "si_value": 3778.2207684937407,
      "unit": "m/s"
    },
    ...
  },
  "param": {
    "dry_mass": {
      "display_value": 1200.0,
      "si_value": 1200.0,
      "unit": "kg"
    },
    ...
  }
}
```

Multi-indexed values (2D and higher) are displayed as formatted tables:

```bash
# Indexed values displayed as tables
$ graphcal eval mission.gcl
delta_v[Departure]  = 2.46 km/s
delta_v[Correction] = 0.12 km/s
delta_v[Insertion]  = 1.83 km/s

spacecraft_mass (kg):
╭─────────┬───────────┬────────────┬───────────╮
│         │ Departure │ Correction │ Insertion │
├─────────┼───────────┼────────────┼───────────┤
│ Launch  │      5000 │          0 │         0 │
│ Cruise  │         0 │       4500 │         0 │
│ Arrival │         0 │          0 │      4000 │
╰─────────┴───────────┴────────────┴───────────╯
```

1D indexed values remain as flat lines, while 2D values are shown as table
grids. 3D and higher values are displayed as multiple 2D table slices with
section headers.

When a file contains assertions, they are checked after evaluation and printed
below the values:

```bash
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
...

Assertions:
  fuel_budget    PASS
  fuel_positive  PASS
  pressure_safe  FAIL  (assertion evaluated to false)
                       affected: safety_factor
```

### Plot Output

When a file contains `plot`, `figure`, or `layer` declarations, use the
`--plot` option to render them. Three output modes are available:

**Browser mode** generates a self-contained HTML file with all figures
(rendered via [Vega-Embed](https://github.com/vega/vega-embed)) and opens
it in the default browser. Normal evaluation output is still printed according
to `--format`:

```bash
graphcal eval analysis.gcl --plot browser
```

**JSON mode** prints only a JSON array of figure objects to stdout, useful for
piping to other tools or pasting into the
[Vega-Lite Editor](https://vega.github.io/editor/). Normal evaluation output is
suppressed in this mode, and `--format` does not add eval values to stdout:

```bash
graphcal eval analysis.gcl --plot json
```

**HTML file mode** writes the same self-contained HTML page that browser mode
generates to a path of your choice (the path must end in `.html`) without
opening a browser — useful in headless or CI environments. Normal evaluation
output is still printed according to `--format`:

```bash
graphcal eval analysis.gcl --plot report.html
```

Each figure in the array has a `name` and a `spec` (Vega-Lite JSON):

```json
[
  { "name": "curve_a", "spec": { /* Vega-Lite spec */ } },
  { "name": "comparison", "spec": { /* Vega-Lite hconcat spec */ } }
]
```

Each `plot` declaration produces a standalone figure. Each `figure`
declaration produces a combined chart using Vega-Lite `hconcat`. Each `layer`
declaration produces a chart using Vega-Lite `layer`. Plots marked
`#[hidden]` are suppressed from standalone output but still appear in any
`figure` or `layer` that references them.

If no `plot`, `figure`, or `layer` declarations are found, JSON mode prints
`[]` to stdout and a warning to stderr. Browser mode prints the warning and
opens nothing.

See the [Plot Declarations](language/plots.md) reference for the language
syntax.

---

## `graphcal format`

Format `.gcl` files. When given a directory, recursively formats regular
`.gcl` files within. Symlinked entries found during directory traversal are
skipped; an explicitly named symlinked file path is treated like any other
file argument and may write through to its target.

Formatting only ever changes layout, never meaning. After producing the
formatted text, the formatter re-parses it and verifies the result is the same
syntax tree as the input (ignoring source positions). If they ever diverge —
which would be a bug in the formatter, not in your code — formatting fails with
an error instead of writing a file whose meaning might differ from the source.

```bash
graphcal format [OPTIONS] [PATHS]...
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `[PATHS]...` | Files or directories to format (default: current directory) |

**Options:**

| Option | Description |
|--------|-------------|
| `--check` | Check formatting without modifying files (exit 1 if unformatted) |

**Examples:**

```bash
# Format all .gcl files in the current directory
graphcal format

# Format specific files
graphcal format rocket.gcl hohmann.gcl

# Check formatting in CI (non-destructive)
graphcal format --check
```

---

## `graphcal check`

Check `.gcl` files for type/dimension errors without evaluation. Performs parsing and type/dimension checking.

```bash
graphcal check [PATHS]...
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `[PATHS]...` | Files or directories to check (default: current directory) |

**Examples:**

```bash
# Check all .gcl files in the current directory
graphcal check

# Check a specific file
graphcal check rocket.gcl

# Check a directory
graphcal check my_project/
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | No errors found |
| `1` | Errors detected |

---

## `graphcal graph`

!!! warning "Experimental"
`graphcal graph` is experimental. The DOT output (node naming, labels,
styling) and the CLI surface (flags, formats) may change in any release
while the [DAG visualizer](https://github.com/graphcal-lang/graphcal/issues/512)
design evolves. The command prints a warning on stderr to that effect;
stdout carries only the exported graph.

Export the dependency graph of a `.gcl` file as text for external rendering
tools. The graph is a one-way projection of the compiled program: `param`,
`const node`, and `node` declarations become vertices; every `@` reference
becomes a directed edge from the value being read to the declaration reading
it. Inline `dag` blocks render as nested clusters. Assertions, plots, figures,
and layers are not part of the dataflow and are omitted.

```bash
graphcal graph [OPTIONS] <FILE>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<FILE>` | Path to the `.gcl` file (required) |

**Options:**

| Option | Description |
|--------|-------------|
| `--format <FORMAT>` | Output format: `dot` (default; currently the only format) |
| `--root <ROOT>` | Project root directory (overrides automatic `graphcal.toml` detection) |

The output is deterministic (declarations keep source order, edges are sorted),
so exported graphs diff cleanly in version control.

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Graph exported successfully |
| `2` | Compile error (parse or type check) or I/O error |

**Examples:**

```bash
# Print Graphviz DOT text to stdout
$ graphcal graph rocket.gcl
digraph graphcal {
    rankdir=LR;
    node [fontname="Helvetica,Arial,sans-serif"];
    "rocket.dry_mass" [label="dry_mass\nMass", shape=ellipse];
    ...
    "rocket.v_exhaust" -> "rocket.delta_v";
}

# Render to SVG with Graphviz
graphcal graph rocket.gcl | dot -Tsvg -o rocket.svg
```

Node styling encodes the declaration kind: `param` declarations are ellipses
(the graph's inputs), `const node` declarations are rounded boxes, `node`
declarations are plain boxes, and values imported from other files are dashed
boxes labeled with their fully qualified name. Each vertex's label shows the
declaration name and its resolved type.

---

## `graphcal lsp`

Start the Language Server Protocol (LSP) server. The server communicates over stdin/stdout and is intended to be launched by an editor.

```bash
graphcal lsp
```

This command has no additional options. See [Editor Setup](editor-setup.md) for how to configure your editor to use the LSP.

### LSP Features

The LSP server provides:

- **Diagnostics** -- Real-time parse errors, dimension mismatches, unknown references, assertion failures
- **Inlay hints** -- Computed param/node values displayed inline
- **Go to definition** -- Navigate from references to declarations
- **Hover** -- Show resolved type and dimension information
- **Find references** -- Find all usages of a declaration
- **Document symbols** -- Outline view of all declarations
- **Formatting** -- Format the current document
- **Document links** -- Clickable `import` paths
