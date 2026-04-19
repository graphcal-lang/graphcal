---
icon: material/file-multiple
---

# Multi-File Projects

Graphcal supports splitting projects across multiple files using `import` and `include` declarations. Import paths can be either **file paths** (quoted strings) or **module paths** (bare identifiers).

- **`import`** brings compile-time definitions into scope: `const`, `type`, `dim`, `unit`, `index`, `dag`
- **`include`** instantiates a DAG (inline or from a file) into the current computation graph

There are two import styles: **selective imports** and **module imports**.

## Selective Imports

Selective imports bring specific names into the local scope:

```
import "./path/to/file.gcl" { name1, name2 };
```

- The path is a **string literal** relative to the importing file
- Braces list the specific names to import
- All compile-time declarations can be imported: `const node`, `dim`, `unit`, `type`, `index`, `dag`

## Module Imports

Module imports bring an entire file in as a namespace, accessed via `::`:

```
import "./constants.gcl";                  // module named "constants"
import "./constants.gcl" as consts;        // module named "consts"
```

When no alias is given, the module name is derived from the filename stem (e.g., `constants.gcl` becomes `constants`). The filename stem must be a valid `lower_snake_case` identifier; otherwise, use `as` to provide an explicit alias.

Members are accessed with `::`:

```
import "./constants.gcl";
import "./params.gcl";
import "./lib.gcl";

node g: Acceleration = @constants::g0;           // qualified const node
node total: Mass = @params::dry_mass;           // qualified graph ref
```

Module imports only resolve declarations that are actually referenced via `::` in the importing file. Unreferenced declarations are not imported.

## Path Resolution

Paths are resolved relative to the file containing the `import` declaration:

```
// In project/main.gcl:
import "./lib/constants.gcl" { g0 };     // resolves to project/lib/constants.gcl
import "../shared/units.gcl" { knot };   // resolves to shared/units.gcl
```

## Module Paths (Bare Imports)

For organized projects, you can import modules using bare identifier paths instead of quoted file paths. This requires a `graphcal.toml` manifest file at the project root.

### Setting up `graphcal.toml`

Create a `graphcal.toml` in your project root with a `[package]` section:

```toml
[package]
name = "nasa"
# source_dir = "src"  # optional, defaults to "src"
```

### Using module paths

With the manifest above, organize your source files under the `src/` directory and import them by package-qualified paths:

```
import nasa/rocket { delta_v };
import nasa/orbital/transfer as transfer;
import nasa/constants;
```

Module paths require at least two segments separated by `/`. The first segment must match the `package.name` from `graphcal.toml`.

### Resolution

`import nasa/rocket { delta_v }` resolves to `<project_root>/src/nasa/rocket.gcl`.

Nested paths work as expected: `import nasa/orbital/transfer` resolves to `<project_root>/src/nasa/orbital/transfer.gcl`.

### Custom source directory

Set `source_dir` in `graphcal.toml` to use a different directory:

```toml
[package]
name = "myproject"
source_dir = "lib"
```

Now `import myproject/helpers` resolves to `<project_root>/lib/myproject/helpers.gcl`.

### Parameterized module paths

Module paths work with all import forms, including parameterized imports:

```
import nasa/rocket(dry_mass: 800.0 kg) as stage_1;
import nasa/rocket(dry_mass: 500.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

### Mixing file paths and module paths

File paths and module paths can be used in the same project:

```
import nasa/constants { g0 };
import "./local_helpers.gcl" { HelperType };
```

### Project layout example

```
my_project/
  graphcal.toml            # [package] name = "nasa"
  src/
    main.gcl               # entry point
    nasa/
      constants.gcl        # import nasa/constants { g0 };
      rocket.gcl            # import nasa/rocket { delta_v };
      orbital/
        transfer.gcl       # import nasa/orbital/transfer { dv };
```

## Import Aliasing

Rename imports with `as` to avoid name conflicts:

```
import "./file_a.gcl" { velocity as velocity_a };
import "./file_b.gcl" { velocity as velocity_b };

node diff: Velocity = @velocity_a - @velocity_b;
```

## Visibility and Bindability

Graphcal's visibility system uses a **two-axis split**:

- **Visibility** (`pub`): whether a declaration is visible across the include / import boundary.
- **Bindability** (`pub(bind)`): whether importers may *override* it via an include or import binding.

Bindability implies visibility — every `pub(bind)` item is also `pub`.

| Annotation   | Visible? | Bindable? | Use for                                                           |
|--------------|:--------:|:---------:|-------------------------------------------------------------------|
| (none)       | no       | no        | internal helpers, private values                                  |
| `pub`        | yes      | no        | constants, derived dims / units / types consumers read but don't rewire |
| `pub(bind)`  | yes      | yes       | the library's bindable interface: required indexes / types / dims |

`param` is a special case (axiom A5): `param` declarations never carry an
annotation. Required `param` is implicitly part of the bindable interface,
and defaulted `param` is implicitly bindable-with-default. Writing `pub param`
or `pub(bind) param` is a parse error.

```
pub param dry_mass: Mass = 1200.0 kg;   // parse error — drop the `pub`
param dry_mass: Mass = 1200.0 kg;       // OK
```

### Private by default

Declarations without an annotation are private:

```
pub param dry_mass: Mass = 1200.0 kg;   // visible to importers
param internal: Mass = 500.0 kg;        // private — but bindable because
                                        // `param` is always bindable (A5)
```

Attempting to import a private non-`param` item produces error `V001`:

```
// ERROR: cannot import private item `internal_helper` from `./lib.gcl`
import "./lib.gcl" { internal_helper };
```

### Required items must be `pub(bind)`

Required `index` / `type` / `dim` declarations (no body) form the *bindable*
interface of a library — importers must supply a binding. They must therefore
be declared `pub(bind)`. Writing bare `pub` on a required item is error `V002`:

```
// ERROR: required index must be declared `pub(bind)`
pub index Phase;

// OK
pub(bind) index Phase;

// Required types and dims follow the same rule.
pub(bind) type Element;
pub(bind) dim Distance;
```

Required `param` is excluded from V002 (annotation-free; implicitly bindable).

### Private-in-public (`V003`)

A visible declaration must not reference private type-system items (`dim`,
`type`, `index`, `base dim`) in its written signature. This prevents leaking
names that importers cannot see. Violating this rule is error `V003`:

```
dim Velocity = Length / Time;
// ERROR: `pub node` `speed` references private dim `Velocity`
pub node speed: Velocity = 10.0 m/s;

// Fix: make the dim visible too.
pub dim Velocity = Length / Time;
pub node speed: Velocity = 10.0 m/s;
```

### `pub(bind)` indexes and variant literals (`V004`)

When an `index` is declared `pub(bind)`, its variant literals cannot appear
in the defining file's `node` / `const` bodies or in public sinks
(`plot` / `assert` / `figure` / `layer`). The reason: importers may rebind
the index to a different variant set, which would orphan the literal.
Either abstract over the index with `for p: I { ... }` or move the
variant-specific value into a `param`. Violating the rule is error `V004`:

```
pub(bind) index Phase = { Design, Test };
// ERROR: variant literal `Phase::Design` of `pub(bind) index` cannot be
//        used in the defining file
pub node cost: Dimensionless = if @mode == Phase::Design { 1.0 } else { 2.0 };
```

### Include overrides must reconcile (`V005`)

If an include overrides a bindable symbol `s` and some kept declaration in
the merged IR still mentions a name nominally tied to `s` (e.g. a variant
literal of an overridden `index`, a field access of an overridden `type`),
the importer must *also* re-bind that dependent declaration. Otherwise the
orphan mention has no meaning in the merged graph — error `V005`:

```
// lib.gcl
pub(bind) index Phase = { Design, Test };
pub param cost: Dimensionless[Phase] = { Phase::Design: 1.0, Phase::Test: 2.0 };

// main.gcl
pub(bind) index NewPhase = { Review, Ship };
// ERROR: include overrides index `Phase` but does not re-bind `cost`,
//        whose default mentions `Phase::Design`
include "./lib.gcl"(Phase: NewPhase);

// Fix: re-bind `cost` as well.
include "./lib.gcl"(
    Phase: NewPhase,
    cost: { NewPhase::Review: 1.0, NewPhase::Ship: 2.0 },
);
```

`dim` and `param` overrides never trigger V005: their substitution is total
(algebraic / by value) and leaves no orphan nominal mentions.

### Re-exports and generics leakage (`V006`)

A `pub include` / `pub import` re-exports the dependency's `pub` items under
the importer's namespace. If the include's bindings rename a `pub(bind)`
symbol to a name that is *private* at the importer, and that private name
appears in a re-exported signature, downstream consumers would see a
symbol they cannot name. That's error `V006`:

```
// container.gcl
pub(bind) type Element;
pub type Widget { item: Element }

// main.gcl
type Inner {}                         // private at the importer
// ERROR: re-exported type `Widget`'s signature references private type `Inner`
pub include "./container.gcl"(Element: Inner) as c;

// Fix: make the substituted name visible too.
pub type Inner {}
pub include "./container.gcl"(Element: Inner) as c;
```

### Re-export syntax

Prefix an `import` or `include` with `pub` to re-export the dependency's
`pub` items at the importer:

```
pub import "./types.gcl";                // whole-module re-export
pub import "./math.gcl" { sqrt, clamp }; // selective with every item re-exported

import "./mixed.gcl" { pub public_helper, private_helper };
//                     ^^^ only `public_helper` is re-exported
```

`pub include` behaves the same way for DAG instantiations. The
re-exported surface is subject to V006.

## What Can Be Imported

### Selective imports

`import` is restricted to compile-time items:

| Declaration | How to Import | How to Reference |
|-------------|--------------|-----------------|
| `const node` | `import "..." { name }` | `@name` |
| `dim` | `import "..." { DimName }` | `DimName` |
| `unit` | `import "..." { unit_name }` | `unit_name` |
| `type` | `import "..." { TypeName }` | `TypeName` |
| `index` | `import "..." { IndexName }` | `IndexName` |
| `dag` | `import "..." { dag_name }` | Used with `include dag_name(...)` |

Runtime values (`param`, `node`) are not imported directly. To use values from another file's DAG, use `include` with a DAG path (see [Cross-File DAG Inclusion](#cross-file-dag-inclusion)).

### Module imports

| Declaration | How to Reference |
|-------------|-----------------|
| `const node` | `@module::name` |

Dimension, unit, type, and index declarations cannot currently be referenced via module-qualified syntax. To use types or dimensions from another file, use selective imports.

### Cross-File DAG Inclusion

To instantiate a DAG from another file, use `include` with a DAG path:

```
include "./lib/orbital.gcl"/hohmann_transfer(gm: @gm_earth, r1: @r1, r2: @r2) {
    total_dv,
}
```

The syntax is `include "path"/dag_name(param: value, ...) { output as alias, ... }`.

### Inline DAG Invocation

A `dag` can also be invoked directly inside an expression:

```
dag scale {
    param factor: Dimensionless;
    param v: Length;
    node result: Length = @v * @factor;
}

param src: Length = 10.0 m;
node doubled: Length = @scale(factor: 2.0, v: @src)::result;
```

The form is `@dag_name(arg: expr, ...)::output`. Syntactically, it combines the
call-and-projection pattern of `include`/`@alias::node` into a single
expression position.

**Semantics.**

- Each syntactic call site is a fresh DAG instantiation. Two textually
  distinct occurrences with identical arguments still denote two distinct
  sub-graphs.
- The projected output after `::` is mandatory and must name a `node` of the
  called dag — there is no single-output elision.
- Arguments are evaluated in the **surrounding expression scope**, so they may
  reference local variables from an enclosing `for` comprehension, `scan`,
  `unfold`, or `match` binding:

  ```
  node distances: Length[Region] = for r: Region {
      @scale(factor: 2.0, v: @dist[r])::result
  };
  ```

  This is a deliberate divergence from top-level `include`, whose bindings
  have no access to enclosing binders.
- Inline calls may appear anywhere an expression is valid: node bodies,
  `match` arms, `if`/`else` branches, `for` / `scan` / `unfold` bodies, and as
  arguments to other inline calls (composition).

**Known limitation.** The cross-file qualified form
`@module::dag(args)::out` parses but errors at dim-check / eval with
`graphcal::G007` — only same-file (local) dag calls resolve today.
Cross-file inline dag calls compile through the project pipeline and
are planned as a follow-up.

All other MVP limitations (recursive-cycle detection, topological
ordering of dag body nodes, `pub` projection enforcement, indexed
outputs, compile-time `const node` evaluation) are now resolved by the
compile-pipeline refactor: each `dag { ... }` body is lowered through
the same `AST → IR → TIR` stages as a Graphcal file, so the regular
dim-check, cycle detection, topological execution, visibility rules,
and indexing infrastructure all apply uniformly.

## When to Use Each Style

- **Selective imports** are best when you need only a few names, or when you need to import types and dimensions
- **Module imports** are best when you import many values from a file and want to keep their origin clear

## Parameterized Imports (Module Instantiation)

You can instantiate a file with different parameter values by supplying **param bindings** in parentheses after the path. This inlines the dependency's computation graph into your file, replacing the specified param defaults with your expressions.

### Selective instantiation

```
import "./rocket.gcl"(dry_mass: 800.0 kg) { delta_v };

node result: Velocity = @delta_v;
```

Only `delta_v` is exposed to the importing file. Other declarations from `rocket.gcl` are computed internally but not directly accessible.

### Module instantiation

```
import "./rocket.gcl"(dry_mass: 800.0 kg) as r;

node dv: Velocity = @r::delta_v;
node mr: Dimensionless = @r::mass_ratio;
```

All declarations from `rocket.gcl` are accessible via the `r::` prefix.

### Multiple instantiations

The same file can be instantiated multiple times with different parameters:

```
import "./rocket.gcl"(dry_mass: 800.0 kg, isp: 320.0 s) as stage_1;
import "./rocket.gcl"(dry_mass: 500.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

Each instantiation creates an independent set of computations under its own namespace.

### Graph references in bindings

Binding expressions can reference `@` values from the importing file's scope:

```
param my_mass: Mass = 800.0 kg;
import "./rocket.gcl"(dry_mass: @my_mass) { delta_v };
```

The dependency's computation graph is merged into the importer's DAG, so the topological sort naturally handles evaluation order.

### Index Bindings

In addition to `param` bindings, parameterized imports can bind **indexes**. This enables library files that are generic over their label sets.

A library declares a [required index](indexes.md#required-indexes):

```
// lib/budget.gcl
pub(bind) index Phase;

pub param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

The importer binds it to a concrete index:

```
// main.gcl
pub index MyPhase = { Design, Build, Test };

import "./lib/budget.gcl"(
    Phase: MyPhase,
    cost: { MyPhase::Design: 10.0, MyPhase::Build: 20.0, MyPhase::Test: 5.0 },
) { total };

node result: Dimensionless = @total;  // 35.0
```

Index bindings use the same `Name: Value` syntax as param bindings, but the right-hand side must be the **name of a concrete index** (not an expression).

#### Kind matching

Named indexes can only be bound to named indexes, and range indexes can only be bound to range indexes. Binding a named index to a range or vice versa is a compile error.

#### Dimension matching for range indexes

When binding a required range index, the concrete range index must have the **same dimension**:

```
// lib.gcl
pub(bind) index Step: Time;   // requires dimension Time

// main.gcl
index MyStep = linspace(0.0 s, 10.0 s, step: 1.0 s);   // OK: dimension is Time
import "./lib.gcl"(Step: MyStep) { ... };

index DistStep = linspace(0.0 m, 100.0 m, step: 10.0 m);  // ERROR: dimension is Length
import "./lib.gcl"(Step: DistStep) { ... };     // dimension mismatch
```

### Strict Binding Mode

When a parameterized import has **any** bindings (param or index), **all** params and indexes with defaults must be explicitly bound. This prevents accidentally relying on stale default values when you intend to customize the module. Required indexes must **always** be bound, regardless of `#[allow_defaults]`.

```
// rocket.gcl has params: dry_mass (default), fuel_mass (default), isp (default)

// ERROR: only dry_mass is bound; fuel_mass and isp are not explicitly provided
import "./rocket.gcl"(dry_mass: 800.0 kg) as r;

// OK: all params are explicitly bound
import "./rocket.gcl"(dry_mass: 800.0 kg, fuel_mass: 2800.0 kg, isp: 320.0 s) as r;
```

#### Opting out with `#[allow_defaults]`

If you intentionally want to bind only some params and let the rest use their defaults, add the `#[allow_defaults]` attribute to the import:

```
#[allow_defaults]
import "./rocket.gcl"(dry_mass: 800.0 kg) as r;
// fuel_mass and isp keep their default values
```

The `#[allow_defaults]` attribute is only valid on `import` declarations with param bindings. Using it on other declarations (e.g., `param`, `node`) is a compile error.

### Required parameters

A param declared without a default value is **required** — it must be provided by the importer via a parameterized import binding. This is the primary mechanism for creating reusable "library" files:

```
// library: rocket_engine.gcl
pub param dry_mass: Mass;                     // required — must be provided
pub param isp: Velocity = 320.0 s;           // optional — has default

node v_exhaust: Velocity = @isp * @g0;
node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
```

```
// consumer: main.gcl
import "./rocket_engine.gcl"(dry_mass: 800.0 kg) as engine;

node dv: Velocity = @engine::delta_v;
```

If a required param is not provided, the compiler emits error `O003`:

```
error[graphcal::O003]: required param `dry_mass` has no value
  ┌─ rocket_engine.gcl:2:1
  │
2 │ param dry_mass: Mass;
  │ ^^^^^^^^^^^^^^^^^^^^^ declared here without a default value
  │
  = help: provide a value via `--set 'dry_mass=<value>'`, `--input`,
          or a parameterized import binding
```

Required params can also be satisfied from the command line with `--set` or `--input`, which is useful for top-level entry-point files.

### Validation

- Binding names must be `param` or index (`index`) declarations in the imported file
- Binding a `node`, `const node`, or unknown name is a compile error
- All required params (those without defaults) must be provided by bindings, `--set`, or `--input`
- All required indexes must be provided by bindings
- Index binding values must be the name of a concrete index in the importer's scope
- Named indexes can only be bound to named indexes; range indexes can only be bound to range indexes
- Range index dimensions must match between the required index and the bound index
- When any binding is provided, all params and indexes with defaults must be bound, unless `#[allow_defaults]` is present
- Dimension mismatches are caught by the normal dimension checker after merging

## Circular Import Detection

Graphcal detects circular imports at compile time:

```
// a.gcl
import "./b.gcl" { x };

// b.gcl
import "./a.gcl" { y };
// ERROR: circular import detected
```

## Project Organization Patterns

### Constants / Parameters / Main

A common pattern separates constants, parameters, and main logic:

```
project/
  constants.gcl   -- shared physical constants
  params.gcl      -- tunable input parameters
  main.gcl        -- computation graph
```

### Library / Application

For reusable DAG blocks:

```
project/
  lib/
    orbital.gcl   -- reusable orbital mechanics DAG blocks
    thermal.gcl   -- thermal analysis DAG blocks
  main.gcl        -- application-specific graph
```

### Reusable Templates with Required Parameters

Use required params to create library files that must be instantiated with specific values:

```
project/
  lib/
    rocket.gcl    -- template with required params (dry_mass, fuel_mass)
  main.gcl        -- instantiates rocket.gcl with different param values
```

```
// lib/rocket.gcl
pub param dry_mass: Mass;     // required
pub param fuel_mass: Mass;    // required
pub param isp: Time = 320 s;  // optional default

pub const node g0: Acceleration = 9.80665 m/s^2;
pub node v_exhaust: Velocity = @isp * @g0;
pub node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
pub node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```
// main.gcl
import "./lib/rocket.gcl"(dry_mass: 800.0 kg, fuel_mass: 2000.0 kg, isp: 320.0 s) as stage_1;
import "./lib/rocket.gcl"(dry_mass: 500.0 kg, fuel_mass: 1200.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1::delta_v + @stage_2::delta_v;
```

### Reusable Templates with Required Indexes

Use required indexes to create library files that are generic over their label sets:

```
project/
  lib/
    budget.gcl    -- template with required index (Phase) and required param (cost)
  main.gcl        -- instantiates budget.gcl with a concrete Phase index
```

```
// lib/budget.gcl
pub(bind) index Phase;

pub param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

```
// main.gcl
pub index ProjectPhase = { Design, Build, Test };

import "./lib/budget.gcl"(
    Phase: ProjectPhase,
    cost: { ProjectPhase::Design: 10.0, ProjectPhase::Build: 20.0, ProjectPhase::Test: 5.0 },
) { total };

node project_cost: Dimensionless = @total;
```

## Assertions in Imported Files

When a file is imported, **all its assertions are automatically evaluated and
reported**, regardless of whether they are explicitly listed in the import. This
ensures that safety checks in library files are never silently skipped.

To use an imported assertion in `#[assumes(...)]`, you must import it by name.
See [Assertions](assertions.md#assertions-in-multi-file-projects) for details.

## Evaluation Entry Point

When running `graphcal eval`, the entry file is the one you pass on the command line. All `import` dependencies are resolved transitively from that file:

```bash
graphcal eval project/main.gcl
```

## Project Root and Import Sandboxing

Graphcal restricts imports to files within the **project root** directory tree. This prevents accidental or malicious access to files outside your project.

### Default behavior

By default, the project root is the **parent directory of the entry-point file**. A file can import siblings and descendants, but not files above its own directory:

```
workspace/
  shared/
    constants.gcl
  project/
    main.gcl        ← entry point; project root = project/
```

```bash
# In main.gcl, this import would FAIL:
# import "../shared/constants.gcl" { g0 };
# ERROR: import resolves outside the project root
```

### Widening the root with `graphcal.toml`

Place a `graphcal.toml` file in an ancestor directory to widen the project root to that directory. A `graphcal.toml` with a `[package]` section also enables [module paths](#module-paths-bare-imports). An empty `graphcal.toml` (without `[package]`) only widens the root:

```
workspace/
  graphcal.toml     ← project root marker
  shared/
    constants.gcl
  project/
    main.gcl        ← entry point
```

Now `main.gcl` can `import` from `../shared/constants.gcl` because the project root is `workspace/` (the directory containing `graphcal.toml`), not `project/`.

Graphcal searches upward from the entry-point file's directory for the nearest `graphcal.toml`. If none is found, the default behavior applies.

### Explicit root with `--root`

You can also set the project root explicitly on the command line:

```bash
graphcal eval --root ./workspace project/main.gcl
```

When `--root` is provided, automatic `graphcal.toml` discovery is skipped — the given directory is used directly. This is useful in CI/scripts or when placing a `graphcal.toml` file is inconvenient.

The `--root` flag is available for both `eval` and `check` subcommands.
