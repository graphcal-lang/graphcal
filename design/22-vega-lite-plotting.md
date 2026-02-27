# Vega-Lite Plotting

> Replace the current Plotly-based plotting with Vega-Lite grammar via an Altair-inspired API.

## Status

**Decision level:** Proposal. This document outlines the motivation, proposed syntax, and implementation strategy.

## Summary

The current plotting system uses a bespoke `plot` declaration with a fixed set of chart types (`line`, `scatter`, `bar`, `heatmap`) that maps directly to Plotly traces. This proposal replaces it with a **Vega-Lite grammar** expressed through an **Altair-inspired declarative API**, where the user specifies *mark* + *encoding channels* instead of chart type + raw x/y/z arrays. The renderer switches from Plotly.js to Vega-Lite (via the Vega-Embed JS library).

## Motivation

### Problems with the current approach

1. **Tight coupling to Plotly.** The `plot.rs` rendering code directly constructs Plotly `Scatter`, `Bar`, and `HeatMap` Rust objects. Changing the rendering backend requires rewriting the entire module.

2. **Unpolished API.** The current syntax (`plot name = line { x: ..., y: ... }`) hard-codes chart types as keywords and requires users to manually construct parallel x/y arrays. There is no concept of encoding channels, data types, or aesthetic mappings.

3. **No composition model.** The `figure` declaration provides basic subplot tiling, but there is no support for layering (e.g., line + point on the same axes), faceting by a data field, or concatenation with shared axes.

4. **No data type awareness.** Graphcal knows the physical dimension and unit of every value, but the current plot system strips this information and passes raw `f64` arrays to Plotly. Axis labels must be set manually.

### Why Vega-Lite?

- **Grammar of graphics.** Vega-Lite is based on Wilkinson's Grammar of Graphics (via Vega), making it a principled foundation rather than a bag of chart types.
- **Declarative JSON spec.** A Vega-Lite spec is a JSON document. Graphcal can emit this JSON without needing a Rust Plotly library — just a `serde_json::Value`.
- **Rich rendering ecosystem.** Vega-Embed renders Vega-Lite specs in any browser context. VS Code, Jupyter, and many other tools have native Vega-Lite viewers.
- **Altair proves the API works.** Altair's Python API for Vega-Lite is widely regarded as one of the best-designed visualization APIs. We can adapt its patterns to Graphcal's DSL syntax.
- **Unit-aware axes for free.** Since Graphcal knows the dimension and display unit of every value, the Vega-Lite axis titles and tooltips can be auto-populated with unit information.

## Current Implementation

### Syntax (before)

```gcl
plot my_scatter = scatter {
    x: for m: Maneuver { @delta_v[m] },
    y: for m: Maneuver { @spacecraft_mass[m] },
    title: "Mass vs Delta-V",
    x_label: "Delta-V",
    y_label: "Mass",
};

figure comparison = {
    plots: [curve_a, curve_b],
    title: "Side-by-side",
};
```

### Architecture (before)

```
  .gcl source
      │
      ▼
  Parser  ──►  PlotDecl { chart_type, fields }
      │
      ▼
  IR  ──►  PlotEntry { decl, hidden }
      │
      ▼
  Eval  ──►  PlotSpec { chart_type, fields: Vec<(String, PlotFieldValue)> }
      │
      ▼
  CLI (plot.rs)  ──►  plotly::Plot  ──►  HTML / JSON
```

The `PlotFieldValue` enum holds `Numbers(Vec<f64>)`, `Labels(Vec<String>)`, `Number(f64)`, or `String(String)`. The CLI's `plot.rs` module converts these into Plotly trace objects.

## Proposed Design

### Core Concepts from Vega-Lite / Altair

| Concept | Vega-Lite term | Graphcal term | Description |
|---------|---------------|---------------|-------------|
| **Mark** | `mark` | Mark type | The geometric primitive: `point`, `line`, `bar`, `area`, `rect`, `tick`, `arc`, `rule`, `text` |
| **Encoding** | `encoding` | Encoding block | Maps data fields to visual channels: `x`, `y`, `color`, `size`, `shape`, `opacity`, `detail`, `text`, `tooltip` |
| **Data type** | `quantitative`, `nominal`, `ordinal`, `temporal` | Inferred from Graphcal types | `f64` / dimensioned → quantitative, `index` labels → nominal/ordinal, datetime → temporal |
| **Data** | `data` | Implicit from `@`-references | Vega-Lite takes a flat table; Graphcal computes the table from for-comprehensions over indexes |
| **Layer** | `layer` | `layer` declaration | Multiple marks on the same axes |
| **Facet** | `facet` | `facet` channel | Split data into sub-plots by an index |
| **Concat** | `hconcat` / `vconcat` | `figure` (enhanced) | Tile independent charts |

### Proposed Syntax

The new `plot` declaration uses the pattern: **mark** + **encoding block**.

#### Basic single-mark plot

```gcl
plot mass_vs_dv = {
    mark: point,
    encode: {
        x: for m: Maneuver { @delta_v[m] },
        y: for m: Maneuver { @spacecraft_mass[m] },
    },
    title: "Spacecraft Mass vs Delta-V",
};
```

Key changes from the current syntax:
- The chart type keyword (`scatter`) after `=` is replaced by a `mark: <type>` field inside the block.
- Data mappings live inside an `encode: { ... }` sub-block.
- The plot body is always a `{ ... }` block (no chart type keyword before it).

#### Line chart with mark properties

```gcl
plot decay = {
    mark: line { stroke_width: 2.0 },
    encode: {
        x: for t: TimeStep { t },
        y: for t: TimeStep { @y[t] },
    },
    title: "Exponential Decay",
};
```

Mark properties (like `stroke_width`, `opacity`, `color`, `size`) are optional arguments to the mark.

#### Bar chart with automatic axis labels

```gcl
plot power_by_mode = {
    mark: bar,
    encode: {
        x: for m: OpMode { m },           // nominal (index labels)
        y: for m: OpMode { @total_power[m] }, // quantitative (Power dimension)
    },
};
```

When `x` contains index labels, Graphcal infers `nominal` type. When `y` contains dimensioned values, Graphcal infers `quantitative` and auto-generates the axis title from the dimension and display unit (e.g., "Power (W)").

#### Color encoding

```gcl
plot scatter_colored = {
    mark: point,
    encode: {
        x: for m: Maneuver { @delta_v[m] },
        y: for m: Maneuver { @spacecraft_mass[m] },
        color: for m: Maneuver { m },  // color by maneuver label
    },
};
```

#### Layered plot (multiple marks on same axes)

```gcl
#[hidden]
plot line_layer = {
    mark: line,
    encode: {
        x: for t: TimeStep { t },
        y: for t: TimeStep { @y[t] },
    },
};

#[hidden]
plot point_layer = {
    mark: point { size: 60.0 },
    encode: {
        x: for t: TimeStep { t },
        y: for t: TimeStep { @y[t] },
    },
};

layer decay_with_points = {
    plots: [line_layer, point_layer],
    title: "Decay Curve with Points",
};
```

The `layer` declaration is analogous to Altair's `alt.layer()` or the `+` operator. It overlays multiple mark layers on a single set of axes.

#### Faceted plot

```gcl
plot power_faceted = {
    mark: bar,
    encode: {
        x: for s: Subsystem, m: OpMode { @power_draw[s, m] },
        y: for s: Subsystem, m: OpMode { @power_draw[s, m] },
        facet: for s: Subsystem, m: OpMode { s },  // one subplot per subsystem
    },
};
```

The `facet` encoding channel splits the data by the given field, producing a small-multiples grid.

#### Figure (concatenation)

The existing `figure` declaration is retained for explicit concatenation of independent plots:

```gcl
figure dashboard = {
    plots: [mass_vs_dv, power_by_mode],
    title: "Mission Overview",
    columns: 2,
};
```

### Data Flow: From Graphcal to Vega-Lite JSON

The key insight is that Graphcal's for-comprehensions over indexes naturally produce **columnar data** — exactly what Vega-Lite expects.

```
  Graphcal encode block          Vega-Lite spec
  ─────────────────────          ──────────────
  x: for m: OpMode { m }    →   { "field": "x", "type": "nominal" }
  y: for m: OpMode { @p[m] } →   { "field": "y", "type": "quantitative",
                                    "axis": { "title": "Power (W)" } }

  Evaluated data:
  [
    { "x": "Safe",    "y": 10.0 },
    { "x": "Nominal", "y": 22.0 },
    { "x": "Science", "y": 55.0 }
  ]
```

The evaluator produces a flat table (array of row objects) from the for-comprehensions, plus metadata about each channel (data type, dimension, display unit). The CLI then assembles the Vega-Lite JSON spec.

### Automatic Type Inference for Encoding Channels

| Graphcal type | Vega-Lite type | Rationale |
|---------------|---------------|-----------|
| `f64` / dimensioned scalar | `quantitative` | Continuous numeric |
| `i64` | `quantitative` | Integer, treated as continuous |
| `bool` | `nominal` | Two categories |
| Index label (`OpMode::Safe`) | `nominal` | Categorical |
| Range index value (`0.0 s`, `0.5 s`, ...) | `quantitative` | Continuous steps |
| `Datetime` | `temporal` | Time axis |
| `Str` | `nominal` | Text category |

Ordinal can be specified explicitly via an attribute when needed:

```gcl
plot ordered = {
    mark: bar,
    encode: {
        x: #[ordinal] for s: Severity { s },
        y: for s: Severity { @count[s] },
    },
};
```

### Unit-Aware Axis Titles

One of Graphcal's unique advantages is full dimensional analysis. When a `quantitative` encoding channel carries a physical dimension, the axis title is auto-generated:

- Dimension `Velocity` with display unit `km/s` → axis title: **"Velocity (km/s)"**
- Dimension `Power` with display unit `W` → axis title: **"Power (W)"**
- `Dimensionless` → no unit in title

Users can override with an explicit `title` property in the encoding:

```gcl
encode: {
    x: for t: TimeStep { t } | title("Mission Elapsed Time (s)"),
    y: for t: TimeStep { @altitude[t] },
},
```

Or via a top-level `title` field on the plot.

## Implementation Plan

### Phase 1: New AST, Parser, IR (syntax change)

1. **Modify `ChartType` → `MarkType` enum.** Expand from `{Line, Scatter, Bar, Heatmap}` to `{Point, Line, Bar, Area, Rect, Tick, Arc, Rule, Text}`.

2. **Restructure `PlotDecl` AST node:**
   ```rust
   pub struct PlotDecl {
       pub name: Spanned<DeclName>,
       pub mark: MarkSpec,           // mark type + optional properties
       pub encodings: Vec<Encoding>, // channel: expr pairs
       pub properties: Vec<PlotField>, // title, width, height, etc.
   }

   pub struct MarkSpec {
       pub mark_type: MarkType,
       pub properties: Vec<(Ident, Expr)>, // stroke_width, opacity, etc.
       pub span: Span,
   }

   pub struct Encoding {
       pub channel: EncodingChannel, // x, y, color, size, shape, ...
       pub value: Expr,
       pub attributes: Vec<Attribute>, // #[ordinal], etc.
       pub span: Span,
   }
   ```

3. **Add `layer` declaration** to the parser alongside `plot` and `figure`.

4. **Update tree-sitter grammar** with new syntax rules.

5. **Update the formatter** (`graphcal-fmt`) to handle the new AST.

### Phase 2: Evaluation (Vega-Lite spec generation)

1. **Replace `PlotFieldValue` with a richer evaluated type:**
   ```rust
   pub struct EvalChannel {
       pub channel: EncodingChannel,
       pub values: ChannelValues,
       pub vega_type: VegaDataType,  // quantitative, nominal, ordinal, temporal
       pub dimension: Option<Dimension>,
       pub display_unit: Option<DisplayUnit>,
   }
   ```

2. **Produce `serde_json::Value` (Vega-Lite spec)** instead of Plotly objects. The evaluator assembles:
   - `"data": { "values": [...] }` — the flat table from for-comprehensions
   - `"mark": "point"` (or `{ "type": "point", "size": 60 }`)
   - `"encoding": { "x": { "field": "x", "type": "quantitative", "axis": { "title": "..." } }, ... }`

3. **Handle `layer` declarations** by producing a Vega-Lite `layer` spec (array of unit specs sharing the same data).

4. **Handle `figure` declarations** by producing `hconcat`/`vconcat` Vega-Lite specs.

### Phase 3: Rendering (output change)

1. **Replace the `plotly` crate dependency** with pure `serde_json` for spec construction. No Rust rendering library is needed — Vega-Lite specs are rendered client-side.

2. **HTML output:** Embed the Vega-Lite JSON spec with Vega-Embed:
   ```html
   <script src="https://cdn.jsdelivr.net/npm/vega@5"></script>
   <script src="https://cdn.jsdelivr.net/npm/vega-lite@5"></script>
   <script src="https://cdn.jsdelivr.net/npm/vega-embed@6"></script>
   <div id="vis"></div>
   <script>vegaEmbed('#vis', spec);</script>
   ```

3. **JSON output (`--plot json`):** Emit the raw Vega-Lite JSON spec. This is directly usable by any Vega-Lite viewer, VS Code Vega extension, Jupyter, etc.

4. **LSP integration:** The LSP can serve the Vega-Lite spec for inline preview in editors that support it (VS Code Vega Viewer, etc.).

### Phase 4: Advanced features (future)

- **Interactive selections** (Vega-Lite `params` with `"select"`).
- **Data transforms** in the spec (aggregate, filter, calculate) — though Graphcal's own node/fn system handles most of this.
- **Repeat** charts (Altair's `repeat()` pattern for multi-panel from same data).
- **Tooltip** with unit-formatted values.
- **Theming** via Vega-Lite config object (the Graphcal color palette can be expressed as a Vega-Lite theme).

## Migration from Current Syntax

Since the project is not yet published, a breaking change is acceptable (per CLAUDE.md guidelines). The migration is straightforward:

| Current | New |
|---------|-----|
| `plot name = line { x: ..., y: ..., title: "..." };` | `plot name = { mark: line, encode: { x: ..., y: ... }, title: "..." };` |
| `plot name = scatter { ... };` | `plot name = { mark: point, encode: { ... } };` |
| `plot name = bar { ... };` | `plot name = { mark: bar, encode: { ... } };` |
| `plot name = heatmap { x: ..., y: ..., z: ... };` | `plot name = { mark: rect, encode: { x: ..., y: ..., color: ... } };` |
| `figure name = { plots: [...] };` | `figure name = { plots: [...] };` (unchanged) |

Note: Vega-Lite heatmaps use `mark: rect` with a `color` encoding channel, not a separate `z` field.

## Comparison with Altair

| Altair (Python) | Graphcal (proposed) | Notes |
|-----------------|---------------------|-------|
| `alt.Chart(data).mark_point()` | `plot name = { mark: point, ... }` | Graphcal is declarative, not method-chained |
| `.encode(x='field:Q', y='field:Q')` | `encode: { x: expr, y: expr }` | Graphcal uses expressions, not field name strings |
| `x=alt.X('field', title='...')` | `x: expr \| title("...")` | Channel-level properties via pipe syntax (future) |
| `alt.layer(chart1, chart2)` | `layer name = { plots: [...] }` | Same concept, declaration-based |
| `chart.facet('field')` | `facet: for ... { field }` encoding | Facet as encoding channel |
| `alt.hconcat(a, b)` | `figure name = { plots: [a, b] }` | Retained from current design |
| Auto type inference (`':Q'`, `':N'`) | Inferred from Graphcal's type system | Graphcal has richer type info |

## Crate-Level Changes

| Crate | Changes |
|-------|---------|
| `graphcal-syntax` | New `MarkType` enum, restructured `PlotDecl`, new `Encoding` and `MarkSpec` AST nodes, `layer` declaration, parser updates |
| `graphcal-ir` | Updated `PlotEntry` to carry new AST structure |
| `graphcal-eval` | Replace `PlotSpec`/`PlotFieldValue` with `VegaLiteSpec` (a `serde_json::Value`). Evaluate encodings, infer Vega-Lite types, auto-generate axis titles |
| `graphcal-cli` | Replace `plot.rs` Plotly rendering with Vega-Lite JSON assembly and Vega-Embed HTML output. Remove `plotly` dependency |
| `graphcal-lsp` | Update symbol table, hover info, and document symbols for new plot syntax |
| `graphcal-fmt` | Update formatter for new plot/layer syntax |
| `graphcal-tir` | Update typed IR for new plot structure |
| `graphcal-dag` | Minor: update canvas rendering if plots are shown in DAG view |
| `tree-sitter-graphcal` | New grammar rules for `mark`, `encode`, `layer`, updated `plot_declaration` |
| `editors/vscode`, `editors/zed` | Update syntax highlighting for new keywords |

## Dependency Changes

| Remove | Add | Notes |
|--------|-----|-------|
| `plotly = "0.14.1"` | *(none — use existing `serde_json`)* | Vega-Lite specs are pure JSON; no Rust rendering library needed |
| | Vega-Embed JS (CDN link in HTML output) | Client-side rendering |

This is a net simplification: we remove a large Rust dependency (`plotly`) and replace it with a few lines of `serde_json` construction. The rendering is fully delegated to the battle-tested Vega-Embed JavaScript library.

## Open Questions

1. **Pipe syntax for channel properties.** Should we support `x: expr | title("...") | scale(zero: false)` for per-channel configuration? Or use a more verbose sub-block syntax? This can be deferred.

2. **Mark-level `encode` vs top-level `encode`.** Altair allows encoding at both levels. For simplicity, Graphcal could start with top-level only.

3. **Multi-series data.** How should multiple series (e.g., two lines on the same axes with a color legend) be expressed? Options:
   - Use `layer` with separate plots (explicit, safe).
   - Use a `color` encoding over a combined for-comprehension (more concise).

4. **Interaction and selections.** Vega-Lite supports rich interactive selections (brush, click, hover). Should Graphcal expose these? Probably deferred to Phase 4.

5. **Offline rendering.** Vega-Embed requires a browser/JS runtime. For pure CLI output (e.g., SVG to file), we could add optional server-side rendering via a bundled Deno/Node.js runtime or a Rust WASM Vega evaluator. This is a future concern.

6. **`layer` vs enhanced `figure`.** Should `layer` be a separate declaration kind, or should `figure` be enhanced to support both layering (shared axes) and concatenation (independent axes) via a mode field?

## Dependencies on Other Aspects

- **Computation Model** ([01](./01-computation-model.md)): Plot data is derived from the reactive DAG.
- **Syntax Design** ([02](./02-syntax-design.md)): The new plot syntax must be consistent with existing declaration patterns.
- **Dimensions & Units** ([04](./04-dimensions-and-units.md)): Unit-aware axis titles depend on the dimension/unit system.
- **Indexes** ([07](./07-indexes.md)): For-comprehensions over indexes produce the columnar data for Vega-Lite.
- **Tables** ([10](./10-tables-and-autofill.md)): Table data can be plotted by iterating over indexes.
- **Live View** ([13](./13-live-view.md)): Vega-Lite rendering is well-suited for the live view's web and VS Code targets.
- **Assertions & Testing** ([19](./19-assertions-and-testing.md)): The `#[hidden]` attribute for suppressing standalone plot output is retained.
