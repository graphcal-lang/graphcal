---
icon: material/chart-line
---

# Plot Declarations

Plot declarations define charts that visualize computed values from the
computation graph. They are rendered using [Vega-Lite](https://vega.github.io/vega-lite/)
and produce interactive HTML or JSON output.

## Syntax

```
plot <name> = {
    mark: <mark_type>,
    encode: {
        <channel>: <expr>,
        ...
    },
    <property>: <expr>,
    ...
};
```

A `plot` declaration has:

- A **name** (conventionally `lower_snake_case`, like `param` and `node`).
- A **`mark` field** specifying the visual mark type (required).
- An **`encode` block** mapping data to visual channels (required, with at
  least one channel).
- Optional **properties** like `title`.

Each field — `mark`, `encode`, each encoding channel, and each property —
may appear at most once; duplicates are parse errors.

The plot-level properties:

| Property | Type |
|----------|------|
| `title` | String literal |
| `width` | Positive dimensionless number |
| `height` | Positive dimensionless number |
| `x_label` | String literal |
| `y_label` | String literal |

Property names and value types are validated by `graphcal check`: an unknown
name (e.g. a misspelled `title:`) is an error, a wrongly-typed value (e.g.
`title: 42.0`) is an error, and a dimensioned value on a raw rendering
quantity (e.g. `width: 2.0 m`) is rejected — units never get silently
stripped. `width`/`height` must be strictly positive (checked at evaluation
time).

### Mark Types

| Type | Description |
|------|-------------|
| `point` | Scatter plot / point marks |
| `line` | Line chart for trends and time series |
| `bar` | Bar chart for categorical comparison |
| `area` | Area chart (filled region) |
| `rect` | Rectangle marks (heat maps, 2D bins) |
| `tick` | Tick marks for distributions |

Mark types can have optional properties:

```gcl
plot styled = {
    mark: line { stroke_width: 2.0 },
    encode: { ... },
};
```

| Mark property | Type |
|---------------|------|
| `stroke_width` | Dimensionless number |
| `opacity` | Dimensionless number |
| `size` | Dimensionless number |
| `color` | String literal |
| `filled` | Boolean |
| `interpolate` | String literal |

### Encoding Channels

The `encode` block maps data to visual channels:

| Channel | Description |
|---------|-------------|
| `x` | X-axis position |
| `y` | Y-axis position |
| `color` | Color encoding (also used for heat maps) |
| `size` | Size encoding |
| `shape` | Shape encoding |
| `opacity` | Opacity encoding |
| `detail` | Detail channel (for grouping) |
| `text` | Text channel |
| `tooltip` | Tooltip channel |

Channel values are typically `for` comprehensions producing indexed data:

```gcl
encode: {
    x: for m: Maneuver { @delta_v[m] },
    y: for m: Maneuver { @mass[m] },
},
```

### Channel Alignment

All channels of one plot are flattened onto a single shared row set:

- The row set is the cross product of the index axes of the channel with the
  *widest* axis set. A two-variable comprehension like
  `color: for p: P, t: T { ... }` drives one row per `P × T` cell.
- Every other channel must range over a subset of those axes; its values are
  broadcast across the axes it does not mention. A channel with no index (a
  plain scalar) repeats on every row.
- Channels ranging over unrelated indexes (e.g. `x: for s: Step { ... }` with
  `y: for p: Pair { ... }`) have no meaningful row pairing and are rejected
  with an error — rows are never silently padded or misaligned.
- Values that cannot be represented in a plot (structs, or a mix of numbers
  and labels within one channel) are errors. Index variant names are never
  substituted for data.

Booleans encode as the labels `"true"`/`"false"`, labels as their variant
names, datetimes as ISO 8601 timestamps with a temporal axis, and numbers
as quantitative data.

### Unit-Aware Axis Titles

Graphcal auto-generates axis titles from dimensional metadata. When an
encoding channel references a dimensioned declaration (via `@`), the axis
title is formatted as "Dimension (unit)":

- `@velocity` with display unit `km/s` produces axis title **"Velocity (km/s)"**
- `@power` with display unit `W` produces axis title **"Power (W)"**
- Dimensionless values produce no automatic title

You can override auto-generated titles with explicit `x_label` or `y_label`
properties:

```gcl
plot custom_labels = {
    mark: point,
    encode: {
        x: for m: Maneuver { @delta_v[m] -> km/s },
        y: for m: Maneuver { @mass[m] -> kg },
    },
    x_label: "Mission Delta-V",
    y_label: "Spacecraft Dry Mass",
};
```

## Examples

### Line chart

```gcl
index Sample = { T0, T1, T2, T3, T4 };

node elapsed: Time[Sample] = { ... };
node altitude: Length[Sample] = { ... };

plot altitude_over_time = {
    mark: line,
    encode: {
        x: for sample: Sample { @elapsed[sample] -> s },
        y: for sample: Sample { @altitude[sample] -> km },
    },
    title: "Altitude Over Time",
};
```

### Bar chart

```gcl
index Mode = { Normal, Eco, Boost };

node mode_code: Dimensionless[Mode] = { ... };
node power: Power[Mode] = { ... };

plot power_by_mode = {
    mark: bar,
    encode: {
        x: for mode: Mode { @mode_code[mode] },
        y: for mode: Mode { @power[mode] -> W },
    },
    title: "Power by Operating Mode",
};
```

### Scatter plot

```gcl
index Maneuver = { Departure, Correction, Insertion };

node delta_v: Velocity[Maneuver] = { ... };
node mass: Mass[Maneuver] = { ... };

plot mass_vs_dv = {
    mark: point,
    encode: {
        x: for m: Maneuver { @delta_v[m] -> km/s },
        y: for m: Maneuver { @mass[m] -> kg },
    },
    title: "Mass vs Delta-V",
};
```

### Heat map

```gcl
plot efficiency_map = {
    mark: rect,
    encode: {
        x: for p: Pressure { p },
        y: for t: Temperature { t },
        color: for p: Pressure, t: Temperature { @efficiency[p, t] },
    },
    title: "Efficiency Map",
};
```

## Display and Visibility

Plots are **displayed standalone by default** — you write a plot to see it:

```gcl
plot curve_a = {
    mark: line,
    encode: {
        x: for t: Time { t },
        y: for t: Time { @altitude[t] -> km },
    },
    title: "Altitude",
};
```

To keep a plot as a composition-only building block (referenced by `figure`
or `layer` declarations but not rendered standalone), mark it `#[hidden]`:

```gcl
#[hidden]
plot curve_a = { mark: line, encode: { ... } };
```

A `#[hidden]` plot still participates in the computation graph and can be
referenced by `figure` and `layer` declarations — it simply does not appear
as a standalone chart. `#[hidden]` is valid only on `plot` declarations
(figures and layers cannot be referenced by anything, so hiding one would be
equivalent to deleting it).

Display and cross-file visibility are independent axes: `pub` makes a plot
includable by consumer files (like `pub` on any other declaration) and says
nothing about display. `#[hidden]` governs only the declaring file's own
output when that file is the entry point.

## Cross-File Plots

A library plot never displays implicitly in a consumer's output — the
consumer of a library cannot edit its code, so the consumer (not the library
author) controls what is displayed. Naming a `pub plot` in an include's brace
list is the display request:

```gcl
include pkg.engine(fuel: 500.0 kg).{ delta_v, thrust_curve, mass_breakdown as mb };
```

- A library plot must be `pub` to be includable — `pub` keeps its single
  meaning (exported across the file boundary), exactly as for other
  declarations.
- An included plot evaluates against **its instance** — the include's
  parameter bindings. Two instantiations of the same library may include the
  same plot under different aliases, and both render.
- Included plots enter the root namespace under their local alias and are
  referenceable from root `figure`/`layer` declarations. Alias collisions
  with root declarations are the usual duplicate-name error.
- To include a plot **for composition only** (e.g. to layer it into a
  consumer figure without standalone output), put `#[hidden]` on the include
  item:

```gcl
include pkg.engine(fuel: 500.0 kg).{
    thrust_curve,                    // displayed
    #[hidden] mass_breakdown as mb,  // included for composition only
};

figure summary = { plots: [thrust_curve, mb] };
```

- `#[hidden]` on a non-plot include item is an error. An explicit include of
  a library plot that is itself declared `#[hidden]` **does** display it: the
  include is the consumer's explicit request, and the library author does not
  control the consumer's output.
- Requesting a plot via `import` is an error: plots are runtime sinks
  evaluated against an instance, and `import` carries only compile-time
  names.
- Library plots that are *not* named in any brace list (including everything
  behind a module-form `include ... as alias`) are simply not part of the
  consumer's output.

## Figure Declarations

A `figure` declaration groups multiple plots into a single combined chart with
side-by-side subplots (horizontal concatenation).

```
figure <name> = {
    plots: [<plot_name>, ...],
    <field>: <expr>,
    ...
};
```

A `figure` declaration has:

- A **name** (conventionally `lower_snake_case`).
- A **`plots` field** listing the plot names to combine: `plots: [a, b]`.
- Optional **fields** like `title` (a string literal).

| Field | Type | Description |
|-------|------|-------------|
| `plots` | List of plot names | Plots to include as subplots (required) |
| `title` | String literal | Figure title |

`title` is the only property a figure supports: figures render as
side-by-side concatenation, which has no overall width/height — set sizes on
the constituent plots (or use a `layer`). `width:`/`height:` on a figure are
check-time errors.

### Example

```gcl
plot curve_a = {
    mark: line,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] * @values[s] },
    },
    title: "Values Squared",
};

plot curve_b = {
    mark: bar,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] + 1.0 },
    },
    title: "Values Plus One",
};

figure comparison = {
    plots: [curve_a, curve_b],
    title: "Side-by-side Comparison",
};
```

This produces **three** figures in the output: `curve_a` (standalone),
`curve_b` (standalone), and `comparison` (combined subplot chart).

### Hiding Standalone Plots

To output only the combined figure, mark the individual plots `#[hidden]`:

```gcl
#[hidden]
plot curve_a = { mark: line, encode: { ... } };

#[hidden]
plot curve_b = { mark: bar, encode: { ... } };

figure comparison = {
    plots: [curve_a, curve_b],
    title: "Combined View",
};
```

This produces **one** figure: `comparison`. The `#[hidden]` plots are still
evaluated and included in the combined figure, but do not appear as standalone
charts.

## Layer Declarations

A `layer` declaration overlays multiple plots on shared axes, producing a
single chart with layered marks. This is useful for combining different mark
types (e.g., line + point) on the same data.

```
layer <name> = {
    plots: [<plot_name>, ...],
    <field>: <expr>,
    ...
};
```

| Field | Type | Description |
|-------|------|-------------|
| `plots` | List of plot names | Plots to overlay (required) |
| `title` | String literal | Layer title |
| `width` | Positive dimensionless number | Chart width in pixels |
| `height` | Positive dimensionless number | Chart height in pixels |

### Layer Example

```gcl
plot line_trace = {
    mark: line,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] * @values[s] },
    },
};

plot point_trace = {
    mark: point,
    encode: {
        x: for s: Step { @values[s] },
        y: for s: Step { @values[s] * @values[s] },
    },
};

layer line_with_points = {
    plots: [line_trace, point_trace],
    title: "Line with Points",
};
```

This overlays the line and point marks on the same axes.

## Key Properties

- Plot, figure, and layer names conventionally use `lower_snake_case`, like
  `param` and `node`.
- Plot bodies can reference any `@param` or `@node`, plus constants.
- Plots, figures, and layers are **leaf nodes** -- no declaration can reference
  them with `@`.
- Plots participate in the dependency graph (they depend on the nodes they
  reference) but do not produce runtime values.
- Figures and layers reference plots by name and are validated at resolution
  time: an unknown name, a reference to another figure/layer (they cannot
  nest), or a repeated entry in `plots:` is a check-time error, and the
  `plots:` list must be non-empty.

## CLI Output

Plot output is controlled by the `--plot` option on `graphcal eval`:

```bash
# Open interactive chart in default browser
graphcal eval file.gcl --plot browser

# Print only the plot JSON array to stdout
graphcal eval file.gcl --plot json

# Write a self-contained HTML page (headless/CI-friendly)
graphcal eval file.gcl --plot report.html
```

In `--plot json` mode, stdout is exactly one JSON array of figure objects,
each with a `name` and `spec`; normal evaluation output is suppressed so the
result can be piped directly to JSON tools:

```json
[
  { "name": "curve_a", "spec": { /* Vega-Lite JSON */ } },
  { "name": "comparison", "spec": { /* Vega-Lite hconcat spec */ } }
]
```

See [CLI Reference](../cli-reference.md#plot-output) for details.
