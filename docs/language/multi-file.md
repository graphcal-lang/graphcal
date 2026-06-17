---
icon: material/file-multiple
---

# Multi-File Projects

Graphcal organizes code into **packages**. Every `.gcl` file belongs to
exactly one package, and every name a DAG uses is reached by an absolute
path that starts at a package root. The path syntax is the same whether
the package lives on disk under a manifest or whether it is a single
file you just created — the only thing that changes between those cases
is the externally-visible package name.

Two declarations bring outside material into a DAG:

- **`import`** brings *names* (compile-time references: `type`, `dim`,
  `unit`, `const node`, `dag`, `index`) into the local
  scope. Imports never instantiate anything.
- **`include`** *instantiates* a DAG with parameter bindings and embeds
  it as a sub-graph, exposing its outputs as nodes.

Both use the same path discipline: dot-separated segments, absolute from
a package root. There are no relative paths, no `..`, no quoted file
strings, and no `/` inside Graphcal source.

## Files Are Packages

A package has a name and a tree of modules. The name comes from one of
two places:

| Flavor       | Name source                          | When                                                                  |
|--------------|--------------------------------------|-----------------------------------------------------------------------|
| **Virtual**  | The file's stem (the `.gcl` filename without extension) | No `graphcal.toml`, *or* the file lives outside the manifest's package namespace |
| **Real**     | `package.name` in `graphcal.toml`    | The file lives at `<source_dir>/<package_name>.gcl` or under `<source_dir>/<package_name>/` |

A **virtual package is a single file** — a standalone Graphcal script.
The package consists of exactly one module: the file itself. The only
import path that resolves in a virtual package is the file's own stem
(see [Self-Reference](#self-reference-a-file-is-its-own-package)
below). There are no sibling-file imports from a virtual-package file;
the loader rejects `import helper.{X};` with a structured error pointing
you at [Promoting to a Real Package](#promoting-to-a-real-package).

This applies even when a `graphcal.toml` sits next to the file. A
manifest only "claims" files that live inside the package namespace
(`<source_dir>/<package_name>.gcl` or under
`<source_dir>/<package_name>/`); a loose `.gcl` sibling of the manifest
is still a virtual-package script with no cross-file import privileges.
There is exactly one rule: to import across files, the file must be in
its package's namespace directory.

A **real package** can span many files arranged in a directory tree
under `source_dir`. Resolution walks `<source_dir>/<segments>.gcl`
exactly as the path is written.

A path like `nasa.rocket.dynamics` walks the tree starting at the
package root: package `nasa` → directory `rocket` → file `dynamics.gcl`
(or an inline `dag dynamics { ... }` declared inside `rocket.gcl`). The
path before any `.{...}` or `as` clause **always names a module**, never
a symbol — the parser knows the module/symbol boundary from syntax
alone.

### File and directory names

Every `.gcl` filename stem and every directory below the source root
must be a valid Graphcal identifier (snake_case, no hyphens, no spaces,
not a keyword). The compiler rejects files like `match.gcl`,
`my-helpers.gcl`, or `MyModule.gcl` outright; the file name *is* a path
segment in `import` / `include` declarations and must therefore be
syntactically usable there.

## The `import` Form

`import` brings names into the current DAG's scope. There are three
surface forms; each form introduces **exactly the names you write
down** — no implicit additions.

```graphcal
import nasa.rocket;                                  // bare: brings `rocket`
import nasa.rocket as nr;                            // alias: brings `nr`
import nasa.rocket.{type Orbit, compute_thrust as ct}; // brace: brings `Orbit` and `ct` only
```

The forms differ in what enters scope:

| Form                                                   | Names added                                |
|--------------------------------------------------------|--------------------------------------------|
| `import nasa.rocket;`                                  | `rocket` (the module, by its leaf name)    |
| `import nasa.rocket as nr;`                            | `nr` (the module under alias)              |
| `import nasa.rocket.{type Orbit};`                     | `Orbit` only — **not** `rocket`            |
| `import nasa.rocket.{type Orbit, compute_thrust as ct};` | `Orbit` and `ct` only                    |
| `import nasa.rocket as nr.{type Orbit};`               | parse error — alias and brace mutually exclusive |

If you want both the module qualifier *and* an unqualified item, write
two statements:

```graphcal
import nasa.rocket;            // brings: rocket
import nasa.rocket.{type Orbit}; // brings: Orbit
// Now both `rocket.Orbit` and `Orbit` are usable.
```

This is a deliberate divergence from Gleam: Graphcal's brace form does
**not** also bring the module leaf. Each `import` should name exactly
what enters scope so a reader scanning the import list sees the precise
set of names introduced.

### Type imports use `type`

Graphcal has separate namespaces for type names and constructors. To
import a type name selectively, prefix the item with `type`. Bare items
target the default compile-time namespace:

```graphcal
import nasa.rocket.{type Orbit, Length, m, MAX_THRUST, Maneuver};
//                       type   dim     unit  const       index
```

This is required even when the type and its constructor share the same
spelling. To import both namespaces, write both items:

```graphcal
import school.records.{type Student, Student};
```

### Aliasing items

Each item in a brace list may be aliased independently:

```graphcal
import nasa.rocket.{type Orbit as O, compute_thrust as ct};
```

### What `import` may bring

Only compile-time names cross the `import` boundary:

| Declaration kind     | Reference after import                 |
|----------------------|----------------------------------------|
| `const node`         | `@name`                                |
| `dim`                | `DimName`                              |
| `unit`               | `unit_name`                            |
| `type`               | `TypeName`                             |
| `index`              | `IndexName`                            |
| `dag`                | Used with `include` or `@name(args).out` |

Runtime values — non-`const` `node` and any `param` — are **not**
importable. To consume runtime values from another file, instantiate
the producing DAG via `include` (see [The `include` Form](#the-include-form)).
Plots are likewise not importable: they are runtime sinks evaluated against
an instance, so naming one in an `import` brace list is an error — request
it through an `include` brace list instead (see
[Cross-File Plots](plots.md#cross-file-plots)).

Imported module aliases are first-class qualifiers in type, expression, and
unit positions. If two imports export the same leaf name, write the
module-qualified path to select the owner explicitly:

```graphcal
import collide.a as a;
import collide.b as b;

const node gain: Dimensionless = a.bias;
node phase_score: Dimensionless[a.Phase] = for phase: a.Phase {
    match phase {
        a.Phase.Burn => 1.0,
        a.Phase.Coast => 2.0,
    }
};
node result: a.Item = a.Pick(distance: 2.0 m);
node span: Length = 2.0 a.mile;
node span_miles: Length = @span -> a.mile;
```

The compiler resolves those paths to the canonical exported item; it does not
merge same-leaf types, indexes, constructors, or labels from different modules.

### `pub import` — re-export

Prefixing an `import` with `pub` re-exports the imported names under
the importer's namespace:

```graphcal
pub import nasa.rocket;                              // re-exports `rocket`
pub import nasa.rocket.{type Orbit, compute_thrust}; // re-exports both items
```

The brace form also supports per-item `pub`, for fine-grained
re-export:

```graphcal
import nasa.rocket.{ pub type Orbit, compute_thrust };
//                   ^^^ only `Orbit` is re-exported
```

Mixing whole-import `pub` with per-item `pub` is rejected:

```graphcal
pub import nasa.rocket.{ pub type Orbit };   // parse error
```

`pub(bind)` is illegal on `import`. Imports name use-sites, and
use-sites are not bindable — `pub(bind)` belongs only on declarations.

## The `include` Form

`include` instantiates a DAG, embedding its body as a sub-graph and
exposing its outputs as nodes in the surrounding DAG. The argument list
in parentheses is **mandatory** (it may be empty), which makes
`include` syntactically distinct from `import`.

```graphcal
// Bare: leaf becomes the alias; outputs accessed as @compute_thrust.<output>
include nasa.rocket.compute_thrust(orbit: @o, dry_mass: 800.0 kg);
node t: Force = @compute_thrust.thrust;

// Aliased: outputs accessed as @ct.<output>
include nasa.rocket.compute_thrust(orbit: @o) as ct;
node t: Force = @ct.thrust;

// Brace list: selects (and optionally renames) outputs as direct nodes
include nasa.rocket.compute_thrust(orbit: @o).{ thrust };
include nasa.rocket.compute_thrust(orbit: @o).{ thrust, isp, mass_flow as mdot };
node t: Force = @thrust;
```

| Form                                                            | Result                                                |
|-----------------------------------------------------------------|-------------------------------------------------------|
| `include path.dag(args);`                                       | Sugar for `... as dag` — leaf name is the alias       |
| `include path.dag(args) as a;`                                  | Outputs reached as `@a.<output>`                      |
| `include path.dag(args).{x};`                                   | `x` itself becomes a node in the current DAG          |
| `include path.dag(args).{x as y};`                              | Same, renamed                                         |
| `include path.dag(args) as a.{x};`                              | parse error — alias and brace mutually exclusive      |

Across module boundaries, the DAG named by `path.dag` must be `pub`, and
each output selected through a brace list or reached through an include alias
must be public. Renaming an output does not change its visibility. Private
nodes inside the included DAG remain implementation details.

A brace-list item may also name a `pub plot` of the included file: that is
the consumer's request to display the library's chart of this instance. The
plot enters the root namespace under its local alias; `#[hidden]` on the
item includes it for figure/layer composition only. See
[Cross-File Plots](plots.md#cross-file-plots).

### `include` does not require `import` of the DAG

Because the include path is absolute from the package root, no preceding
`import` is needed for the DAG itself. The DAG's outputs (named in the
brace list, or by alias) are the only names introduced. **Types** in
the param interface, however, must still be brought into scope by
`import`:

```graphcal
dag mission {
    import nasa.rocket.{type Orbit};          // type for the param
    param o: Orbit;

    include nasa.rocket.compute_thrust(orbit: @o, dry_mass: 800.0 kg).{ thrust };

    node total: Force = @thrust + 100.0 N;
}
```

### `pub include` — re-export

A leading `pub` on `include` re-exports the included outputs from the
including DAG:

```graphcal
pub include nasa.rocket.compute_thrust(orbit: @o).{ thrust };
```

### Inline-DAG call expression

Inside an expression, `@dag(args).out` is sugar for an anonymous
`include ... as <synthetic>; @<synthetic>.out`. What `@` enforces is
that the post-`@` expression must denote a *node* — and `dag(args).out`
does, because `out` is a node belonging to the DAG instance `dag(args)`.

```graphcal
dag mission {
    import nasa.rocket.{compute_thrust, type Orbit};
    param o: Orbit;
    node t: Force = @compute_thrust(orbit: @o, dry_mass: 800.0 kg).thrust;
}
```

Each call site is a fresh instantiation, and the dag's `assert`
declarations are checked per instantiation just like the `include`
path. Because an expression has no reporting surface, a failing (or
erroring) assert fails the calling expression itself — the calling
node reports an evaluation error such as ``assertion `v_positive`
failed in inline call of dag `checked` (assertion evaluated to
false)``, fault-isolated to that node. `#[expected_fail]` inversion
applies as usual: an expected failure is not an error, and an
unexpected pass is.

Arguments are evaluated in the surrounding expression scope, so they
may reference local variables from an enclosing `for`, `scan`,
`unfold`, or `match` binding:

```graphcal
node distances: Length[Region] = for r: Region {
    @scale(factor: 2.0, v: @dist[r]).result
};
```

#### Qualified form: `@module.dag(args).out`

Inline calls also accept a module-qualified path, mirroring the way a
DAG comes into scope via `import` without needing a `{dag}` brace list.
After `import nasa.rocket as rocket;` (or unaliased `import nasa.rocket;`,
which binds the leaf `rocket` as the module name) you can write:

```graphcal
import nasa.rocket as rocket;

param o: Orbit;
node t: Force = @rocket.compute_thrust(orbit: @o, dry_mass: 800.0 kg).thrust;
```

The semantics are identical to the bare form — `compute_thrust(args).thrust`
is still a node, and prefixing the path with the in-scope module alias
just adds a qualifier. The "post-`@` expression must denote a node"
rule is unchanged; it has nothing to do with how many segments appear
before the `(`.

The projected output must be a public node of the called DAG. For
module-qualified calls, the called DAG itself must also be public.

What *is* still rejected is dropping the projection: `@dag(args)` and
`@module.dag(args)` (without the trailing `.<out>`) are both parse
errors. A DAG instance with no projection is not a node, and that's
the property `@` requires.

## Inline DAGs as Modules

A `dag` declaration inside another module — whether at file top level
or nested inside another DAG — is itself a module, addressable by
extending the path:

```graphcal
// orbit_analysis.gcl  (virtual package: orbit_analysis)
dag analyze {
    type IntermediateResult { IntermediateResult(value: Length) }

    dag deeper {
        import orbit_analysis.analyze.{type IntermediateResult};
        param r: IntermediateResult;
        // ...
    }
}
```

The path `orbit_analysis.analyze.deeper` reads: package
`orbit_analysis`, sub-module `analyze`, sub-module `deeper`. Identical
addressing rule as cross-package `nasa.rocket.compute_thrust`.

`import` and `include` declarations inside an inline DAG are real
project dependency edges. They load dependencies, import values and
compile-time names with the same visibility checks as file-root
imports, and are honored whether the DAG is consumed with
`include dag(args)` or as `@dag(args).out`.

Sibling top-level DAGs are addressed the same way:

```graphcal
// orbit_analysis.gcl
dag double {
    param x: Length;
    node y: Length = @x * 2.0;
}

dag analyze {
    param input_dist: Length;
    include orbit_analysis.double(x: @input_dist).{ y as doubled };
    node final: Length = @doubled + 1.0 m;
}
```

### Recursive parent-DAG include

An inline DAG may `include` its enclosing DAG by full path. This is
recursive instantiation: the source-level grammar accepts it, but the
evaluator currently emits `NotYetImplemented`. A future implementation
will require recursion to terminate (via diverging param values).

## Self-Reference: A File Is Its Own Package

To reach a top-level declaration of the *current file* from inside an
inline DAG, use the file's own package address. There is no relative
shortcut, no `super`, no `..`.

In a virtual package, the file stem is the package name:

```graphcal
// dynamics.gcl  (virtual package: dynamics)
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }
const node earth_mu: GravParam = 3.986e5 km^3/s^2;

dag analyze {
    dag energy {
        import dynamics.{type OrbitType, earth_mu};   // file's own name
        param o: OrbitType;
        node e: SpecificEnergy = -@earth_mu / (2.0 * @o.sma);
    }
}
```

In a real package, the same reference uses the full package path:

```graphcal
// On disk: src/nasa/rocket/dynamics.gcl
// Source address: nasa.rocket.dynamics
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }

dag analyze {
    dag energy {
        import nasa.rocket.dynamics.{type OrbitType};
        param o: OrbitType;
        // ...
    }
}
```

Note: `/` appears in the on-disk filesystem path (a tooling concern),
never in Graphcal source.

## Strict Isolation

Inline DAG bodies see **only** their own declarations, their own
`import`s, and the outputs of their own `include`s. There is no
lexical inheritance from the enclosing file's top-level scope, and no
inheritance from an enclosing DAG body. Every name a DAG uses must
either be declared inside it or imported by it explicitly.

```graphcal
// dynamics.gcl
type OrbitType { OrbitType(sma: Length, ecc: Dimensionless) }

dag analyze {
    // ERROR: `OrbitType` is not visible here without an import.
    param o: OrbitType;
}

dag analyze_ok {
    import dynamics.{type OrbitType};
    param o: OrbitType;
}
```

This rule is uniform across every DAG — top-level or inline. It is the
same isolation guarantee that makes file-based and inline DAGs
interchangeable: same name resolution, same scoping, same dependency
visibility.

## Promoting to a Real Package

A real package is announced by a `graphcal.toml` manifest. Create it at
the project root:

```toml
[package]
name = "nasa"
# source_dir = "src"  # optional, defaults to "src"
```

Lay out source under `<source_dir>/<package_name>/`:

```text
my_project/
  graphcal.toml            # [package] name = "nasa"
  src/
    nasa/
      constants.gcl
      rocket.gcl
      orbital/
        transfer.gcl
```

The files are now addressed as:

```graphcal
import nasa.constants.{g0};
import nasa.rocket.{type Orbit, compute_thrust};
import nasa.orbital.transfer.{dv};
```

### Migrating self-references

When a virtual package is promoted, every file's self-reference must
be rewritten from the bare file stem to the full package path:

```graphcal
// Before (virtual package `dynamics`):
import dynamics.{type OrbitType};

// After (real package `nasa`, file at src/nasa/rocket/dynamics.gcl):
import nasa.rocket.dynamics.{type OrbitType};
```

The LSP rename refactor handles the mechanical part of this rewrite.

### Custom source directory

Override `source_dir` to point elsewhere:

```toml
[package]
name = "myproject"
source_dir = "lib"
```

Now `import myproject.helpers` resolves to
`<project_root>/lib/myproject/helpers.gcl`.

## Package Dependencies

A real package may depend on other Graphcal packages fetched from Git. The MVP
supports source-only Git dependencies pinned to exact commits:

```toml
[package]
name = "mission"
source_dir = "src"

[dependencies]
orbital = {
    git = "https://github.com/acme/orbital.git",
    rev = "0123456789abcdef0123456789abcdef01234567",
}
```

Rules:

- `rev` is required and must be a full 40-character commit hash.
- Branches, tags, version ranges, `latest`, registries, archives, publishing,
  and implicit package discovery inside a Git repository are not part of this
  MVP.
- Public Git repositories are expected to work with ordinary HTTPS or SSH Git
  URLs. Private repositories require whatever credentials the underlying Git
  fetch implementation can obtain in the current environment, so support is
  environment-dependent. For example, SSH may work when an SSH key/agent is
  available, while HTTPS may fail unless a compatible credential helper or
  non-interactive credential provider is configured. Graphcal manifests must not
  embed credentials in dependency URLs.
- The dependency table key is the source-visible dependency name used in
  `import` and `include` paths.
- If `package` is omitted, the fetched package's `[package].name` must match
  the dependency key.
- If `package` is present, the dependency key is a local alias and `package`
  names the fetched package's real package name.

```toml
[dependencies]
units_v1 = { package = "units", git = "https://github.com/acme/units.git", rev = "1111111111111111111111111111111111111111" }
units_v2 = { package = "units", git = "https://github.com/acme/units.git", rev = "2222222222222222222222222222222222222222" }
```

Source resolves those aliases explicitly:

```graphcal
import units_v1.si.{m as m_v1};
import units_v2.si.{m as m_v2};
```

Run `graphcal deps lock` before checking or evaluating a package with
dependencies. The command writes `graphcal.lock`, a deterministic,
tool-maintained lockfile that records package instances, exact Git commits,
source tree hashes, direct dependency edges, and the Graphcal/standard-library
versions used for resolution. The lockfile records dependency graph edges
between package instances; it is not a flat "one version per package name" map,
so multiple revisions of the same package can coexist.

`graphcal check`, `graphcal eval`, `graphcal graph`, and the LSP read only the
lockfile and locally materialized cache entries. They do not fetch dependencies
or update `graphcal.lock`; if the lockfile is missing, stale, version-mismatched,
or points at a missing or hash-mismatched cached source, they fail and ask you
to run `graphcal deps lock`.

### Dependency Visibility

Dependency names are local to the package instance currently being compiled.
When package `mission` imports `orbital.constants`, Graphcal interprets the
first segment in `mission`'s direct dependency namespace:

```text
current package: pkg-mission
first segment: orbital
mission.dependencies[orbital] = pkg-orbital
remaining path: constants
=> pkg-orbital/src/orbital/constants.gcl
```

A package may resolve only its own package name and its own direct dependency
aliases. Transitive dependencies are not implicitly visible. If `mission`
depends on `orbital`, and `orbital` depends on `units`, source in `mission`
cannot write `import units...` unless `mission` also declares a direct `units`
dependency or alias. `orbital` may still expose values, dimensions, units,
types, indexes, or DAGs derived from `units` through its public API; `mission`
then names those through `orbital`, preserving their original package-instance
identity.

Package-defined dimensions, units, types, indexes, and algebraic types are
nominal by package instance. Two locked package instances with the same package
name and same source spelling are distinct if their Git commit, source identity,
or dependency graph differs. Standard-library dimensions and units are the
exception: they are singleton identities tied to the active Graphcal toolchain
and standard-library version recorded in `graphcal.lock`.

## Stdlib Reservation: `graphcal` and `std`

The first segments `graphcal` and `std` are reserved for Graphcal's
standard library. User packages may not be named `graphcal` or `std`,
and user source may not begin a path with either segment except to
import from the stdlib:

```graphcal
import std.math.{sin, cos};   // (reserved) — stdlib import
```

The standard library itself is still being designed; references in
user code are rejected with a "stdlib not yet available" diagnostic
unless the project opts into the experimental stdlib explicitly.

## Visibility and Bindability

Graphcal's visibility system uses a **two-axis split**:

- **Visibility** (`pub`): whether a declaration is visible across the
  include / import boundary.
- **Bindability** (`pub(bind)`): whether importers may *override* it
  via an include or import binding. Bindability implies visibility.

| Annotation   | Visible? | Bindable? | Use for                                                                 |
|--------------|:--------:|:---------:|-------------------------------------------------------------------------|
| (none)       | no       | no        | internal helpers, private values                                        |
| `pub`        | yes      | no        | constants, derived dims / units / types consumers read but don't rewire |
| `pub(bind)`  | yes      | yes       | the library's bindable interface: required indexes / types / dims       |

`param` is a special case (axiom A5): `param` declarations never
carry an annotation. Required `param` is implicitly part of the
bindable interface, and defaulted `param` is implicitly
bindable-with-default. Writing `pub param` or `pub(bind) param` is a
parse error.

```graphcal
pub param dry_mass: Mass = 1200.0 kg;   // parse error — drop the `pub`
param dry_mass: Mass = 1200.0 kg;       // OK
```

### Private by default

Declarations without an annotation are private:

```graphcal
param dry_mass: Mass = 1200.0 kg;       // visible/bindable because `param` is special
param internal: Mass = 500.0 kg;        // also bindable; use naming/module boundaries
                                        // to separate public inputs from internals
```

Importing a private non-`param` item produces error `V001`:

```graphcal
// ERROR: cannot import private item `internal_helper` from `lib`
import lib.{internal_helper};
```

### Required items must be `pub(bind)`

Required `index` / `type` / `dim` declarations (no body) form the
*bindable* interface of a library — importers must supply a binding.
They must therefore be declared `pub(bind)`. Writing bare `pub` on a
required item is error `V002`:

```graphcal
// ERROR: required index must be declared `pub(bind)`
pub index Phase;

// OK
pub(bind) index Phase;

// Required types and dims follow the same rule.
pub(bind) type Element;
pub(bind) dim Distance;
```

Required `param` is excluded from V002 (annotation-free; implicitly
bindable).

### Private-in-public (`V003`)

A visible declaration must not reference private type-system items
(`dim`, `type`, `index`, `base dim`) in its written signature. This
prevents leaking names that importers cannot see. Violating this rule
is error `V003`:

```graphcal
dim Velocity = Length / Time;
// ERROR: `pub node` `speed` references private dim `Velocity`
pub node speed: Velocity = 10.0 m/s;

// Fix: make the dim visible too.
pub dim Velocity = Length / Time;
pub node speed: Velocity = 10.0 m/s;
```

### `pub(bind)` indexes and variant literals (`V004`)

When an `index` is declared `pub(bind)`, its variant literals cannot
appear in the defining file's `node` / `const` bodies or in public
sinks (`plot` / `assert` / `figure` / `layer`). The reason: importers
may rebind the index to a different variant set, which would orphan
the literal. Either abstract over the index with `for p: I { ... }` or
move the variant-specific value into a `param`. Violating the rule is
error `V004`:

```graphcal
pub(bind) index Phase = { Design, Test };
// ERROR: variant literal `Phase.Design` of `pub(bind) index` cannot be
//        used in the defining file
param phase_cost: Dimensionless[Phase];
pub node cost: Dimensionless = @phase_cost[Phase.Design];
```

### Include overrides must reconcile (`V005`)

If an include overrides a bindable symbol `s` and some kept declaration
in the merged IR still mentions a name nominally tied to `s` (e.g.,
a variant literal of an overridden `index`, a field access of an
overridden `type`), the importer must *also* re-bind that dependent
declaration. Otherwise the orphan mention has no meaning in the merged
graph — error `V005`:

```graphcal
// lib.gcl
pub(bind) index Phase = { Design, Test };
param cost: Dimensionless[Phase] = { Phase.Design: 1.0, Phase.Test: 2.0 };

// main.gcl
pub(bind) index NewPhase = { Review, Ship };
// ERROR: include overrides index `Phase` but does not re-bind `cost`,
//        whose default mentions `Phase.Design`
include lib(Phase: NewPhase);

// Fix: re-bind `cost` as well.
include lib(
    Phase: NewPhase,
    cost: { NewPhase.Review: 1.0, NewPhase.Ship: 2.0 },
);
```

`dim` and `param` overrides never trigger V005: their substitution is
total (algebraic / by value) and leaves no orphan nominal mentions.

### Re-exports and generics leakage (`V006`)

A `pub include` / `pub import` re-exports the dependency's `pub` items
under the importer's namespace. If the include's bindings rename a
`pub(bind)` symbol to a name that is *private* at the importer, and
that private name appears in a re-exported signature, downstream
consumers would see a symbol they cannot name. That's error `V006`:

```graphcal
// container.gcl
pub(bind) type Element;
pub type Widget { Widget(item: Element) }

// main.gcl
type Inner { Inner }                  // private at the importer
// ERROR: re-exported type `Widget`'s signature references private type `Inner`
pub include container(Element: Inner) as c;

// Fix: make the substituted name visible too.
pub type Inner { Inner }
pub include container(Element: Inner) as c;
```

## Parameterized Includes

A bound `param` or `index` in an `include` instantiates the dependency
with a specific value. This is how reusable "library" DAGs are
specialized at the call site.

### Param bindings

```graphcal
include nasa.rocket.compute_thrust(dry_mass: 800.0 kg).{ thrust };
```

Multiple instantiations with different values produce independent
sub-graphs:

```graphcal
include nasa.rocket.compute_thrust(dry_mass: 800.0 kg, isp: 320.0 s) as stage_1;
include nasa.rocket.compute_thrust(dry_mass: 500.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1.delta_v + @stage_2.delta_v;
```

Binding expressions can reference `@` values from the surrounding scope:

```graphcal
param my_mass: Mass = 800.0 kg;
include nasa.rocket.compute_thrust(dry_mass: @my_mass).{ thrust };
```

### Required parameters

A `param` declared without a default value is **required** — the
importer must supply it via an `include` binding (or, for entry-point
files, via `--set` / `--input` on the command line):

```graphcal
// lib/rocket_engine.gcl
param dry_mass: Mass;                     // required — must be provided
param fuel_mass: Mass;                    // required — must be provided
param isp: Time = 320.0 s;                // optional — has default

pub const node g0: Acceleration = 9.80665 m/s^2;
pub node v_exhaust: Velocity = @isp * @g0;
pub node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
pub node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```graphcal
// main.gcl
include lib.rocket_engine(dry_mass: 800.0 kg) as engine;
node dv: Velocity = @engine.delta_v;
```

If a required param is not provided, the compiler emits error `O003`.

### Index bindings

Bind a [required index](indexes.md#required-indexes) by name; the
right-hand side must be the **name of a concrete index**, not an
expression:

```graphcal
// lib/budget.gcl
pub(bind) index Phase;

param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

```graphcal
// main.gcl
pub index MyPhase = { Design, Build, Test };

include lib.budget(
    Phase: MyPhase,
    cost: { MyPhase.Design: 10.0, MyPhase.Build: 20.0, MyPhase.Test: 5.0 },
).{ total };

node result: Dimensionless = @total;  // 35.0
```

#### Kind matching

Named indexes can only be bound to named indexes, and range indexes can
only be bound to range indexes. Binding a named index to a range or
vice versa is a compile error.

#### Dimension matching for range indexes

When binding a required range index, the concrete range index must have
the **same dimension**:

```graphcal
// lib.gcl
pub(bind) index Step: Time;   // requires dimension Time

// main.gcl
index MyStep = linspace(0.0 s, 10.0 s, step: 1.0 s);     // OK
include lib(Step: MyStep);

index DistStep = linspace(0.0 m, 100.0 m, step: 10.0 m); // dimension is Length
include lib(Step: DistStep);                              // ERROR: dimension mismatch
```

### Partial bindings

Bindings are optional for any param or index that has a default. Bind
only the ones you want to override; the rest keep their defaults.
Required indexes (those without a default) must always be bound.

```graphcal
// rocket.gcl has params: dry_mass (default), fuel_mass (default), isp (default)

// OK: only dry_mass is overridden; fuel_mass and isp keep their defaults
include lib.rocket(dry_mass: 800.0 kg) as r;

// OK: all params are explicitly bound
include lib.rocket(dry_mass: 800.0 kg, fuel_mass: 2800.0 kg, isp: 320.0 s) as r;
```

### Validation

- Binding names must be `param` or `index` declarations in the included
  module.
- Binding a `node`, `const node`, or unknown name is a compile error.
- All required params must be provided by bindings, `--set`, or
  `--input`.
- All required indexes must be provided by bindings.
- Index binding values must be the name of a concrete index in the
  importer's scope.
- Named indexes can only be bound to named indexes; range indexes can
  only be bound to range indexes. Range index dimensions must match.
- Dimension mismatches are caught by the dimension checker after
  merging.

## Circular Imports

Graphcal detects circular imports at compile time:

```graphcal
// a.gcl
import b.{x};

// b.gcl
import a.{y};
// ERROR: circular import detected
```

## Project Organization Patterns

### Constants / Parameters / Main

A common pattern separates concerns into separate files inside a single
package:

```text
project/
  graphcal.toml   -- [package] name = "project"
  src/
    project/
      constants.gcl   -- shared physical constants
      params.gcl      -- tunable input parameters
      main.gcl        -- computation graph, imports the others
```

### Library / Application

For reusable DAGs, group them into a real package and import by full
path:

```text
project/
  graphcal.toml   -- [package] name = "project"
  src/
    project/
      lib/
        orbital.gcl   -- reusable orbital mechanics DAGs
        thermal.gcl   -- thermal analysis DAGs
      main.gcl        -- application-specific graph
```

### Reusable Templates with Required Parameters

Use required params to create library files that must be instantiated
with specific values:

```graphcal
// src/project/lib/rocket.gcl
param dry_mass: Mass;          // required
param fuel_mass: Mass;         // required
param isp: Time = 320.0 s;     // optional default

pub const node g0: Acceleration = 9.80665 m/s^2;
pub node v_exhaust: Velocity = @isp * @g0;
pub node mass_ratio: Dimensionless = (@dry_mass + @fuel_mass) / @dry_mass;
pub node delta_v: Velocity = @v_exhaust * ln(@mass_ratio);
```

```graphcal
// src/project/main.gcl
include project.lib.rocket(dry_mass: 800.0 kg, fuel_mass: 2000.0 kg, isp: 320.0 s) as stage_1;
include project.lib.rocket(dry_mass: 500.0 kg, fuel_mass: 1200.0 kg, isp: 450.0 s) as stage_2;

node total_dv: Velocity = @stage_1.delta_v + @stage_2.delta_v;
```

### Reusable Templates with Required Indexes

```graphcal
// src/project/lib/budget.gcl
pub(bind) index Phase;

param cost: Dimensionless[Phase];
pub node total: Dimensionless = sum(for p: Phase { @cost[p] });
```

```graphcal
// src/project/main.gcl
pub index ProjectPhase = { Design, Build, Test };

include project.lib.budget(
    Phase: ProjectPhase,
    cost: { ProjectPhase.Design: 10.0, ProjectPhase.Build: 20.0, ProjectPhase.Test: 5.0 },
).{ total };

node project_cost: Dimensionless = @total;
```

## Assertions in Imported Files

When a file is imported (or its declarations included), **all of its
assertions are automatically evaluated and reported**, regardless of
whether they are explicitly listed. This ensures that safety checks in
library files are never silently skipped.

To use an imported assertion in `#[assumes(...)]`, you must import it
by name. See
[Assertions](assertions.md#assertions-in-multi-file-projects) for
details.

## Evaluation Entry Point

When running `graphcal eval`, the entry file is the one you pass on the
command line. All `import` and `include` dependencies are resolved
transitively from that file:

```bash
graphcal eval project/src/project/main.gcl
```

The entry file's package flavor is determined by where it lives. If a
`graphcal.toml` sits in an ancestor directory **and** the entry file is
inside that package's namespace
(`<source_dir>/<package_name>.gcl` or under
`<source_dir>/<package_name>/`), the manifest defines the package
layout and the file can use cross-file imports. Otherwise — no manifest
in any ancestor, or a manifest exists but the entry file lives outside
its namespace — the file is treated as a single-file virtual package
and may only self-reference.
