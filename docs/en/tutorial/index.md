---
icon: material/school
---

# Tutorial Overview

This tutorial teaches you Graphcal step by step. Each step builds on the previous one, introducing new concepts incrementally.

## What You'll Build

By the end of this tutorial, you'll have built engineering calculations that:

- Define input parameters and computed nodes in a reactive DAG
- Use physical dimensions and units with compile-time checking
- Organize data with algebraic data types
- Write reusable computation with `dag` blocks and `include`
- Split projects across multiple files
- Work with indexed collections and aggregations

## Tutorial Steps

| Step | Topic | What You'll Learn |
|------|-------|-------------------|
| [Step 1](step1-hello-graphcal.md) | Hello, Graphcal | Parameters, nodes, constants, `@`-sigil, `graphcal eval` |
| [Step 2](step2-dimensions-and-units.md) | Dimensions & Units | Physical dimensions, units, dimension annotations, unit conversion |
| [Step 3](step3-structs-and-blocks.md) | Algebraic types | Constructors, payloads, field access |
| [Step 4](step4-functions.md) | DAG Blocks | Reusable computation with `dag` blocks, `include`, named arguments |
| [Step 5](step5-multi-file-projects.md) | Multi-File Projects | `import` declarations, project organization |
| [Step 6](step6-indexed-values.md) | Indexed Values | Finite indexes, `for` comprehensions, aggregations, `scan` |

## Running the Examples

Single-file steps link to the [standalone playground](https://graphcal.org/playground/), so you can start without installing anything. Its full-size editor supports examples and shareable source URLs. The compiler and evaluator run locally in WebAssembly and do not upload your source. Step 5 teaches multi-file projects and requires the CLI.

Select **Report** to view an interactive HTML report with parameter controls,
values, plots, checks, and source provenance. Controls accept closed Graphcal
values with explicit units (for example, `36.0 km/h`). Algebraic parameters expose
a constructor selector with recursive field controls, and fixed-axis indexed
parameters expose every entry; changing a control does not edit the source.
**Auto run** is enabled by default and validates a complete parameter after a
short pause in editing. Turn it off to keep drafts unapplied until **Apply** is
selected. **Discard edits** restores the accepted snapshot, while **Raw literal**
keeps advanced whole-value entry available. Invalid inputs show errors and leave
the last successful results visible.

The adaptive input outline shows scalars and one-field records as editable
rows, with larger structures expandable on demand. Search names, paths, or
parameter descriptions, and star frequently used inputs to pin editable rows.
The **⋯** menu applies or discards the whole containing parameter.
**Advanced controls** provides metadata, sliders, and raw entry. Results scroll
independently; their tabs and output pins keep useful values in view while you
edit. Pins are session-only and are not included in shared links.
**Reset parameters** restores source defaults;
editing source clears all
parameter overrides. **Stop** cancels evaluation; **Run** retries with the last
applied parameters.

**Share** captures the source, successfully applied parameter literals, and the
selected view. A link shared from Report opens that view, but readers must press
**Run** to generate the report. Pending, rejected, or not-yet-validated restored
overrides are excluded from a new share link with a visible warning. Links contain
readable source and parameter values, not encrypted data: do not share secrets.
The 16 KiB URL limit still applies, and links use the currently deployed compiler,
not an archived compiler or immutable report.

To save projects locally or use the complete CLI and editor tooling, [install Graphcal](../installation.md) and run examples as `.gcl` files:

```bash
graphcal eval my_file.gcl
```

## Prerequisites

The browser path has no prerequisites beyond a modern browser with JavaScript and WebAssembly enabled. For local development, use a text editor with [Graphcal editor support](../editor-setup.md) for diagnostics and inlay hints.

Ready? Start with [Step 1: Hello, Graphcal](step1-hello-graphcal.md), or open the [standalone playground](https://graphcal.org/playground/).
