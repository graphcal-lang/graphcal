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
| [Step 3](step3-structs-and-blocks.md) | Structs | Algebraic data types, constructor calls, field access |
| [Step 4](step4-functions.md) | DAG Blocks | Reusable computation with `dag` blocks, `include`, named arguments |
| [Step 5](step5-multi-file-projects.md) | Multi-File Projects | `import` declarations, project organization |
| [Step 6](step6-indexed-values.md) | Indexed Values | Finite indexes, `for` comprehensions, aggregations, `scan` |

## Running the Examples

All examples can be saved as `.gcl` files and run with `graphcal eval`:

```bash
graphcal eval my_file.gcl
```

## Prerequisites

Before starting, make sure you have:

- [Installed Graphcal](../installation.md)
- A text editor (ideally with [Graphcal editor support](../editor-setup.md) for inlay hints)

Ready? Let's start with [Step 1: Hello, Graphcal](step1-hello-graphcal.md).
