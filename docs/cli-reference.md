---
icon: material/console
---

# CLI Reference

The `graphcal` command-line tool provides subcommands for evaluating, serving, formatting, and checking `.gcl` files, as well as starting the LSP server.

## Global Options

```bash
graphcal [OPTIONS] <COMMAND>
```

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print version and build commit SHA when available |

## Commands Overview

| Command | Description |
|---------|-------------|
| [`eval`](#graphcal-eval) | Evaluate a `.gcl` file |
| [`format`](#graphcal-format) | Format `.gcl` files |
| [`check`](#graphcal-check) | Check `.gcl` files for errors without evaluation |
| [`dump`](#graphcal-dump) | Debug-print compiler/evaluator pipeline artifacts (experimental) |
| [`graph`](#graphcal-graph) | Export the dependency graph of a `.gcl` file (experimental) |
| [`model serve`](#graphcal-model-serve) | Serve a prepared model over Tenax stdio Arrow IPC |
| [`report build`](#graphcal-report-build) | Build a self-contained HTML report from a model (experimental) |
| [`deps lock`](#graphcal-deps-lock) | Resolve exact-rev Git dependencies and write `graphcal.lock` |
| [`plugin new`](#graphcal-plugin-new) | Scaffold a WASM plugin crate using the Rust SDK (experimental) |
| [`plugin test`](#graphcal-plugin-test) | Validate a built `.wasm` plugin module and call its functions (experimental) |
| [`lsp`](#graphcal-lsp) | Start the LSP server |

Commands that load Graphcal source sandbox filesystem access to the explicit
`--root`, the nearest discovered `graphcal.toml` directory, or (for a loose
file) the file's enclosing directory. An explicit root that does not exist or
cannot be canonicalized is a hard error; Graphcal never drops to unrestricted
filesystem access when sandbox setup fails. A rooted reader holds an open root
directory capability and performs opens, metadata queries, canonicalization,
and directory listing relative to that handle. Concurrent file or parent
symlink swaps therefore cannot redirect a checked operation outside the root.

---

## `graphcal deps lock`

Resolve a package's exact-rev Git dependencies, materialize them in the local
Graphcal cache, verify their source hashes, and write `graphcal.lock`.

```bash
graphcal deps lock [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--root <ROOT>` | Project root directory (overrides automatic `graphcal.toml` detection) |

`graphcal deps lock` is the only public package-management command in the MVP.
It reads `[dependencies]` from `graphcal.toml`, accepts only supported remote
HTTPS or SSH Git URLs with a full commit-hash `rev`, fetches any missing
sources, records a graph-shaped package-instance lockfile, and writes nothing
when the deterministic lockfile contents are already up to date. A checkout
whose source-tree digest still matches the prior validated lock is reused
without network access. Manifest,
source, source-tree, plugin, and existing-lock reads use the same per-artifact
and aggregate limits as ordinary project loading; lock generation therefore
cannot produce a lock that the default loader rejects only for artifact size.
Local paths,
`file://`, plain HTTP, unsupported schemes, embedded credentials, and malformed
or option-shaped remote components fail validation before any fetch starts.
In the generated lock graph, the unique root uses the fixed path source `.`,
every dependency is Git-backed, and every package entry is reachable from that
root. Loading rejects arbitrary path sources and orphan entries before deriving
source-root filesystem capabilities.

The command also scans the package's `.gcl` sources for
[WASM plugin imports](language/extern-functions.md#trust-lockfile-pins) and
pins each referenced `.wasm` file's SHA-256 as a `[[plugin]]` entry. Loading
enforces the pins: an unpinned or hash-mismatched plugin is a hard error, so
plugin binaries can only change together with a reviewable lockfile diff.

Dependency-consuming commands (`check`, `eval`, `dump`, `graph`, `model serve`, and the LSP) are
read-only with respect to packages: they read `graphcal.lock` and cached
sources, but they do not fetch, create, or update lockfile entries. If the
lockfile is missing, stale, uses a different Graphcal or standard-library
version, or references a missing or hash-mismatched cache entry, they fail and
ask you to run `graphcal deps lock`. Cache entries are content-addressed by a
typed canonical-URL/commit identity and the verified tree digest. Writers use a
per-source advisory lock, fetch into a same-cache staging directory, and publish
an immutable generation by rename; another process evaluating an older valid
generation continues to use it. Lock creation and verification share one
iterative hashing implementation; it rejects symlinks, special files,
non-UTF-8 relative names, and canonical paths outside the package root instead
of traversing them.

Private Git repositories are supported only when the underlying Git fetch can
obtain credentials from the current environment. This is intentionally not a
portable guarantee: SSH may work with a configured key/agent, while HTTPS may
fail unless a compatible credential helper or non-interactive credential
provider is available. Do not place credentials directly in `git` URLs in
`graphcal.toml`. `GRAPHCAL_CACHE_DIR` overrides the cache root; a relative
value is resolved to an absolute path once before producers and consumers
derive checkout paths.

!!! note "Cache layout migration"
    The content-addressed checkout layout replaces the earlier single-directory
    Git cache layout. Existing `graphcal.lock` files remain valid, but run
    `graphcal deps lock` once with network access to populate the new layout.

**Examples:**

```bash
# Resolve dependencies from the discovered project root
$ graphcal deps lock
wrote /path/to/project/graphcal.lock
```

```bash
# Resolve dependencies for an explicit project root
$ graphcal deps lock --root mission
up to date: /path/to/mission/graphcal.lock
```

See [Package Dependencies](language/multi-file.md#package-dependencies) for
manifest syntax and source-resolution rules.

---

## `graphcal plugin new`

!!! warning "Experimental"
    The plugin commands are experimental, like the plugin system itself;
    their surface may change in any release.

Scaffold a new WASM plugin crate that uses the
[`graphcal-plugin` Rust SDK](authoring-plugins.md): a `Cargo.toml`
(cdylib + rlib), a `rust-toolchain.toml` with the
`wasm32-unknown-unknown` target, a `src/lib.rs` with a sample `plugin!`
block, native unit tests, a `justfile`, and a README describing the
build-vendor-pin workflow.

```bash
graphcal plugin new <NAME> [OPTIONS]
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<NAME>` | Crate name: a lowercase letter followed by lowercase letters, digits, `-`, or `_` |

**Options:**

| Option | Description |
|--------|-------------|
| `--dir <DIR>` | Target directory (default: `./<NAME>`); must not exist yet |

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Scaffold created |
| `2` | Invalid name, existing target directory, or I/O error |

```bash
$ graphcal plugin new fluid-props
created plugin crate `fluid-props` at fluid-props
```

---

## `graphcal plugin test`

Validate a built `.wasm` module against the plugin ABI and print its
identity and signatures. Every load-time check runs before any plugin
code: the embedded manifest must decode, the module may import nothing
beyond `graphcal::fail`, and each manifest function's wasm parameter/result types must match its
declared signature. On success the output includes the SHA-256 (the
value `graphcal deps lock` pins) and a paste-ready `import plugin` block
in `.gcl` syntax. Module inspection accepts only a regular file, rejects
symlinks and special files before opening them, and reads at most 16 MiB before
WASM validation.

```bash
graphcal plugin test <MODULE> [--call <FUNCTION> [ARGS]...]
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<MODULE>` | Path to the `.wasm` plugin module |
| `[ARGS]...` | Arguments for `--call`: finite numbers in SI base units for quantities, `true`/`false` for `Bool`, exactly representable integers for `Int`, and rectangular JSON arrays whose leaves use the declared element kind |

**Options:**

| Option | Description |
|--------|-------------|
| `--call <FUNCTION>` | Call one provided function after validation, under the same fuel and memory limits evaluation uses |

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Module is a valid plugin (and the call, if any, succeeded) |
| `1` | ABI validation failed, the manifest cannot be rendered as Graphcal source, or the called function failed or returned a value violating its declared kind |
| `2` | Unreadable file, unknown function, or unusable call arguments |

Calls use the same ABI value policy as normal Graphcal evaluation. Quantity
arguments and results (including array elements and record fields) must be
finite; `Int` slots must convert exactly; and `Bool` slots must be numeric zero
or one. Numeric `-0.0` returned by a plugin is accepted as `false` or integer
zero rather than treated as a distinct encoding.

```bash
$ graphcal plugin test plugins/fluid_props.wasm --call lerp 1.0 3.0 0.5
plugin: plugins/fluid_props.wasm
sha256: 3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855c
abi: version 5, 2 function(s)

import plugin "plugins/fluid_props.wasm" as fluid_props {
    fn air_density(p: Length^-1 * Mass * Time^-2, t: Temperature) -> Length^-3 * Mass;
    fn lerp<D: Dim>(a: D, b: D, t: Dimensionless) -> D;
}

lerp(1.0, 3.0, 0.5) = 2
```

The rendered fixed dimensions are spelled structurally over the prelude
base dimensions (the manifest's alphabet); in your `.gcl` declaration you
may equivalently write derived names such as `Pressure` — verification is
structural. Array arguments to `--call` are bracketed literals
(`--call share [1.0,3.0]`) and array results render the same way; for
struct-returning functions the import block is preceded by a suggested
record declaration to paste alongside it. Because the block is source code,
Bool arrays use JSON booleans (`[true,false]`), Int arrays use JSON integers
(`[1,-2,3]`), and quantity arrays use JSON numbers in SI base units. Numeric
`0`/`1` are not accepted for Bool leaves; fractional or non-lossless numbers
are not accepted for Int leaves. Results render with the same semantic JSON
leaf kinds.

Because the block is source code, manifest function, parameter, generic, and result-field names must be valid
Graphcal identifiers rather than reserved keywords. A module whose wire names
cannot be represented safely is rejected instead of interpolating those names
into the block. Graphcal string literals do not currently have an escape syntax,
so a module path containing a quote or line break is likewise rejected.

---

## `graphcal eval`

Evaluate a `.gcl` file and print results.

```bash
graphcal eval [OPTIONS] <FILE>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<FILE>` | Path to the `.gcl` file (required) |

**Options:**

| Option | Description |
|--------|-------------|
| `--format <FORMAT>` | Output format: `text` (default) or `json` |
| `--output-view <VIEW>` | Values to display: `surface` (default) or `all` |
| `--param <NAME=VALUE>` | Bind one param to a closed Graphcal value (repeatable) |
| `--params-json <JSON>` | Bind params from an inline JSON object |
| `--params-json-file <FILE>` | Bind params from a JSON file; `-` reads stdin |
| `--params-json-max-bytes <BYTES>` | Maximum size of the JSON parameter document (default: 1 MiB) |
| `--plot <MODE>` | Plot output mode: `browser` (open in browser), `json` (print only plot JSON to stdout), or a path ending in `.html` (write a self-contained HTML page) |
| `--root <ROOT>` | Project root directory (overrides automatic `graphcal.toml` detection) |

`--params-json` and `--params-json-file` are mutually exclusive. Repeatable
`--param` bindings may supplement the selected JSON document, but every
parameter name must occur in exactly one CLI source. Repeating a `--param`, or
binding the same name directly and in JSON, is an error rather than a
last-write-wins override. `--params-json-max-bytes` is valid only when one of
the JSON sources is selected.

Binding names are unqualified parameter names declared directly in the entry
file. Imported and included implementation parameters are not independently
addressable; expose an entry parameter and pass it into the dependency instead.
Qualified strings such as `module::x=...` are rejected at the CLI boundary
instead of being interpreted as leaf names.

Entry-file params are the entry DAG's named input ports. Ports not supplied by
any parameter option keep their declared defaults; ports without a default are
required and must be supplied.

Values are recursively **closed**. Accepted forms include finite quantities,
`Int`, `Bool`, `datetime(...)`, `epoch<S>(...)`, Cartesian `complex(...)`,
named/key literals, complete algebraic constructors, and complete fixed-axis
map literals, with arbitrary nesting. A map entry cannot contain more key
levels than its declared indexed value permits; an over-nested entry is a shape
error and is never lowered without its schema. Graph references,
arithmetic such as `2 * PI`, constants, ordinary function/plugin calls, dynamic
units, conditionals, comprehensions, matches, scans, and unfolds are rejected.
This keeps CLI input as typed data rather than injecting a second computation
into the prepared DAG.

The same bounded JSON parser handles inline strings, files, and stdin. Semantic
errors point to the parameter key in the source, labelled with the file path,
`<--params-json>`, or `<stdin>`, and include the affected entry parameter name.
Inline JSON is convenient for small generated invocations, but it may be
recorded in shell history, process listings, or CI logs; use a file or stdin for
sensitive or large parameter documents.

The default `--output-view surface` prints every const, param, and node declared
by the entry DAG plus the outputs intentionally exposed by each include. A
brace include contributes only its selected local aliases. A whole-instance
include (`include ... as alias`) contributes its param ports and public
consts/nodes under that alias. Successful private implementation values and
producer-side duplicates are omitted.

Use `--output-view all` to debug an included DAG's complete instantiated state,
including bound params, private helper nodes, and producer-side values:

```bash
graphcal eval mission.gcl --output-view all
```

The view changes presentation only; it never makes a private declaration
referenceable from Graphcal source. Assertions are always reported, and a
failed internal value is shown even in `surface` view so a non-zero exit is
never unexplained.

Output entries (text and JSON) are keyed by the full alias-qualified path for
whole-instance includes (e.g. `good.out`, `good.v_positive`), so multiple
instantiations of the same DAG never collide and JSON output never silently
drops an instance. This qualification applies only to output names — the CLI parameter-binding
surface is unaffected.

Text output rounds finite quantities to seven significant decimal digits. It
uses positional notation when the rounded decimal exponent is from -4 through
14 and scientific notation outside that window. JSON output preserves the
underlying binary64 value and does not apply this presentation rounding.

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Success, all assertions pass |
| `1` | Assertion failure or evaluation error |
| `2` | Compile error (parse or type check), invalid CLI parameter argument, or internal I/O error |

**Examples:**

```bash
# Basic evaluation
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
isp        = 320 s
g0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.333333
delta_v    = 3778.221 m/s
```

```bash
# Bind all parameters directly
$ graphcal eval rocket.gcl --param 'dry_mass=1200.0 kg' --param 'fuel_mass=3500.0 kg' --param 'isp=320.0 s'
dry_mass   = 1200 kg
fuel_mass  = 3500 kg
isp        = 320 s
g0         = 9.80665 m/s^2
v_exhaust  = 3138.128 m/s
mass_ratio = 3.916667
delta_v    = 4284.301 m/s
```

```bash
# Provide a required param (param declared without a default value)
graphcal eval engine.gcl --param 'dry_mass=800.0 kg'

# Bind several params from inline JSON
graphcal eval engine.gcl --params-json '{"dry_mass":"800.0 kg","mode":"Mode#Nominal"}'

# Read the same JSON schema from a file or stdin
graphcal eval engine.gcl --params-json-file scenario.json
generate-scenario | graphcal eval engine.gcl --params-json-file -
```

The JSON parser preserves module-qualified constructor and index paths inside
structured value payloads. An unqualified constructor in a structured value is
resolved from the parameter's canonical expected type, recursively for nested
records. Its field values use the type's defining-module unit and index scope,
so an entry that imports `type Request` does not also need to import `Request`
or the dimensions, units, indexes, and nested constructors used by Request's
schema. These definition-site rules apply only to prepared boundary decoding;
constructors written in Graphcal source remain term imports.

The top-level JSON keys are still entry parameter names, and the same
closed-value compiler recursively checks every field and indexed entry against
that parameter's concrete type. Object keys must be unique at every nesting
level; escaped spellings that decode to the same key are duplicates too.
Integer-syntax tokens must fit exactly in Graphcal's signed 64-bit `Int` domain
and are never reinterpreted as floating point. Tokens containing a decimal point
or exponent are parsed as finite binary64 values; overflow and nonzero underflow
are rejected, while ordinary binary64 rounding follows the JSON numeric policy.

```json
{
  "phase": { "index": "mission::Phase", "entries": { "Burn": 1, "Coast": 0 } },
  "choice": { "variant": "mission::Pick", "fields": { "distance": "2.0 m" } }
}
```

Complex parameters use the same expression-string leaf form as other
quantities, keeping dimensions and units explicit:

```json
{
  "displacement": "complex(3.0 m, 4.0 m)"
}
```

```bash
# JSON output
$ graphcal eval rocket.gcl --format json
{
  "const": {
    "g0": {
      "display_value": 9.80665,
      "si_value": 9.80665,
      "unit": "m/s^2"
    }
  },
  "node": {
    "delta_v": {
      "si_value": 3778.2207684937407,
      "unit": "m/s"
    },
    ...
  },
  "param": {
    "dry_mass": {
      "display_value": 1200.0,
      "si_value": 1200.0,
      "unit": "kg"
    },
    ...
  }
}
```

A complex output represents each component explicitly. `si_value` always uses
SI base units; `display_value` is included when `->` selected a display unit:

```json
{
  "si_value": { "re": 3.0, "im": 4.0 },
  "display_value": { "re": 300.0, "im": 400.0 },
  "unit": "cm"
}
```

Multi-indexed values (2D and higher) are displayed as formatted tables:

```bash
# Indexed values displayed as tables
$ graphcal eval mission.gcl
delta_v[Departure]  = 2.46 km/s
delta_v[Correction] = 0.12 km/s
delta_v[Insertion]  = 1.83 km/s

spacecraft_mass (kg):
╭─────────┬───────────┬────────────┬───────────╮
│         │ Departure │ Correction │ Insertion │
├─────────┼───────────┼────────────┼───────────┤
│ Launch  │      5000 │          0 │         0 │
│ Cruise  │         0 │       4500 │         0 │
│ Arrival │         0 │          0 │      4000 │
╰─────────┴───────────┴────────────┴───────────╯
```

1D indexed values remain as flat lines, while 2D values are shown as table
grids. 3D and higher values are displayed as multiple 2D table slices with
section headers. A unit is shown once in the table caption only when every cell
has the same physical dimension, display scale, and label. Heterogeneous tables
omit the shared caption and label each quantity or complex cell individually.

When a file contains assertions, they are checked after evaluation and printed
below the values:

```bash
$ graphcal eval rocket.gcl
dry_mass   = 1200 kg
fuel_mass  = 2800 kg
...

Assertions:
  fuel_budget    PASS
  fuel_positive  PASS
  pressure_safe  FAIL  (assertion evaluated to false)
                       affected: safety_factor
```

### Plot Output

When a file contains `plot`, `figure`, or `layer` declarations, use the
`--plot` option to render them. Three output modes are available:

**Browser mode** generates a self-contained HTML file with all figures
(rendered via [Vega-Embed](https://github.com/vega/vega-embed), with the
Vega bundles inlined so the page needs no network access and works from
`file://` paths) and opens it in the default browser. Normal evaluation output is still printed according
to `--format`:

```bash
graphcal eval analysis.gcl --plot browser
```

**JSON mode** prints only a JSON array of figure objects to stdout, useful for
piping to other tools or pasting into the
[Vega-Lite Editor](https://vega.github.io/editor/). Normal evaluation output is
suppressed in this mode, and `--format` does not add eval values to stdout:

```bash
graphcal eval analysis.gcl --plot json
```

**HTML file mode** writes the same self-contained HTML page that browser mode
generates to a path of your choice (the path must end in `.html`) without
opening a browser — useful in headless or CI environments. Normal evaluation
output is still printed according to `--format`:

```bash
graphcal eval analysis.gcl --plot report.html
```

Each figure in the array has a `name` and a `spec` (Vega-Lite JSON):

```json
[
  { "name": "curve_a", "spec": { /* Vega-Lite spec */ } },
  { "name": "comparison", "spec": { /* Vega-Lite hconcat spec */ } }
]
```

Each `plot` declaration produces a standalone figure. Each `figure`
declaration produces a combined chart using Vega-Lite `hconcat`. Each `layer`
declaration produces a chart using Vega-Lite `layer`. Plots marked
`#[hidden]` are suppressed from standalone output but still appear in any
`figure` or `layer` that references them.

If no `plot`, `figure`, or `layer` declarations are found, JSON mode prints
`[]` to stdout and a warning to stderr. Browser mode prints the warning and
opens nothing.

See the [Plot Declarations](language/plots.md) reference for the language
syntax.

---

## `graphcal report build`

Build a shareable, self-contained, **interactive** HTML report derived
entirely from the model — no presentation layer to author. This feature is
**experimental**.

```bash
graphcal report build model.gcl
```

Writes `model.report.html` next to the model (`-o`/`--output` overrides the
path). The page is derived from what the model already declares:

| Report section | Derived from |
|---|---|
| Inputs | `param` declarations: every entry param becomes a control |
| Values | `const`/`node` declarations in declaration order; indexed values render as grids |
| Plots | `plot`/`figure`/`layer` declarations, rendered with inlined Vega bundles; single-view charts with a continuous axis get pan/zoom |
| Checks | `assert` results as pass/fail badges |
| Captions | `///` doc comments attached to declarations |
| Provenance | compiler version, SHA-256 of every source file, baseline parameter values, and a copy-pasteable reproduction command |

The controls follow the param's type and domain constraints: a unit-checked
entry field for quantities (plus a slider when finite `min`/`max` bounds are
declared), a checkbox for `Bool`, a stepper for `Int`, a select for
`Key<Index>` over a named index, and a full closed-literal field for
everything else. Readers bind closed typed values under the same rules as
`eval --param` — wrong or missing units are rejected at the input box, values
clamp to declared domains, and expressions are never injected into the
prepared model. Edits re-evaluate the embedded engine (the same compiler and
evaluator as this CLI, compiled to WebAssembly, running in a Web Worker) and
patch values, grids, badges, and charts in place. The moment any input
differs from the as-published baseline, a persistent banner appears with a
one-click reset.

The artifact is a single file with everything inlined — engine, sources,
Vega bundles, styles: it works offline, from `file://` paths, and as an
email attachment. The static baseline renders without JavaScript; charts and
interactivity need it. Output is deterministic — identical sources, engine,
and compiler produce a byte-identical page, and there is no build timestamp.

**Engine bundle.** The CLI embeds a release-matched WebAssembly build of the
evaluator, so interactive reports work from any directory without a separate
installation step. Graphcal developers can override it with `--engine-dir` or
`$GRAPHCAL_REPORT_ENGINE_DIR`, pointing at a
`wasm-pack build crates/graphcal-wasm --target no-modules` output. An invalid
explicit override fails rather than silently using a different engine;
`--static` builds a non-interactive report without embedding the engine.

Models the browser engine cannot run — plugin imports, package
dependencies — fail loudly at build time with the reason; build them with
`--static` instead (the baseline is still computed natively, plugins
included).

Options:

| Option | Description |
|--------|-------------|
| `-o, --output <FILE>` | Output HTML path (default: the model path with a `.report.html` extension) |
| `--markdown <FILE.md>` | Also write a deterministic Markdown rendering for CI diffing |
| `--param 'name=value'` | Bind a param to a closed value for the baseline (same rules as `eval --param`); replayed as the initial control value |
| `--params-json <JSON>` | Bind baseline params from inline JSON (static reports only) |
| `--params-json-file <FILE>` | Bind baseline params from a JSON file or `-` for stdin (static reports only) |
| `--params-json-max-bytes <BYTES>` | Maximum JSON parameter document size (default: 1 MiB) |
| `--root <DIR>` | Project root directory (overrides automatic `graphcal.toml` detection) |
| `--static` | Build a non-interactive report: no embedded engine, no controls |
| `--engine-dir <DIR>` | Override the bundled browser engine (default: `$GRAPHCAL_REPORT_ENGINE_DIR`, then the release-matched embedded engine) |

Exit codes: `0` success; `1` the report was written but contains evaluation
errors or failed assertions (rendered as error chips and FAIL badges); `2` the
project failed to compile or cannot hydrate (nothing is written).

---

## `graphcal model serve`

Prepare a Graphcal project once and expose it as a persistent Tenax model over
stdio Arrow IPC.

```bash
graphcal model serve <FILE> --output <NAME> [--output <NAME> ...] [--root <ROOT>]
```

**Arguments and options:**

| Item | Description |
|------|-------------|
| `<FILE>` | Entry `.gcl` file |
| `--output <NAME>` | Public scalar `Bool` node declared directly in the entry file; repeatable and required |
| `--root <ROOT>` | Explicit project root |

The initial integration implements Tenax stdio protocol version 1 and Arrow
schema contract version 2. It compiles the project and plugins once, emits a
schema-only discovery stream followed by one persistent result stream on
stdout, reads one persistent request stream from stdin, and evaluates request
rows sequentially. Stdout contains Arrow IPC only; human diagnostics and logs
go to stderr.

Tenax v2 exposes every entry parameter, including parameters with defaults.
The complete interface must contain at least one input and one selected output:

| Graphcal input | Tenax v2 requirement |
|----------------|----------------------|
| Real quantity / `Dimensionless` | Finite inclusive `min` and `max`, with finite interval width; dimensioned values carry a canonical SI unit |
| `Int` | Inclusive `min` and `max` representable as `i64` |
| `Key<I>` | `I` is a concrete, non-empty named index; categories are transmitted lexically and decoded by dictionary value |

`Bool`, datetime, complex, algebraic, indexed, coordinate-key, and finite-key
inputs are supported by Graphcal's prepared binding core but are not
representable by Tenax schema v2. They cause model preparation to fail rather
than being flattened or coerced. Selected outputs must currently be explicitly
`pub`, scalar `Bool` nodes; unknown, private, duplicate, and non-Boolean
selections fail before any IPC bytes are written.

Example model:

```graphcal
pub index Mode = { Nominal, Degraded };

param load: Force(min: 0.0 N, max: 10_000.0 N);
param cycles: Int(min: 0, max: 100_000);
param mode: Key<Mode>;

pub node failure: Bool =
    @load > 8_000.0 N && @cycles > 50_000 && @mode == Mode#Degraded;
```

```bash
graphcal model serve reliability.gcl --output failure
```

A model/runtime/domain/assertion failure becomes a failed result row with null
outputs and a diagnostic. Malformed Arrow, schema mismatches, invalid shared
input-domain values, duplicate evaluation IDs, broken pipes, and internal
invariant failures invalidate the process. Clean request-stream EOS finishes
the result stream and exits successfully. See [Tenax Integration](tenax-integration.md)
for lifecycle diagrams and client guidance.

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Clean request EOS and result-stream shutdown |
| `1` | Protocol/process failure after serving began |
| `2` | Project/model configuration failed before IPC startup |

---

## `graphcal format`

Format `.gcl` files. When given a directory, recursively formats regular
`.gcl` files within. Symlinked entries found during directory traversal are
skipped, and explicitly named symlinked paths are rejected rather than followed.

Formatting only ever changes layout, never meaning. After producing the
formatted text, the formatter re-parses it and verifies the result is the same
syntax tree as the input (ignoring source positions). If they ever diverge —
which would be a bug in the formatter, not in your code — formatting fails with
an error instead of writing a file whose meaning might differ from the source.
Changed files are written completely to same-directory temporary files,
synchronized, and atomically renamed into place. Existing permissions are
preserved, but ownership, ACLs, extended attributes, and hard-link identity are
not; an atomic replacement creates a new file identity. If a source changes
after it was read, replacement fails instead of overwriting the newer bytes.
One replacement failure does not prevent later batch targets from being
formatted.

Parentheses are removed only when an exhaustive precedence and associativity
check proves that reparsing preserves the same expression tree, including
prefix, infix, postfix, conversion, call, and delimited contexts. Breakable
layouts target 100 display columns. Long `&&` and `||` chains break
before their operators, with all continuation operators aligned; short chains
remain inline. Long attribute argument lists are expanded to one argument per
line with a trailing comma. Adding a trailing comma to a short source list
requests the same multiline layout; short lists without one remain inline.

```bash
graphcal format [OPTIONS] [PATHS]...
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `[PATHS]...` | Files or directories to format (default: current directory) |

**Options:**

| Option | Description |
|--------|-------------|
| `--check` | Check formatting without modifying files (exit 1 if unformatted) |

**Examples:**

```bash
# Format all .gcl files in the current directory
graphcal format

# Format specific files
graphcal format rocket.gcl hohmann.gcl

# Check formatting in CI (non-destructive)
graphcal format --check
```

Directory traversal errors are hard failures: visible files are still processed
for a complete diagnostic batch, but a partial tree never produces a successful
certification result.

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Formatting completed, or every inspected file was already formatted under `--check` |
| `1` | A file could not be read/formatted/replaced, or `--check` found changes |
| `2` | One or more requested directories could not be traversed completely |

---

## `graphcal check`

Check `.gcl` files without runtime evaluation. This performs parsing, module and name resolution, type/dimension/policy validation, compile-time constant checks, and extern-signature verification. It does not evaluate ordinary nodes, assertions, dynamic unit scales, or callable host functions.

```bash
graphcal check [PATHS]...
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `[PATHS]...` | Files or directories to check (default: current directory) |

**Examples:**

```bash
# Check all .gcl files in the current directory
graphcal check

# Check a specific file
graphcal check rocket.gcl

# Check a directory
graphcal check my_project/
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | No errors found and every requested directory was inspected completely |
| `1` | Source errors detected |
| `2` | One or more requested directories could not be traversed completely |

---

## `graphcal dump`

!!! warning "Experimental debugging output"
    `graphcal dump` pretty-prints internal Rust data structures with `Debug`.
    The command defines no stable schema, and output may change after any
    implementation refactor. It may include complete source text, absolute
    paths, and runtime values; inspect it before sharing or attaching it to a
    public issue. Raw loaded-plugin byte buffers are always redacted.

Debug-print one compiler or evaluator artifact:

```bash
graphcal dump <STAGE> <FILE> [OPTIONS]
```

| Stage | Artifact and stopping point |
|-------|-----------------------------|
| `source` | Physical source path and complete text after a bounded read |
| `tokens` | Parser-facing tokens, formatter trivia, and the first lexical-error span |
| `ast` | Parser-produced `File<Raw>` (`raw-ast` is an alias) |
| `desugared` | `File<Desugared>` after parser sugar removal (`desugared-ast` is an alias) |
| `modules` | Raw `LoadedProject` together with its canonical `ModuleResolver` |
| `hir` | Complete `HirProject` after elaboration and canonical reference resolution, before static checking |
| `tir` | Fully checked raw `TIR` |
| `plan` | Raw `ExecPlan` produced by runtime preparation |
| `runtime` | One raw `RuntimeEvaluation`, including SI values and contained errors |
| `result` | One normal public `EvalResult` |

There is intentionally no `all` stage, JSON format, filtering language, record
envelope, or compatibility guarantee. Run the specific stage needed for a
debugging session. `HirProject` is an actual compiler continuation rather than
a dump-only projection: it owns every file-root and inline-DAG HIR module, and
static checking consumes it to produce `CheckedProject`.

`source`, `tokens`, `ast`, and `desugared` never follow imports. `modules` stops
before HIR lowering. `hir` resolves the complete project but does not construct
TIR, check expression types/dimensions, evaluate constants, or verify host
signatures. Consequently, `hir` remains available when a later static check
fails. `tir` and `plan` do not evaluate ordinary runtime parameters, nodes,
assertions, dynamic unit scales, or callable host functions. Only `runtime` and
`result` perform ordinary runtime evaluation.

Examples:

```bash
graphcal dump tokens model.gcl
graphcal dump ast model.gcl
graphcal dump modules src/mission/main.gcl --root .
graphcal dump hir model.gcl
graphcal dump tir model.gcl
graphcal dump plan model.gcl
graphcal dump runtime model.gcl --param 'mass=1200 kg'
graphcal dump result model.gcl --params-json-file params.json
```

Every stage accepts `--root`. `runtime` and `result` additionally reuse the
bounded `--param`, `--params-json`, `--params-json-file`, and
`--params-json-max-bytes` parser from `graphcal eval`.
A failed stage prints the normal diagnostic and exits with code `2`. A completed
runtime/result dump containing node, assertion, or plot failures is still
printed and exits with code `1`.

---

## `graphcal graph`

!!! warning "Experimental"
`graphcal graph` is experimental. The DOT output (node naming, labels,
styling) and the CLI surface (flags, formats) may change in any release
while the [DAG visualizer](https://github.com/graphcal-lang/graphcal/issues/512)
design evolves. The command prints a warning on stderr to that effect;
stdout carries only the exported graph.

Export the dependency graph of a `.gcl` file as text for external rendering
tools. The graph is a one-way projection of the compiled program: `param`,
`const node`, and `node` declarations become vertices; every `@` reference
becomes a directed edge from the value being read to the declaration reading
it. The projection retains source-module and concrete include-instance
provenance, including the reusable template behind each instance. Assertions,
plots, figures, and layers are not part of the dataflow and are omitted.

```bash
graphcal graph [OPTIONS] <FILE>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<FILE>` | Path to the `.gcl` file (required) |

**Options:**

| Option | Description |
|--------|-------------|
| `--format <FORMAT>` | Output format: `dot` (default; currently the only format) |
| `--view <VIEW>` | Composition detail: `grouped` (default), `flat`, or `module` |
| `-L, --max-depth <DEPTH>` | Positive number of grouped composition levels to display |
| `--root <ROOT>` | Project root directory (overrides automatic `graphcal.toml` detection) |

The three views support overview-to-detail workflows:

- **`flat`:** every declaration and dependency edge, without composition
  boundaries. Repeated declaration names are owner-qualified so the detailed
  debugging view remains unambiguous.
- **`grouped` (default):** the same declaration-level graph, placed in real
  Graphviz `cluster_*` subgraphs for the root module, inline DAG definitions,
  concrete include instances, and referenced external modules. Clusters nest
  according to composition, include labels identify their reusable template,
  and cross-cluster edges connect the actual source and receiving declaration
  nodes. Parameter bindings therefore terminate at the bound parameter rather
  than at the receiving cluster border.
- **`module`:** one node per source module or include instance. Internal wiring
  is hidden, declaration dependencies are collapsed into deduplicated
  module-level dataflow edges, and dashed edges show containment or
  instantiation.

`-L`/`--max-depth` limits expansion in the grouped view using `tree -L`-style
levels. The root is level 1: `-L 1` shows root declarations and collapses each
direct child DAG to a summary; `-L 2` expands direct children and collapses
grandchildren. A summary reports how many declaration values its whole hidden
subtree contains. Dependencies crossing a collapsed boundary are redirected to
the summary and deduplicated, so module-level dataflow remains visible rather
than silently disappearing. Omitting `-L` fully expands every cluster. The
option is rejected with `flat` and `module` views.

The output is deterministic (declarations keep source order, edges are sorted),
so exported graphs diff cleanly in version control. DOT statement identifiers
(`n0`, `n1`, …, collapsed summaries `s0`, `s1`, …, clusters `cluster_c0`,
`cluster_c1`, …, and modules `m0`, `m1`, …) are opaque renderer-local ids;
semantic compiler names appear only as labels. This
prevents Graphviz from merging separate package versions or source modules and
concrete instances that happen to have the same display path.

**Exit codes:**

| Code | Meaning |
|------|---------|
| `0` | Graph exported successfully |
| `2` | Compile error (parse or type check) or I/O error |

**Examples:**

```bash
# Render the default composition-preserving graph to SVG
graphcal graph rocket.gcl | dot -Tsvg -o rocket.svg

# Keep root detail but collapse direct child DAG internals
graphcal graph composed-model.gcl -L 1 | dot -Tsvg -o overview.svg

# Expand direct child DAGs but collapse nested children
graphcal graph composed-model.gcl -L 2 | dot -Tsvg -o two-levels.svg

# Render an architecture overview with one node per DAG
graphcal graph rocket.gcl --view module | dot -Tsvg -o rocket-modules.svg
```

In declaration-level views, `param` declarations are ellipses (the graph's
inputs), `const node` declarations are rounded boxes, `node` declarations are
plain boxes, public calculated outputs have a green double border, and values
imported from other files are dashed boxes. Each vertex's label shows the
declaration name and its resolved type. Grouped and module views include a
legend. When multiple declarations share an ordinary display path, qualified
labels put package identity on a separate `package …` line and distinguish
lexical (`.`) from concrete-instance (`@`) hierarchy edges. DOT labels never
invent a `::` separator that is not part of Graphcal path syntax.

---

## `graphcal lsp`

Start the Language Server Protocol (LSP) server. The server communicates over stdin/stdout and is intended to be launched by an editor.

```bash
graphcal lsp
```

This command has no additional options. See [Editor Setup](editor-setup.md) for how to configure your editor to use the LSP.

### LSP Features

The LSP server provides:

- **Diagnostics** -- Real-time parse errors, dimension mismatches, unknown references, assertion failures
- **Inlay hints** -- Computed param/node values displayed inline
- **Go to definition** -- Navigate from references to declarations
- **Hover** -- Show resolved type and dimension information
- **Find references** -- Find all usages of a declaration
- **Document symbols** -- Outline view of all declarations
- **Formatting** -- Format the current document
- **Document links** -- Clickable `import` paths
