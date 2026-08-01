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

Each step includes an editable browser playground, so you can start without installing anything. The compiler and evaluator run locally in WebAssembly and do not upload your source.

To save projects locally or use the complete CLI and editor tooling, [install Graphcal](../installation.md) and run examples as `.gcl` files:

```bash
graphcal eval my_file.gcl
```

## Prerequisites

The browser path has no prerequisites beyond a modern browser with JavaScript and WebAssembly enabled. For local development, use a text editor with [Graphcal editor support](../editor-setup.md) for diagnostics and inlay hints.

Ready? Start with [Step 1: Hello, Graphcal](step1-hello-graphcal.md), or open the [standalone playground](../playground.md).
