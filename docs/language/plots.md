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
index Time = { T0, T1, T2, T3, T4 };

node altitude: Length[Time] = { ... };

plot altitude_over_time = {
    mark: line,
    encode: {
        x: for t: Time { @altitude[t] -> km },
        y: for t: Time { @altitude[t] -> km },
    },
    title: "Altitude Over Time",
};
```

### Bar chart

```gcl
index Mode = { Normal, Eco, Boost };

node power: Power[Mode] = { ... };

plot power_by_mode = {
    mark: bar,
    encode: {
        x: for m: Mode { @power[m] -> W },
        y: for m: Mode { @power[m] -> W },
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

## Visibility and Standalone Output

By default, plots are **private** and do not produce standalone figures in the
output. To make a plot appear as a standalone chart, mark it `pub`:

```gcl
pub plot curve_a = {
    mark: line,
    encode: {
        x: for t: Time { t },
        y: for t: Time { @altitude[t] -> km },
    },
    title: "Altitude",
};
```

A non-`pub` plot still participates in the computation graph and can be
referenced by `figure` and `layer` declarations -- it simply does not appear
as a standalone chart in the output.

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

To output only the combined figure, omit `pub` from the individual plots:

```gcl
plot curve_a = { mark: line, encode: { ... } };

plot curve_b = { mark: bar, encode: { ... } };

figure comparison = {
    plots: [curve_a, curve_b],
    title: "Combined View",
};
```

This produces **one** figure: `comparison`. The non-`pub` plots are still
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
  time.

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
