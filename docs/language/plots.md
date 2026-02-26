---
icon: material/chart-line
---

# Plot Declarations

Plot declarations define charts that visualize computed values from the
computation graph. They are rendered using [Plotly.js](https://plotly.com/javascript/)
and produce interactive HTML or JSON output.

## Syntax

```
plot <name> = <chart_type> {
    <field>: <expr>,
    ...
};
```

A `plot` declaration has:

- A **name** following `lower_snake_case` conventions (like `param` and `node`).
- A **chart type** keyword: `line`, `scatter`, `bar`, or `heatmap`.
- A **body** with named fields inside `{ }`.

### Chart Types

| Type | Description | Plotly Trace |
|------|-------------|--------------|
| `line` | Line chart for trends and time series | Scatter with lines |
| `scatter` | Scatter plot for correlation and distribution | Scatter with markers |
| `bar` | Bar chart for categorical comparison | Bar |
| `heatmap` | Heat map for 2D data visualization | HeatMap |

### Fields

Fields are named key-value pairs inside the plot body. Each field name is an
identifier and the value is an expression.

| Field | Type | Description |
|-------|------|-------------|
| `x` | Numeric expression | X-axis data (typically a `for` comprehension) |
| `y` | Numeric expression | Y-axis data (typically a `for` comprehension) |
| `title` | String literal | Chart title |

Field values can be:

- **`for` comprehensions** producing indexed numeric data.
- **Arithmetic expressions** referencing `@param` or `@node` values.
- **String literals** (for `title` and label fields).

## Examples

### Line chart

```gcl
index Time = { T0, T1, T2, T3, T4 }

node altitude: Length[Time] = { ... };

plot altitude_over_time = line {
    x: for t: Time { @altitude[t] },
    y: for t: Time { @altitude[t] },
    title: "Altitude Over Time",
};
```

### Bar chart

```gcl
index Mode = { Normal, Eco, Boost }

node power: Power[Mode] = { ... };

plot power_by_mode = bar {
    x: for m: Mode { @power[m] },
    y: for m: Mode { @power[m] },
    title: "Power by Operating Mode",
};
```

### Scatter plot

```gcl
index Maneuver = { Departure, Correction, Insertion }

node delta_v: Velocity[Maneuver] = { ... };
node mass: Mass[Maneuver] = { ... };

plot mass_vs_dv = scatter {
    x: for m: Maneuver { @delta_v[m] },
    y: for m: Maneuver { @mass[m] },
    title: "Mass vs Delta-V",
};
```

## The `#[hidden]` Attribute

By default, every `plot` declaration produces its own standalone figure in the
output. To suppress a plot's standalone figure (for example, when it only makes
sense as part of a combined figure), use the `#[hidden]` attribute:

```gcl
#[hidden]
plot curve_a = line {
    x: for t: Time { t },
    y: for t: Time { @altitude[t] },
    title: "Altitude",
};
```

A hidden plot still participates in the computation graph and can be referenced
by `figure` declarations — it simply does not appear as a standalone chart
in the output.

The `#[hidden]` attribute is only valid on `plot` declarations.

## Figure Declarations

A `figure` declaration groups multiple plots into a single combined chart with
subplots. Each referenced plot is rendered as a separate subplot in a
tiled grid layout.

```
figure <name> = {
    plots: [<plot_name>, ...],
    <field>: <expr>,
    ...
};
```

A `figure` declaration has:

- A **name** following `lower_snake_case` conventions.
- A **`plots` field** listing the plot names to combine: `plots: [a, b]`.
- Optional **fields** like `title` (a string literal).

| Field | Type | Description |
|-------|------|-------------|
| `plots` | List of plot names | Plots to include as subplots (required) |
| `title` | String literal | Figure title |

### Example

```gcl
plot curve_a = line {
    x: for s: Step { @values[s] },
    y: for s: Step { @values[s] * @values[s] },
    title: "Values Squared",
};

plot curve_b = bar {
    x: for s: Step { @values[s] },
    y: for s: Step { @values[s] + 1.0 },
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

To output only the combined figure, mark the individual plots as `#[hidden]`:

```gcl
#[hidden]
plot curve_a = line { ... };

#[hidden]
plot curve_b = bar { ... };

figure comparison = {
    plots: [curve_a, curve_b],
    title: "Combined View",
};
```

This produces **one** figure: `comparison`. The hidden plots are still evaluated
and included in the combined figure, but do not appear as standalone charts.

### Subplot Layout

Subplots are arranged in an auto-computed grid: `ceil(sqrt(n))` columns,
with rows to accommodate all plots. Each subplot gets its own x-axis and y-axis.

## Key Properties

- Plot and figure names follow `lower_snake_case`, like `param` and `node`.
- Plot bodies can reference any `@param` or `@node`, plus constants.
- Plots and figures are **leaf nodes** — no declaration can reference them with
  `@`.
- Plots participate in the dependency graph (they depend on the nodes they
  reference) but do not produce runtime values.
- Figures reference plots by name and are validated at resolution time.

## CLI Output

Plot output is controlled by the `--plot` option on `graphcal eval`:

```bash
# Open interactive chart in default browser
graphcal eval file.gcl --plot browser

# Print Plotly JSON spec to stdout
graphcal eval file.gcl --plot json
```

The JSON output is an array of figure objects, each with a `name` and `spec`:

```json
[
  { "name": "curve_a", "spec": { /* Plotly JSON */ } },
  { "name": "comparison", "spec": { /* Plotly JSON with subplots */ } }
]
```

See [CLI Reference](../cli-reference.md#plot-output) for details.
