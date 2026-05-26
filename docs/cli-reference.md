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
| [`lsp`](#graphcal-lsp) | Start the LSP server |

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
| `--plot <MODE>` | Plot output mode: `browser` (open in browser) or `json` (print Vega-Lite JSON) |

When both `--set` and `--input` are provided, `--set` takes precedence.

Params not given via `--set` or `--input` keep their declared defaults.
Params declared without a default (required params) must be provided.

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
`--plot` option to render them. Two output modes are available:

**Browser mode** generates a self-contained HTML file with all figures
(rendered via [Vega-Embed](https://github.com/vega/vega-embed)) and opens
it in the default browser:

```bash
graphcal eval analysis.gcl --plot browser
```

**JSON mode** prints a JSON array of figure objects to stdout, useful for
piping to other tools or pasting into the
[Vega-Lite Editor](https://vega.github.io/editor/):

```bash
graphcal eval analysis.gcl --plot json
```

Each figure in the array has a `name` and a `spec` (Vega-Lite JSON):

```json
[
  { "name": "curve_a", "spec": { /* Vega-Lite spec */ } },
  { "name": "comparison", "spec": { /* Vega-Lite hconcat spec */ } }
]
```

Each `pub plot` declaration produces a standalone figure. Each `figure`
declaration produces a combined chart using Vega-Lite `hconcat`. Each `layer`
declaration produces a chart using Vega-Lite `layer`. Non-`pub` plots are
suppressed from standalone output but still appear in any `figure` or `layer`
that references them.

If no `plot`, `figure`, or `layer` declarations are found, a warning is
printed to stderr.

See the [Plot Declarations](language/plots.md) reference for the language
syntax.

---

## `graphcal format`

Format `.gcl` files. When given a directory, recursively formats all `.gcl` files within.

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
