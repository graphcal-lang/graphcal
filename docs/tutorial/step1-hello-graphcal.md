---
icon: material/numeric-1-circle
---

# Step 1: Hello, Graphcal

In this step, you'll create your first Graphcal file and learn the three core declaration kinds.

## The Spreadsheet Analogy

If you've used spreadsheets, you already understand Graphcal's core model:

| Spreadsheet | Graphcal | Description |
|-------------|----------|-------------|
| Input cell | `param` | A value you provide, can be overridden |
| Formula cell | `node` | A computed value derived from other values |
| Named constant | `const node` | A fixed value that never changes |

## Your First File

Create a file `mass_budget.gcl`:

```
param dry_mass: Dimensionless = 1200.0;
param fuel_mass: Dimensionless = 2800.0;
const node margin_factor: Dimensionless = 1.1;

node total_mass: Dimensionless = @dry_mass + @fuel_mass;
node mass_with_margin: Dimensionless = @total_mass * @margin_factor;
node mass_ratio: Dimensionless = @total_mass / @dry_mass;
```

## Run It

```bash
$ graphcal eval mass_budget.gcl
dry_mass         = 1200
fuel_mass        = 2800
margin_factor    = 1.1
total_mass       = 4000
mass_with_margin = 4400
mass_ratio       = 3.333333
```

## Understanding the Code

### Parameters (`param`)

Parameters are inputs to your calculation. They have default values but can be overridden from the command line:

```
param dry_mass: Dimensionless = 1200.0;
```

- **`param`** -- declares an input parameter
- **`dry_mass`** -- the name (must be `lower_snake_case`)
- **`: Dimensionless`** -- the dimension annotation (`Dimensionless` means a plain number)
- **`= 1200.0`** -- the default value

### Constants (`const node`)

Constants are fixed values known at compile time:

```
const node margin_factor: Dimensionless = 1.1;
```

- **`const node`** -- declares a compile-time constant
- **`margin_factor`** -- the name (must be `lower_snake_case`)

### Nodes (`node`)

Nodes are computed values that form the reactive computation graph:

```
node total_mass: Dimensionless = @dry_mass + @fuel_mass;
```

- **`node`** -- declares a computed value
- **`@dry_mass`** -- references a parameter or node using the `@` sigil

### The `@` Sigil

The `@` prefix is how you reference values in the computation graph:

- `@name` references a `param`, `node`, or `const node`
- Bare `NAME` references a built-in constant (`PI`, `E`, `TAU`, etc.)
- The `@` sigil makes it visually clear which values participate in the computation

## Overriding Parameters

You can override parameter values from the command line using `--set`. When you use `--set`, you must provide values for **all** parameters (or use `--allow-defaults` to keep unspecified defaults):

```bash
$ graphcal eval mass_budget.gcl --set 'dry_mass=1200.0' --set 'fuel_mass=3500.0'
dry_mass         = 1200
fuel_mass        = 3500
margin_factor    = 1.1
total_mass       = 4700
mass_with_margin = 5170
mass_ratio       = 3.916667
```

All downstream nodes (`total_mass`, `mass_with_margin`, `mass_ratio`) update automatically.

## Naming Conventions

Graphcal enforces naming conventions at parse time:

| Declaration | Convention | Example |
|-------------|-----------|---------|
| `param`, `node`, `const node`, `dag` | `lower_snake_case` | `dry_mass`, `total_dv`, `margin_factor` |
| `type`, `index`, `dim` | `PascalCase` | `TransferResult`, `Maneuver` |

## What You Learned

- **`param`** for input values that can be overridden
- **`node`** for computed values in the reactive graph
- **`const node`** for compile-time constants
- **`@`** sigil to reference graph values
- **`--set`** to override parameters from the command line

## Next Step

In [Step 2](step2-dimensions-and-units.md), you'll add physical dimensions and units to catch unit mismatches at compile time.
