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

## Key Properties

- Plot names follow `lower_snake_case`, like `param` and `node`.
- Plot bodies can reference any `@param` or `@node`, plus constants.
- Plots are **leaf nodes** -- no declaration can reference a plot with `@`.
- Plots participate in the dependency graph (they depend on the nodes they
  reference) but do not produce runtime values.

## CLI Output

Plot output is controlled by the `--plot` option on `graphcal eval`:

```bash
# Open interactive chart in default browser
graphcal eval file.gcl --plot browser

# Print Plotly JSON spec to stdout
graphcal eval file.gcl --plot json
```

See [CLI Reference](../cli-reference.md#plot-output) for details.
