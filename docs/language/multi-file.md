---
icon: material/file-multiple
---

# Multi-File Projects

Graphcal organizes code into **packages**. Every `.gcl` file belongs to
exactly one package. An `import` or `include` identifies an outside module by
an absolute path that starts at a package root; after an import, expressions use
the local name or `as` alias that the declaration introduced. The absolute path
syntax is the same whether the package lives on disk under a manifest or whether
it is a single file you just created — the only thing that changes between those
cases is the externally-visible package name.

Two declarations bring outside material into a DAG:

- **`import`** brings *names* (compile-time references: `type`, `dim`,
  `unit`, `const node`, constructors, `dag`, and `index`) into the local
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
import nasa.rocket;                                      // bare: brings `rocket`
import nasa.rocket as nr;                                // alias: brings `nr`
import nasa.rocket.{type Orbit, dim Length, compute_thrust as ct};
// brace: brings `Orbit`, `Length`, and `ct` only
```

The forms differ in what enters scope:

| Form                                                   | Names added                                |
|--------------------------------------------------------|--------------------------------------------|
| `import nasa.rocket;`                                  | `rocket` (the module, by its leaf name)    |
| `import nasa.rocket as nr;`                            | `nr` (the module under alias)              |
| `import nasa.rocket.{type Orbit};`                     | `Orbit` only — **not** `rocket`            |
| `import nasa.rocket.{type Orbit, dim Length, compute_thrust as ct};` | `Orbit`, `Length`, and `ct` only |
| `import nasa.rocket as nr.{type Orbit};`               | parse error — alias and brace mutually exclusive |

A whole-module name denotes the exact imported DAG module, not merely a prefix
for its contents. Because file roots and inline DAGs are one abstraction,
`@rocket(args).out` or `@nr(args).out` invokes that target directly, while
`@rocket.child(args).out` descends to an exported child DAG. Imported module
aliases and local DAGs share this callable namespace; if both bind the same
spelling to different targets, a call is ambiguous and must be renamed.

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

### Selective imports declare their category

Every non-term namespace has an explicit marker. A bare item selects only the
term namespace (constants, DAGs, constructors, and runtime declarations that
are then rejected by the pure-import policy):

| Item form | Selects |
|-----------|---------|
| `name` | Term declaration or constructor |
| `type Name` | Struct or union type |
| `dim Name` | Dimension |
| `unit name` | Unit |
| `index Name` | Index |

```graphcal
import nasa.rocket.{
    type Orbit,
    dim Length,
    unit nautical_mile,
    MAX_THRUST,
    index Maneuver,
};
```

The marker makes an import a typed manifest. If the exporter changes
`nautical_mile` from a unit into another kind of entity, the import fails at
that line instead of silently binding the new category. The M022 diagnostic
lists every exported category with that spelling using copy-pasteable forms
such as `nautical_mile` or `unit nautical_mile`.

Type names and constructors intentionally occupy different namespaces. To
import both sides of a same-spelled type/constructor pair, write both items:

```graphcal
import school.records.{type Student, Student};
```

Units may likewise share a spelling with a term because units are used only in
unit positions. Import each side explicitly, for example
`import finance.{unit JPY, JPY};`. A value and a constructor may **not** share
a name because both are terms and would be indistinguishable in expressions.

Imported type-system declarations retain semantic dependencies from their
module of definition. For example, importing `dim WeightedRate` preserves its
resolved expansion through private or public sibling dimensions, importing
`unit point` preserves its canonical backing dimension and scale, and importing
`type Request` preserves nested record and index field types. This also applies
when a dimension or unit definition uses module-qualified dependencies. Those
dependencies do not introduce extra source names into the consumer: import a
sibling only when consumer source names it directly. Constructors remain a
separate term import when consumer source explicitly constructs a value.

### Aliasing items

Each item in a brace list may be aliased independently:

```graphcal
import nasa.rocket.{type Orbit as O, compute_thrust as ct};
```

Aliases obey the same namespace-specific reservation rules as direct
local declarations (N009):

- `type`, `dim`, and `index` aliases cannot reuse a prelude dimension or
  built-in type spelling.
- `unit` aliases cannot reuse a prelude unit spelling.
- aliases of graph values cannot reuse a built-in numeric constant spelling
  such as `E` or `PI`.

These are separate namespace policies, not one global reserved-word list.
Graph-value aliases may use time-scale and built-in-function spellings because
`@UTC`, `Datetime<UTC>`, and `sin(...)` are structurally disambiguated.
Selective `include` items and `pub` re-exports apply the same rules.

### What `import` may bring

Only compile-time names cross the `import` boundary:

| Declaration kind | Selective item | Reference after import |
|------------------|----------------|------------------------|
| `const node` | `name` | `@name` |
| `dim` | `dim DimName` | `DimName` |
| `unit` | `unit unit_name` | `unit_name` |
| `type` | `type TypeName` | `TypeName` |
| `index` | `index IndexName` | `IndexName` |
| constructor | `name` | Used in value expressions |
| `dag` | `name` | Used with `include` or `@name(args).out` |

Runtime values — non-`const` `node` and any `param` — are **not**
importable. To consume runtime values from another file, instantiate
the producing DAG via `include` (see [The `include` Form](#the-include-form)).
Assertions are instance outcomes and plots, figures, and layers are runtime
visualization requests, so none can cross a pure `import` boundary. Request
instance-scoped assertions and plots through an `include` brace list instead
(see [Cross-File Plots](plots.md#cross-file-plots)).

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

### Selective import re-exports

An import is always a private use-site. To re-export a binding, select it
explicitly and put `pub` on that item:

```graphcal
import nasa.rocket.{ pub type Orbit, compute_thrust };
//                   ^^^ only `Orbit` is re-exported
```

Bare and aliased imports remain private qualified bindings. This prevents a
new export in a dependency from silently widening an intermediate module's
public API:

```graphcal
import nasa.rocket;       // private binding named `rocket`
import nasa.rocket as r;  // private binding named `r`
```

All leading visibility forms are rejected. Put `pub` on each selected item
instead:

```graphcal
pub import nasa.rocket;                    // parse error
pub import nasa.rocket.{ type Orbit };     // parse error
pub(bind) import nasa.rocket;              // parse error
import nasa.rocket.{ pub type Orbit };     // explicit public binding
```

Imports are use-sites and are never bindable; `pub(bind)` belongs only on
bindable declarations.

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

// Category markers are import-only; include selectors are always node outputs.
// include nasa.rocket.compute_thrust(orbit: @o).{ dim thrust }; // parse error
```

| Form                                                            | Result                                                |
|-----------------------------------------------------------------|-------------------------------------------------------|
| `include path.dag(args);`                                       | Sugar for `... as dag` — leaf name is the alias       |
| `include path.dag(args) as a;`                                  | Outputs reached as `@a.<output>`                      |
| `include path.dag(args).{x};`                                   | `x` itself becomes a node in the current DAG          |
| `include path.dag(args).{x as y};`                              | Same, renamed                                         |
| `include path.dag(args) as a.{x};`                              | parse error — alias and brace mutually exclusive      |

Each output selected through a brace list or reached through an include alias
must be public, including for a same-file DAG. Across a module boundary, the
DAG named by `path.dag` must also be `pub`. Renaming an output does not change
its visibility. Private nodes inside the included DAG remain implementation
details.

`graphcal eval` therefore shows only the entry DAG and the include's selected
or projectable outputs by default. Use `--output-view all` when debugging to
also print each instance's bound params and private helper values. This option
changes presentation only; it does not make private names accessible to the
including DAG.

A brace-list item may also name a `pub plot` of the included file: that is
the consumer's request to display the library's chart of this instance. The
plot enters the root namespace under its local alias; `#[hidden]` on the
item includes it for figure/layer composition only. See
[Cross-File Plots](plots.md#cross-file-plots).

### Configured includes may nest

An included file or inline DAG may own configured includes of its own. Each
level is a distinct instance scope, and a binding is evaluated in its immediate
including scope:

```graphcal
// leaf.gcl
param x: Dimensionless;
pub node doubled: Dimensionless = @x * 2.0;

// middle.gcl
param x: Dimensionless;
include demo.leaf(x: @x) as leaf;
pub node out: Dimensionless = @leaf.doubled;

// main.gcl
include demo.middle(x: 3.0) as middle;
node result: Dimensionless = @middle.out; // 6.0
```

Nesting may be arbitrarily deep as long as the project dependency graph is
acyclic. Multiple outer instances recursively create isolated inner instances.
Private values stay private, selected/public outputs keep the outer instance
path, and assertion diagnostics use the complete path such as
`middle.leaf.positive`. An intermediate DAG that exposes a configurable input
must declare its own `param` and forward it explicitly, as above.

Imports used internally by an included file are isolated with that configured
instance. Two libraries may each import a local `C` (or use the same module
alias) from different modules, and each included body continues to read its own
canonical declaration regardless of include order or nesting. Include-binding
expressions belong to the importer, while declaration signatures and producer
bodies belong to the included file; diagnostics report the file that owns the
failing expression instead of applying one file's spans to another's text.
Dynamic units do not cross an include boundary (`M026`); keep their runtime
scales private inside an entry or explicitly called DAG.

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

### Selective include-output re-exports

An include is also a private use-site. Re-export only explicitly selected
outputs by marking each output `pub` inside the brace list:

```graphcal
include nasa.rocket.compute_thrust(orbit: @o).{ pub thrust, mass_flow };
//                                                      ^^^^^^^^^ private here
```

Bare and aliased includes create private configured instances. Leading
`pub include` and `pub(bind) include` forms are parse errors; Graphcal does
not expose a dependency-controlled output namespace wholesale.

### DAG call expression

Inside an expression, `@dag(args).out` is sugar for an anonymous runtime
`include ... as <synthetic>; @<synthetic>.out`. The projection must name an
externally projectable value in the DAG: either an explicitly exported node or
a param input port. Projecting a param reads its effective call binding or,
when omitted, its default.

```graphcal
dag mission {
    import nasa.rocket.{compute_thrust, type Orbit};
    param o: Orbit;
    node t: Force = @compute_thrust(orbit: @o, dry_mass: 800.0 kg).thrust;
    node effective_mass: Mass =
        @compute_thrust(orbit: @o, dry_mass: 800.0 kg).dry_mass;
}
```

Each call site is a fresh instantiation, and the DAG's `assert`
declarations are checked per instantiation just like the `include`
path. Calls are runtime graph instantiations regardless of whether the target
is a file root or a source-nested `dag`, so they are rejected in `const node`
bodies and domain bounds. Because an expression has no reporting surface, a failing (or
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

#### Imported module calls: `@module(args).out`

Every file root and every inline `dag` block is the same kind of DAG module. A
whole-module `import` binds its exact target as a reusable module alias, so the
alias itself is callable whether the target lives in a file or inside another
DAG:

```graphcal
// Invoke the imported file-root DAG itself.
import nasa.rocket as rocket;
node file_output: Force = @rocket(orbit: @o).thrust;

// Invoke an inline DAG imported by its full module path.
import nasa.rocket.compute_thrust as thrust;
node inline_output: Force = @thrust(orbit: @o, dry_mass: 800.0 kg).thrust;
```

An unaliased import binds the path's leaf (`import nasa.rocket;` binds
`rocket`). Selective DAG imports are callable by their selected local name in
the same way:

```graphcal
import nasa.rocket.{compute_thrust as thrust};
node t: Force = @thrust(orbit: @o, dry_mass: 800.0 kg).thrust;
```

The alias can also qualify a child DAG. After importing the file module, the
same `compute_thrust` DAG can be reached as:

```graphcal
import nasa.rocket as rocket;
node t: Force = @rocket.compute_thrust(orbit: @o, dry_mass: 800.0 kg).thrust;
```

The projected member must be an explicitly exported node or a param input port.
Across a module boundary, every inline DAG traversed by the call path must also
be explicitly exported; a file root is its package-addressable module.

What *is* still rejected is dropping the projection: `@dag(args)`,
`@module(args)`, and `@module.dag(args)` without the trailing `.<out>` are parse
errors. A DAG instance with no projection is not a graph value, which is what
`@` requires.

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
addressing rule as cross-package `nasa.rocket.compute_thrust`. Resolution
chooses the longest prefix that names a physical `.gcl` file, then follows every
remaining inline-DAG segment exactly; same-named descendants under different
parents never alias each other.

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

An inline DAG may spell an `include` of its enclosing DAG by full path, but
that source forms an unbounded concrete-instance graph. Graphcal rejects direct
and mutual recursive includes before constructing module scopes; recursive
instantiation is not a runtime control-flow mechanism.

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

Self-imports use exactly the same pure-import policy as cross-file imports:
constants, constructors, DAGs, and marked type-system names are compile-time
items; params and nodes require an instance; assertions and visualization
requests are rejected because there is no hidden enclosing-file instance.

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

`source_dir` is a portable relative directory, not a host-native path. It must
contain one or more normal `/`-separated components. Empty spellings,
`.`/`..` components, absolute paths, repeated or trailing separators,
backslashes, control characters, and platform path prefixes are rejected. Use
a named directory such as `src` or `lib`; `source_dir = "."` is not supported.

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
- `git` accepts only parsed remote HTTPS URLs, `ssh://` URLs, and SSH scp-like
  `user@host:path` URLs. Every source must include a host and non-empty repository
  path. Local paths, `file://`, plain HTTP, other URL schemes, embedded
  credentials, query strings, fragments, dotted/ambiguous paths, and
  option-shaped authority or path components are rejected before fetching.
- Private repositories require whatever credentials the underlying Git fetch
  implementation can obtain in the current environment, so support is
  environment-dependent. For example, SSH may work when an SSH key/agent is
  available, while HTTPS may fail unless a compatible credential helper or
  non-interactive credential provider is configured. The SSH username in an SSH
  URL is allowed; passwords, tokens, and other secrets must not be embedded in
  the URL.
- The dependency table key is the source-visible dependency name used in
  `import` and `include` paths.
- If `package` is omitted, the fetched package's `[package].name` must match
  the dependency key.
- If `package` is present, the dependency key is a local alias and `package`
  names the fetched package's real package name.

!!! warning "Migration from local or unsupported Git sources"
    Manifests and lockfiles that used a filesystem path, `file://`, `http://`,
    or another previously accepted spelling must switch to a reachable HTTPS or
    SSH remote and then regenerate `graphcal.lock` with `graphcal deps lock`.
    Graphcal currently has no local-path dependency form; local development
    repositories must be served through a supported remote transport.

```toml
[dependencies]
units_v1 = { package = "units", git = "https://github.com/acme/units.git", rev = "1111111111111111111111111111111111111111" }
units_v2 = { package = "units", git = "https://github.com/acme/units.git", rev = "2222222222222222222222222222222222222222" }
```

Source resolves those aliases explicitly:

```graphcal
import units_v1.si.{unit m as m_v1};
import units_v2.si.{unit m as m_v2};
```

Run `graphcal deps lock` before checking or evaluating a package with
dependencies. The command writes `graphcal.lock`, a deterministic,
tool-maintained lockfile that records package instances, exact Git commits,
source tree hashes, direct dependency edges, and the Graphcal/standard-library
versions used for resolution. The lockfile records dependency graph edges
between package instances; it is not a flat "one version per package name" map,
so multiple revisions of the same package can coexist. For the current lock
version, every fixed table is a closed schema: unknown root, package, source,
tree-hash, and plugin fields are rejected rather than ignored and then dropped
by reserialization. Source-tree and plugin SHA-256 values must be exactly 64
lowercase hexadecimal digits. Entries for the same canonical Git URL and commit
must agree on the source-tree digest. Cycle validation uses an explicit typed
work stack, so a deeply nested acyclic dependency graph cannot exhaust the Rust
or WebAssembly call stack; cycle diagnostics retain the package-instance path
that closes the cycle. The only path source is the root package itself, fixed
to `.`; every dependency entry must be Git-backed, and every package entry must
be reachable from the declared root. Arbitrary absolute/parent paths and orphan
entries are rejected before the loader derives or canonicalizes source roots.

`graphcal check`, `graphcal eval`, `graphcal graph`, and the LSP read only the
lockfile and locally materialized cache entries. They do not fetch dependencies
or update `graphcal.lock`; if the lockfile is missing, stale, version-mismatched,
or points at a missing or hash-mismatched cached source, they fail and ask you
to run `graphcal deps lock`.

`graphcal deps lock` reuses a cached checkout without fetching only after its
source-tree digest matches the prior validated lock. Cache identity is typed:
the canonical remote URL and immutable commit select a source directory, and
the verified tree digest selects an immutable generation. Same-source writers
serialize with an advisory lock, stage fetches inside that cache directory, and
publish generations by rename. Valid older generations are not replaced, so a
concurrent evaluator holding a rooted filesystem capability remains usable.
`GRAPHCAL_CACHE_DIR` overrides the cache root; relative values are resolved to
one absolute identity before producer and loader paths are derived.

!!! note "Cache layout migration"
    Existing lockfiles remain valid, but the content-addressed layout does not
    reuse checkout directories written by older Graphcal versions. Run
    `graphcal deps lock` once with network access to materialize the new cache
    layout.

Loading also binds every lock entry to exactly one materialized manifest before
resolving modules. Package names, source directories, and dependency alias sets
must match in both directions. This ensures the source directory used for
module resolution is exactly the directory covered by integrity verification;
a stale or hand-edited lockfile cannot redirect imports to an unhashed tree.

Existing hand-edited lockfiles with custom fields, uppercase hashes, malformed
hashes, non-root path sources, or unreachable package entries must remove those
fields/entries and run `graphcal deps lock` to regenerate canonical pins. Do not
preserve private metadata inside `graphcal.lock`; keep it in a separate file.
Plugin paths containing absolute, `.`/`..`, backslash, drive-prefix-like colon,
or control-character components must move the artifact to a portable relative
path and regenerate the lock.

Lock creation and loading use the same deterministic source-tree algorithm.
Package trees may contain only ordinary directories and regular files: symbolic
links and special files are rejected rather than followed, every canonical path
must remain under its locked package root, and relative path names must be UTF-8.
Rooted disk access is handle-relative to an open directory capability, so a
concurrent file, parent-directory, or directory-listing symlink swap cannot
escape after validation.
The root package continues to use editor overlays, while each cached dependency
is read through its own immutable rooted filesystem capability. Overlay
construction is itself capability-preserving: existing buffers use a canonical
base identity, and unsaved buffers are accepted only after their nearest
existing parent is canonicalized through the rooted base. Relative paths,
paths outside that root, and file/directory collisions are rejected. Open
buffers from unrelated workspace roots are not added to the active project's
snapshot.

Project ingestion is bounded before allocation. CLI and LSP loads, `graphcal
deps lock`, and plugin inspection share limits of 16 MiB per `.gcl` source,
source-tree file, or WASM plugin, 1 MiB per manifest, and 4 MiB per lockfile,
with a shared ceiling of 10,000 loaded artifacts/source-tree entries and 256
MiB. Plugin pinning validates a portable root-relative artifact path before any
filesystem lookup and hashes regular files incrementally instead of allocating
the complete module. Lock parsing additionally limits the
number of package entries to the configured file-count ceiling before graph
validation. Exceeding a limit is a loader error; evaluator work budgets do not
replace these earlier I/O bounds.

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

## Visibility, Bindability, and Input Ports

Graphcal keeps two external boundary roles distinct:

- An **export** is an ordinary declaration made visible across an
  include/import boundary with `pub` or `pub(bind)`.
- An **input port** is declared with `param`. It is not an ordinary declaration
  carrying an implicit `pub` marker: its input-port role is tracked separately.
  The port is both bindable and externally readable/projectable, so callers can
  observe its effective bound/default value.

For ordinary declarations, visibility and bindability form a **two-axis split**:

- **Visibility** (`pub`): whether the declaration is exported across the
  include/import boundary.
- **Bindability** (`pub(bind)`): whether an importer may *override* it with an
  include binding. Bindability implies export visibility.

| Annotation   | Exported? | Bindable? | Use for                                                                 |
|--------------|:---------:|:---------:|-------------------------------------------------------------------------|
| (none)       | no        | no        | internal helpers, private values                                        |
| `pub`        | yes       | no        | constants, derived dims / units / types consumers read but don't rewire |
| `pub(bind)`  | yes       | yes       | required indexes / types / dims                                         |

`param` is outside that annotation matrix. The declaration kind directly
creates a named DAG input port:

- `param x: T;` is a required input port;
- `param x: T = expr;` is an input port with a default;
- every param in a callable DAG belongs to its named include/call signature;
- the effective value of a callable-DAG param may also be selected or projected;
- params in the entry DAG belong to the `--param` / `--params-json` /
  `--params-json-file` surface.

Because `pub` would add no state to an input port, both annotated spellings are
parse errors:

```graphcal
pub param dry_mass: Mass = 1200.0 kg;   // parse error
pub(bind) param dry_mass: Mass;         // parse error
param dry_mass: Mass = 1200.0 kg;       // OK: defaulted input port
```

A param remains readable as an output. Selection returns its supplied value, or
its default when the caller omits the binding:

```graphcal
include external_dag().{ x as default_x };
include external_dag(x: 1.0).{ x as bound_x };

node projected: Dimensionless = @external_dag().x;
```

Selecting or re-exporting that effective value does not preserve the input-port
role transitively: an intermediate DAG that wants its own callers to bind `x`
must declare its own `param x` and forward it in the nested binding.

### Internal values are private by default

Use `node` for an internal computed value and `const node` for an internal fixed
value. Do not use a specially named `param`: every param is part of the DAG's
input signature. Internal parameterization belongs behind a private DAG or
module boundary.

```graphcal
param dry_mass: Mass = 1200.0 kg;             // named input port
const node structure_mass: Mass = 500.0 kg;   // private fixed value
node wet_mass: Mass = @dry_mass + @structure_mass; // private computed value
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

Required `param` is excluded from V002 because `param` directly declares an
annotation-free input port; requiredness is represented by the missing default.

### Private-in-public (`V003`)

A visible declaration must not reference private type-system items
(`dim`, `type`, `index`, `base dim`) in its effective signature. This includes
nested generic arguments and every generic parameter default: omitting a
public type's argument must not silently select a private type, dimension, or
index. The compiler checks canonical resolved dependencies, including through
aliases. This prevents leaking names that importers cannot see. Violating this
rule is error `V003`:

```graphcal
dim TransferSpeed = Length / Time;
// ERROR: `pub node` `speed` references private dim `TransferSpeed`
pub node speed: TransferSpeed = 10.0 m/s;

// Fix: make the dim visible too.
pub dim TransferSpeed = Length / Time;
pub node speed: TransferSpeed = 10.0 m/s;

// Defaults are part of the public signature too.
type Secret { Secret }
// ERROR: omitting T would expose private type `Secret`.
pub type Wrapper<T: Type = Secret> { Wrapper(value: T) }
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

If an include overrides a bindable symbol `s` and some kept parameter
default still performs an operation nominally tied to `s`, the importer must
*also* re-bind that dependent parameter. These dependencies include index
labels, field selection, constructor calls and match patterns, and nominal
index/type generic arguments. They are compared by canonical semantic owner,
not by field or constructor spelling: replacing a type with another type that
happens to have the same `x` field or `Left` constructor still requires
reconciliation. Otherwise the default would be silently reinterpreted under a
different nominal contract — error `V005`:

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
total (algebraic / by value) and leaves no orphan nominal mentions. The
diagnostic points at the include that introduced the override; explicitly
binding each reported dependent parameter reconciles it.

### Re-exports and generics leakage (`V006`)

A selectively exported include output can mention a bound type, dimension, or
index in its effective signature. If the include substitutes a `pub(bind)`
symbol with a name that is *private* at the importer, downstream consumers
would see a symbol they cannot name. That's error `V006`:

```graphcal
// container.gcl
pub(bind) type Element { Element }
pub const node origin: Element = Element;

// main.gcl
type Inner { Inner } // private at the importer
// ERROR: re-exported const node `origin` references private type `Inner`
include container(Element: Inner).{ pub origin };

// Fix: make the substituted name visible too.
pub type Inner { Inner }
include container(Element: Inner).{ pub origin };
```

V006 checks only outputs/items explicitly marked `pub`; adding another public
output to the dependency does not alter the importer's facade. The check follows
only typed include substitutions into the importer. Unsubstituted builtin,
dependency-local, and qualified names retain their original identity rather
than being reinterpreted as a same-spelled importer declaration.

## Parameterized Includes

A bound `param` or `index` in an `include` instantiates the dependency
with a specific value or type-level argument. This is how reusable "library"
DAGs are specialized at the call site.

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
importer must supply it via an `include` binding (or, for entry-point files,
via `--param`, `--params-json`, or `--params-json-file` on the command line):

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

If an `include` or inline DAG call omits a required param, the compiler emits
`G004` at that instantiation site. If an entry-DAG param remains unsatisfied
when preparing evaluation, the compiler emits `O003` at its declaration.

### Index bindings

Bind a [required index](indexes.md#required-indexes) with a compatible Index
argument. The right-hand side can name an index in the including DAG or use a
structural `Fin(N)` axis directly:

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

A positional-axis DAG needs no named alias for its cardinality:

```graphcal
dag scale {
    pub(bind) index Axis;
    param xs: Dimensionless[Axis];
    pub node ys: Dimensionless[Axis] = for i: Axis { @xs[i] * 2.0 };
}

include scale(
    Axis: Fin(3),
    xs: table[Fin(3)] { 1.0; 2.0; 3.0; },
) as scaled;
```

The `Fin` cardinality uses normal type-level Nat addition and multiplication,
must normalize to a concrete positive value at this call site, and remains
subject to the practical index-cardinality limit.

#### Kind matching

An unconstrained required index (`pub(bind) index Axis;`) is a discrete port
and accepts either a named index or `Fin(N)`. A concrete bindable named index
remains nominal and can only be rebound to another named index. Coordinate
indexes can only be bound to coordinate indexes; binding a discrete axis to a
coordinate port (or vice versa) is a compile error.

#### Dimension matching for coordinate indexes

When binding a required coordinate index, the supplied coordinate index must have
the **same dimension**:

```graphcal
// lib.gcl
pub(bind) index Step: Time;   // requires dimension Time

// main.gcl
index MyStep = range(0.0 s, 10.0 s, step: 1.0 s);     // OK
include lib(Step: MyStep);

index DistStep = range(0.0 m, 100.0 m, step: 10.0 m); // dimension is Length
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

- A binding list is a unique name-to-argument mapping. Binding order has no
  effect, and repeating a value, index, type, or dimension target is a compile
  error rather than a last-binding-wins override.
- A value binding name must identify a `param` input port in the included
  module; index/type/dim bindings target declarations explicitly marked
  `pub(bind)`.
- Binding a `node`, `const node`, or unknown name is a compile error.
- Required input ports of an included/called DAG must be provided by bindings;
  required input ports of the entry DAG must be provided by `--param`,
  `--params-json`, or `--params-json-file`.
- All required indexes must be provided by bindings.
- Index binding values must be a compatible index name in the importer's scope
  or a concrete structural `Fin(N)` argument. An outer generic DAG may forward
  a required index; the include chain must eventually supply a concrete index.
- Unconstrained required indexes accept named and structural finite axes.
  Concrete named indexes remain named-only, while coordinate indexes remain
  coordinate-only. Coordinate-index dimensions must match after applying
  simultaneous dimension bindings.
- Coordinate-index binding dimensions are checked after simultaneous
  type-level substitutions have been applied.

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

## Assertions and Module Boundaries

`import` is compile-time only and never creates a hidden default instance, so
it does not evaluate or propagate assertions. Attempting to import an assertion
is rejected (`M024`).

Every explicit `include` creates a concrete runtime instance and evaluates the
assertions in that instance, including nested included assertions. Nested
instances preserve their full concrete owner path when rebased into the
importer; inconsistent internal ownership is an error rather than a stale
reference to the template. Select an assertion in the include braces when an
importer declaration needs to name that instance-local outcome in
`#[assumes(...)]`:

```graphcal
include project.checks(limit: 50.0).{ limit, limit_positive };

#[assumes(limit_positive)]
node ratio: Dimensionless = @limit / 2.0;
```

See [Assertions](assertions.md#assertions-in-multi-file-projects) for details.

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
