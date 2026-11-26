# Changelog

Release notes for Graphcal. Each release section becomes the body of its GitHub Release.
Add a `## v<version>` section before running the release workflow.
Earlier releases are listed on [GitHub Releases](https://github.com/graphcal-lang/graphcal/releases).

## v0.0.1-alpha.46

In this release, a `todo` hole blocks only the values that actually read it. A table with one unfinished entry keeps the values of its other entries.

**Some existing Graphcal source programs need to be modified.** Each program that needs a change fails to compile; see [Breaking Changes](#breaking-changes).

### Breaking Changes

- Civil (offset-free) datetime literals, the first argument of `datetime("…", "Area/City")` and the argument of `epoch<S>("…")`, must be exactly `YYYY-MM-DDTHH:MM:SS[.fraction]`, like the offset form. A space instead of `T`, omitted minutes or seconds (`T12:00`), the basic format (`20241105T1200`), comma fractions, signed six-digit years, and bracketed annotations such as `[u-ca=hebrew]`, which were silently ignored, are now D028. No accepted literal changes its value. (_#2360_)

  ```graphcal
  node a: Datetime = datetime("2024-11-05T10:00", "Asia/Tokyo");     // D028
  node b: Datetime = datetime("2024-11-05T10:00:00", "Asia/Tokyo");  // OK
  node c: Datetime<TAI> = epoch<TAI>("2024-11-05 12:00:00");         // D028
  node d: Datetime<TAI> = epoch<TAI>("2024-11-05T12:00:00");         // OK
  ```

- A module alias that takes a built-in function or constant name is N009 at the alias, as selective-item aliases already were. This also covers implicit aliases (a file named `sqrt.gcl` imported without `as`) and module-form `include` aliases. Common words such as `second` and `outer` are built-in names. Plugin aliases are unchanged. (_#2382_)

  ```graphcal
  import p.lib.f as sqrt;   // N009: module alias `sqrt` shadows a built-in name
  import p.lib.f as f_lib;  // OK
  ```

### Unfinished Values

- A `todo` hole is evaluated like any other expression. A declaration is TODO only when evaluation reaches its hole, and a reader is BLOCKED only when it actually reads an unfinished value. A hole in an unselected branch no longer blocks the result. No formula can test whether a value is unfinished, so a computed value is the same however the holes are filled later. The `eval` exit status stays 1 while the model has a hole, even when no result reached it, and the CLI adds a note in that case. `const node` follows the same rule at compile time. Recurrence groups still block members with unavailable outside inputs. (_#2410_)

  ```graphcal
  param fast: Bool = true;
  node speed: Velocity = todo<Velocity> {};
  node time: Time = if @fast { 10.0 s } else { 100.0 m / @speed };  // 10 s, was BLOCKED
  node distance: Length = @speed * 2.0 s;                            // still BLOCKED
  ```

- Each element of an indexed value has its own outcome. A cell that reaches a hole, reads a value that has none, or fails no longer removes the values of the other cells. A read of one cell needs only that cell, a `for` keeps every cell it can compute, and a whole read such as `sum(...)` still needs every cell. Domain checks and evaluation failures apply per cell. A struct value stays one node. (_#2411_)

  ```graphcal
  index Part = { Wheel, Bracket, Panel };
  node mass: Mass[Part] = table[Part] {
      Wheel: 2.0 kg;
      Bracket: todo<Mass> {};
      Panel: 5.0 kg;
  };
  node wheel_mass: Mass = @mass[Part#Wheel];                   // 2 kg, was BLOCKED
  node with_margin: Mass[Part] = for p: Part { @mass[p] * 1.1 }; // Wheel and Panel have values
  node total: Mass = sum(@mass);                               // still BLOCKED
  ```

- Per-cell outcomes reach constants, inline DAG calls, `scan`, and `unfold`. A `const node` keeps its finished cells, an inline call's arguments and outputs keep theirs, and `@f(...)::out[2]` reads one cell. A `scan` or `unfold` in a result position keeps the steps before the first step without a value. (_#2414_)

  ```graphcal
  index Step = range(0.0 s, 3.0 s, step: 1.0 s);
  // state[0 s] = 1 and state[1 s] = 2 have values; state[2 s] and state[3 s] are TODO
  node state: Dimensionless[Step] = unfold(Step, 1.0, |x, prev_t, t|
      if coord(t) < 1.5 s { x * 2.0 } else { todo<Dimensionless> { x } });
  ```

- A domain bound is C010 only when its evaluation actually reads an unfinished constant or an unfinished cell of one. Before, any bound that mentioned an unfinished constant was rejected, even in an unselected branch or through a finished cell. Programs accepted before are still accepted. (_#2414_)

  ```graphcal
  index Part = { Wheel, Bracket };
  const node limit: Length[Part] = table[Part] { Wheel: 1.0 m; Bracket: todo<Length> {}; };
  param wheel_size: Length(max: @limit[Part#Wheel]) = 0.5 m;      // OK, was C010
  param bracket_size: Length(max: @limit[Part#Bracket]) = 0.5 m;  // still C010
  ```

- Constructor fields and plugin arguments merge the reasons of every unfinished input, as operations already did. `Pair(first: @a, second: @b)` names both `a` and `b`, and a failing field is no longer hidden behind an earlier unfinished one. A plugin call with an unfinished argument reports it before any ABI check of the other arguments. Values do not change. (_#2419_)

### Output of Unfinished Values

- `graphcal eval` prints one line per cell of a partly unfinished value, such as `mass[Wheel] = 2 kg` and `mass[Bracket] = TODO — formula unfinished`. (_#2411_)
- `eval --format json` adds a `cells` object, with the same `index`/`entries` shape as an indexed value, to an unavailable outcome. Language-server inlay hints show `{ Wheel: 4, Bracket: TODO, Panel: 2 }`, report cards list each cell in HTML, Markdown, and the interactive runtime, and the wasm engine's `incomplete` and `error` outcomes carry `cells`. (_#2413_)

  ```json
  {
    "declaration": "mass",
    "status": "todo",
    "cells": {
      "index": "Part",
      "entries": {
        "Wheel": { "si_value": 2.0, "display_value": 2.0, "unit": "kg" },
        "Bracket": { "declaration": "mass", "status": "todo" }
      }
    }
  }
  ```

- `graphcal view` shows `TODO` or `BLOCKED` in place of a value in scalar boxes, the formula bar, and code inlays, and beside a table's name. Missing table cells are muted. Hovering names the unfinished formulas a value waits on, for example `BLOCKED: waits on the unfinished cell_voltage, harness_loss`. A failed value still shows `—`. (_#2404_, _#2413_)

### CLI

- `graphcal eval --plot FILE.svg` writes one figure as static SVG, rendered by the bundled Vega/Vega-Lite in an embedded QuickJS engine. The SVG matches the HTML rendering and needs no browser, Node.js, or network. With several displayed figures, `--figure NAME` is required; a missing or unknown name exits 1 and lists the figures. `--figure` with `browser`, `json`, or `.html` output exits 2 instead of being ignored. Vega-Lite warnings go to stderr. (_#2408_)

  ```sh
  graphcal eval model.gcl --plot figure.svg                   # the only displayed figure
  graphcal eval model.gcl --plot figure.svg --figure curve_b  # one of several
  ```

- `graphcal format -` reads one source from stdin and writes the formatted text to stdout. `--check -` writes nothing to stdout and exits 1 with `<stdin> would be reformatted` on stderr when the input is not formatted. `-` must be the only path (exit 2 otherwise), and stdin is bounded by the source-file size limit. (_#2420_)

  ```sh
  graphcal format - < model.gcl > formatted.gcl
  graphcal format --check - < model.gcl
  ```

### Formatter

- A `///` line with trailing spaces or tabs no longer fails the formatter's equivalence check. Doc text drops trailing spaces and tabs, so hover, reports, and the view show the same text before and after formatting. Two trailing spaces no longer make a Markdown hard line break. (_#2418_)

### Diagnostics

- V001 for an import of a private DAG is labelled at the import's module path, not the whole file. A module-form `include` with a private DAG on its path is labelled the same way. (_#2377_)
- A type mismatch in a rebound include parameter default (D002) is reported at the binding argument. Before, the underline was drawn from the dependency's type annotation onto unrelated text of the importer. (_#2378_)
- A dimension exponent overflow in a rigid view (D010) is reported at the dimension's declaration, or at the include that materializes the instance, not over the whole file. (_#2380_)
- An inline DAG-call `Fin(...)` binding accepts any closed Nat expression, as include bindings do, and reports I001, I037, I039, or I040 instead of M016. (_#2379_)
- A failed `x ^ @y` with a runtime exponent reports "power operation", like `x ^ 2`. (_#2357_)

### Language Server

- N037 (a unit name that equals a local in a quantity literal) offers the quick fix "Close the quantity literal with parentheses", No fix is offered for a leading unit, a `->` target, or a unit inside a parenthesized unit group. (_#2390_)

  ```text
  before:  2.0 m / s     (N037: `s` is a local in scope, not the unit second)
  after:   (2.0 m) / s
  ```

- An unsaved overlay file aliased onto another path, such as `/p/graphcal.toml -> sub/config.toml`, no longer makes `/p` resolve as `/p/sub`. (_#2376_)

### Reports and View

- In the interactive report, pinned inputs and values keep their descriptions, clamped to two lines. A pinned scalar or one-field-record input also keeps its override and draft markers. (_#2402_)
- The browser engine takes up to 1,024 files per project instead of 16, so the web viewer opens larger projects. The byte limits are unchanged. (_#2372_)
- The view uses its own tooltips instead of the native `title`, which a VS Code webview never shows. Reasons and doc comments show at once, and click hints after half a second. (_#2404_)

### Documentation

- `built-ins.md` states the canonical civil datetime form next to the offset form. (_#2360_)
- The "Unfinished nodes" and "Constants" sections of `computation-model.md` describe the runtime and per-cell rules and the read-based C010 rule. (_#2410_, _#2411_, _#2414_)
- The CLI reference and the plots page cover `--plot FILE.svg` and `--figure`. The CLI reference mentions `graphcal format -`. (_#2408_, _#2420_)
- `grammar.ebnf` states the allowed `pub` / `pub(bind)` prefix per declaration kind, matching the parser, and defines the missing `visibility` nonterminal. (_#2375_)

### Dependency, Toolchain, and Workflow Maintenance

- `wasmi`, which runs plugins, goes from 2.0.0 to 2.0.3 in three steps, because 2.0.0, 2.0.1, and 2.0.2 were each yanked. The releases fix miscompilations, a use-after-free, a segfault after a tail call to a host function, and memory-unsafe `v128.store8_lane` / `v128.store16_lane` execution. Each step used the documented minimum-publish-age override. (_#2373_, _#2398_, _#2424_)
- Updated `url` to 2.5.9 for RUSTSEC-2026-0592. Graphcal does not call the affected `Url::origin()`. (_#2406_)
- Ignored RUSTSEC-2026-0543 (`chrono` unmaintained, pulled in only by `arrow-array`) and RUSTSEC-2026-0574 to RUSTSEC-2026-0582 (`heapless` 0.8, pulled in only by `faster-hex` through gix, in code a Graphcal build does not compile), tracked in #2358 and #2374. (_#2359_, _#2373_)
- Added `rquickjs` to the CLI for SVG rendering. `graphcal-wasm` does not depend on it. (_#2408_)
- Stray dependency specifiers moved to `[workspace.dependencies]`; `Cargo.lock` is unchanged. (_#2392_)
- Updated JavaScript tooling: `@playwright/test` to v1.65.0 and pnpm to v12.18.0. (_#2349_)
- Updated `zensical` to v0.1.0. (_#2348_)
- Updated GitHub Actions: `leanprover/lean-action` to v1.7.1 and `taiki-e/install-action` to v2.90.3. (_#2349_)
- Bumped cargo-hawk to 0.1.18, built on Rust 1.100.0 like the workspace. (_#2354_)

### Internal

- Removed 78 unreferenced functions found by coverage and a deletion probe, then unreferenced trait impls, write-only fields, and functions only their own unit tests called. Implicit access paths such as `Deref` for `Spanned<T>` and conversions that duplicated named constructors are gone. (_#2350_, _#2351_, _#2355_)
- Removed the model API left over from the Tenax integration (`PreparedModel`, `evaluate_model_row`, `bind_named_key`), with the never-emitted binding errors O011 and O012. (_#2356_)
- `PipelineError` is replaced by `SemanticError`, and inline calls resolve through the scope's callee table instead of a copy in the execution plan. (_#2352_, _#2353_)
- Deduplicated logic found by a duplicate-code candidate search, including one load-and-prepare helper for `eval`, `dump`, and `model`, one portable-path rule for package and plugin paths, and a bounds-checked `IndexAxis::key_at`. (_#2357_)
- `UncheckedTir` and `InstantiatedTir` can be constructed only in `tir/typed`. N009 and M011 carry typed kind fields. (_#2389_, _#2381_)
- Shared duplicated helpers and removed dead code in `web/view`. (_#2394_, _#2395_)
- Corrected the codebase reading guide and doc comments that contradicted the implementation. (_#2387_, _#2391_)
- Added tests for datetime literal validators at every range bound, calendar arithmetic, duration limits, leap-second readings, and LSP routing to a symlinked dependency. (_#2388_, _#2393_, _#2421_)
- Bumped the workspace version to v0.0.1-alpha.46 and added these notes to `CHANGELOG.md`. (_#2425_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.45...v0.0.1-alpha.46>

## v0.0.1-alpha.45

In this release, an `Int` can scale a quantity in `*` and `/`, and a `Dimensionless` base accepts a runtime `Int` exponent. `const node` formulas accept `todo` holes.
Most of the other changes come from the 2026-11-06 codebase review (#2241). They close silent-misbehavior holes in includes, units, datetimes, numerics, the package loader, plugins, and the model protocol. They also remove quadratic and exponential slowdowns, make output deterministic, and make LSP edits safer.
The workspace toolchain moves to Rust 1.100.0, and Rust-heavy CI jobs now run on Namespace runners.

**Some existing Graphcal source programs need to be modified.** Each program that needs a change fails to compile with a dedicated diagnostic; see [Breaking Changes](#breaking-changes). Some bug fixes change results without a compile error: a file-root include that was silently dropped now runs and reports its assertions, TAI→UTC readings in the 36 s before a leap second move by 1 s, and `sum()`, integer powers, complex `*`, `cross()`, and datetime differences and shifts can change in the last bits. See [Numerical Accuracy](#numerical-accuracy). The lockfile format, the model protocol, and the model client's Rust API also change; see [Lockfile, Model Protocol, and Client Changes](#lockfile-model-protocol-and-client-changes).

### Breaking Changes

#### Numbers and Units

- `to_float(n)` converts exactly. An `Int` that binary64 cannot represent, which happens only beyond 2^53 in magnitude, is an evaluation error, or a compile error in a `const node`. Values such as `2^53` or `-2^62`, which binary64 represents exactly, convert as before. (_#2236_)
- Unit scales and coordinates are typed and evaluated with the same rules as node bodies. (_#2238_)
  - `(1 / 3) m` in a unit scale is now D043, the same as with an `@` reference. Write `(1.0 / 3) m`.
  - `(1.5 + 1) m` is now D001, because `+` never mixes `Int` and real numbers.
  - An `Int` coordinate bound or step, as in `range(0, 10.0, step: 1.0)`, is now D001.
  - A bare integer scale that binary64 cannot hold exactly, such as `9007199254740993 m`, is an error instead of being rounded.
  - A non-finite or underflowing intermediate value in a unit scale is an error. Before, only the final value was checked.
- A float literal with a nonzero significand that parses to zero, such as `1e-400`, is P003. A constant or coordinate whose SI value underflows, such as `1.0e-30 tiny`, is a compile error instead of a silent `0`. Complex scaling and division at runtime report underflow like the real operations. (_#2307_)
- An `Int` bound on a `Dimensionless` domain, such as `Dimensionless(min: 0)`, is the new compile error C011. Write `0.0`, or `to_float(...)` for a computed bound. The language server offers the replacement as a quick fix. C008 is retired. (_#2335_)

#### Datetimes

- A leap-second literal (`:60`) is rejected with D028 in every constructor and time scale. The hint gives the TAI spelling of the instant. Before, the seconds were clamped to `:59`. (_#2283_)
- `Datetime<UTC> - Datetime<UTC>`, `Datetime<UTC> ± Time`, and `Time + Datetime<UTC>` are the new compile error D044. Its help suggests `to_tai(...)`. UTC comparisons still work, and other time scales keep their arithmetic. (_#2283_)
- `to_jd`, `to_mjd`, `from_jd`, and `from_mjd` are removed. A call reports the new N038 with migration help: use `Datetime` differences in the right scale. (_#2283_)
- `to_unix` accepts only `Datetime<UTC>` (D001, help `to_unix(to_utc(@t))`), and `from_unix(Int)` is rejected (D001, help `from_unix(to_float(@n))`). (_#2283_)

#### Names, Includes, and Imports

- A unit name inside a quantity literal or a `->` target that equals a local in scope (a `match`, `for`, `scan`, or `unfold` binder) is the new N037. Before, `2.0 m / s` with a local `s` silently meant m/s. The help suggests `(2.0 m) / s` or renaming the local. (_#2278_)
- A selective-include, include, or self-import alias that takes a reserved spelling is N009. Before, `include p.lib()::{ unit ft as m }` made `1.0 m` mean one foot. (_#2280_)
- A unit over a required dimension port (`pub(bind) dim T;`) is D036, because the binding owns the canonical unit. (_#2280_)
- `base unit X: D = …;` is P001. The help points to `const unit` or `unit`. (_#2280_)
- An include, selective or module-form, that rebinds a port and exposes a type depending on that port without selecting it is the new V008. Before, the included value kept the template type, so `count` and `sum` silently used the wrong entries. The help names every `type T` to add to the selection, and the language server offers a quick fix. (_#2281_, _#2314_)
- Same-file includes and imports resolve from where the path is written: at file root, only the file's top-level DAGs count; in a DAG body, its children and then its siblings. A file-root include no longer disappears when the file also contains an unrelated nested DAG of the same name, or when the name matches the file stem. Such includes now run, so their assertions report, and dropped file-root imports now bind their items. (_#2277_)
- The file stem means the file itself only in standalone files. In a package file, a DAG-body path spelled like the file's stem is the new M037. Its help gives the file's module path. A standalone self-include fails with G009. (_#2277_)
- Selective-include items keep their namespace. After `{ type mode, pick }`, `@mode` is N002 instead of reading the dependency's node, and selecting only a constructor no longer attaches it to an unrelated same-spelled importer type (D002). (_#2308_)

#### Packages and Plugins

- Package files and directories must be lower_snake_case, non-keyword identifiers, as the docs already said. `MyModule.gcl`, `my-helpers.gcl`, and `match.gcl` fail with the new M040. (_#2302_)
- An import spelled with different letter case than the file on disk is the new M041, so no file loads under two identities on a case-insensitive filesystem. (_#2302_)
- A module symlink that escapes the project root is M008 instead of silently resolving to an inline DAG of a shorter prefix. A module that exists but cannot be read is the new M039 instead of M015 "failed to parse graphcal.toml". (_#2302_)
- `import plugin "….wasm"` in a `.gcl` file that has a `graphcal.toml` above it but lies outside the package namespace is the new M038. Before, such a file loaded plugins without checking the lockfile pins. Ad-hoc files with no manifest still load plugins unpinned. (_#2304_)
- Each `graphcal:demo` function registers its real signature, so a declaration with the wrong dimensional shape fails with W005. (_#2304_)
- An extern generic binder named like a visible static name, such as `fn inverse<Length: Dim>(…)`, is N027 instead of silently meaning "any dimension". (_#2304_)

### Lockfile, Model Protocol, and Client Changes

These changes do not require editing Graphcal source programs.

- **Lockfile format version 2.** Every `tree_hashes` entry names its algorithm (`{ algorithm = "graphcal-source-tree-v1", sha256 = "…" }`), and `requested_rev` is gone. Version 1 locks are still read, and their pins carry over. Writing always produces version 2. (_#2301_)
- `graphcal deps lock` keeps the previous lock's pins after a toolchain upgrade. Before, it silently dropped every pin written by another version. An unreadable or self-contradictory previous lock is now an error. (_#2301_)
- `graphcal deps lock` rejects dependency commits whose paths differ only in letter case, on every host, and fails on incomplete checkouts. It rejects `%` in SSH users and hosts, and path components that start with `-` or `%2D`. URL errors no longer show credentials. (_#2301_)
- **Model protocol v3.** v2 clients are rejected. Dimensions and indexes travel by typed identity `{package, module, name}`. Unsupported ports carry a stable reason code. Nested outputs are null exactly under a null parent. Fingerprints change for every model. (_#2311_)
- `DagId` is the module path the user writes, without the source directory (`src/rocket/x.gcl` → `rocket.x`). This changes diagnostic text, graph labels, and node ids in saved view links. (_#2311_)
- Unit spellings with surrounding whitespace, including `--unit` values, are rejected. (_#2311_)
- The model client's Rust API changes: `InputHandle::canonical_domain` (was `domain`), `PortWire::Unsupported { reason, message }`, `ResolvedUnit::from_si(..) -> Result`, and constructor positions taken from the type. `SessionBuilder::request_timeout` is new. (_#2311_)

### Language Features

- An `Int` can scale a quantity in `*` and `/`. `Int * Q`, `Q * Int`, `Q / Int`, `Int / Q`, and the same forms for `Complex<D>` are accepted. `+`, `-`, comparisons, and `/` on two `Int` values still do not mix types, so a truncated quotient cannot reach quantity arithmetic. The conversion is exact. The D043 help now suggests `to_float(a) / b`. (_#2237_)

  ```graphcal
  param p_unit: Power = 2.8 W;
  param qty: Int = 4;
  param r: Length = 0.5 m;
  node p_total: Power = @p_unit * @qty;  // 11.2 W, was D001
  node c: Length = 2 * PI * @r;          // Int literals too
  ```

- `@x ^ @n` with a runtime `Int` exponent is accepted when the base is `Dimensionless`. The value is bit-identical to `@x ^ to_float(@n)`. A dimensioned base keeps D005. (_#2240_)
- A `const node` formula accepts a `todo<T> { … }` hole. The hole's interface may name only constants. An unfinished constant has no value: declarations that read it become BLOCKED, including qualified references without `@` (`alias::c`), which now count as reads. A domain bound that reads an unfinished constant is the new compile error C010. `eval` reports constants as TODO or BLOCKED, the language server marks them, and `graphcal graph` draws them as dashed rounded boxes. (_#2230_)

  ```graphcal
  const node v_exhaust: Velocity = todo<Velocity> { @g0 };
  ```

- `pub` include items are re-exported in their selected namespace, so a facade `include mid()::{ dim Foo }` works. Values, plots, constants, and assertions (including `#[expected_fail]`) work through facades. (_#2308_)
- A unit scale may read an `Int` param, as in `param n: Int = 2; unit u: Money = (@n) USD;`. Coordinates accept `Int` scaling and powers. `%`, `to_float`, and `div_*` work in unit scales. (_#2238_)
- A file module may mention its own required type port (`pub(bind) type T; param b: T;`). Evaluating such a module directly as the entry reports I010. (_#2316_)
- A DAG input binding is parsed with the grammar its marker selects, so `type T: Vec3<Length, Eci>` parses. Applied types and derived dimensions are still rejected as binding values, now with M016 help that names the binding kind. (_#2330_)

### Unfinished Declarations in `check`

- `graphcal check` and `--deny-todo` count a file's own unfinished declarations plus those of other modules its declarations reach. An unused TODO in an imported module or dependency no longer fails the importer. Checking a package root still requires the whole package to be complete. Reads by plots, figures, and layers count. The summary says "unfinished declaration(s)". (_#2228_)
- A bound include port that reads an unfinished constant (`x: c::c_todo`) counts as a dependency, so `check --deny-todo` fails where `eval` reports BLOCKED, and `graphcal graph` draws the edge. (_#2276_)

### Numerical Accuracy

- `sum()` and `mean()` use an exact accumulator, so they are correctly rounded and order-independent. `mean()` of one million values takes 34 ms instead of 34 s. `product()` checks range only on its final result. (_#2295_, _#2307_)
- Every exact integer power goes through one path, so `@x ^ 3` equals `@x ^ @n` and `2.0 ^ -1070` no longer underflows falsely. `Int ^ Int` handles bases 0, 1, and -1 for any exponent. (_#2307_)
- Complex `*` and `cross()` compute `a*b + c*d` exactly and round once. (_#2307_)
- The `solve` residual check works near the floating-point range limit, so `solve([[1e308]], [1e308])` passes. (_#2307_)
- TAI→UTC keeps the old offset until UTC midnight. Before, readings in the 36 s before each leap second were one second early. An instant inside a leap second reads as the repeated `23:59:59`. (_#2283_)
- Datetime differences, `to_unix`, and shifts are computed from the exact nanosecond count, and seconds round half-to-even to nanoseconds. A 1 ns gap is exactly ±1e-9 s. (_#2283_)

### Evaluation and `model serve`

- The evaluation work budget (10M operations) is removed. It was shared across a session's assertions, plots, and bounds but fresh for each recurrence element, so results depended on unrelated work. Programs that hit it now finish. Long-running work is stopped by cancellation, which now also reaches DAG calls, aggregations, and `argmin`/`argmax`. (_#2295_)
- `graphcal model serve --request-timeout SECONDS` cancels a request batch that runs past the deadline and ends the session with exit code 1. Without the flag, no deadline applies. (_#2295_)
- `graphcal model serve … --print-request-schema` prints the exact request Arrow schema. (_#2311_)
- A domain bound published in a chosen unit is rounded inward, so sending it back is accepted. An output that overflows in its chosen unit fails only its row, not the session. Parameter and output names must be disjoint. (_#2311_)
- Coordinate key lookups use binary search. (_#2295_)

### Robustness and Performance

- Deep constant expressions (20k-term unit scales or coordinate bounds) no longer crash `check` and the language server. Nested failing constructs in HIR error recovery cost linear time instead of 2^depth. (_#2296_)
- Include expansion and instantiation observe cancellation. 800 includes that rebind a defaulted dimension take 0.2 s instead of 11 s. `Fin(N)` and coordinate axes compute keys on demand, so 20 includes of a `Fin(1000000)` model use 26 MB instead of 2 GB. (_#2296_)
- The parser tokenizes once. A 1 MB file with a comment per declaration took 8.5 s to check before. Generic calls are detected with a bounded token-level lookahead, so `Flags(low: a < b, high: a > (c))` parses as two comparisons. The rule is documented in `grammar.ebnf`. (_#2292_)
- The formatter's blank-line and table-label lookups are linear. A table with duplicate row or column labels prints verbatim instead of failing. (_#2292_)
- Mounting a large dependency is linear (a 4,000-file dependency took about 9 s on every load). (_#2302_)

### CLI

- JSON parameter nesting deeper than 64 object levels is rejected, and arrays are rejected without being descended into. Before, cost grew with the square of the depth. (_#2293_)
- FIFOs no longer hang `check`, `format`, or `--params-json-file`. (_#2293_)
- Lockfile creation never overwrites a concurrently created file, and replacement re-checks the destination before renaming. New files respect the umask. (_#2293_)
- A closed stdout pipe exits quietly with status 141 instead of panicking with 101. (_#2293_)
- `check target src` and `format --check target src` no longer skip `target`: skipped directories apply only below the walk root. (_#2302_)
- `eval --plot json` writes failing values and assertions to stderr, so stdout keeps only the figure array. (_#2317_)
- `model describe` and `model unit` render compile errors with file, line, and snippet. (_#2317_)
- A repeated `::` in a JSON qualified name gets its own error. `plugin new` prints the real release artifact path. (_#2317_)
- Empty cache variables count as unset, and a relative `XDG_CACHE_HOME`, `HOME`, `LOCALAPPDATA`, or `USERPROFILE` is ignored. (_#2301_)
- Git dependencies are fetched with depth 1. (_#2339_)

### Deterministic Output

- Domain bounds, dynamic unit scales, nominal definitions, assertion aliases, requested plots, include static bindings, and CLI parameters keep a fixed order. `eval`, `report build`, and `model serve` output, and the choice of which of several errors to report, no longer vary between runs. (_#2310_)

### Language Server

- Hover, completion, and other features run off the serving task and can be cancelled, so one slow request no longer freezes the others. (_#2294_)
- References and rename are linear in source size (10.8 s for 4,000 references before). A 300-file `git checkout` schedules one analysis per affected root. Inlay-value formatting is lazy. (_#2294_)
- Compile errors reach the open buffer in package mode and for a root opened through a symlink. Before, they went to an internal `pkg-…:` URI or to the canonical path. (_#2282_)
- Rename re-checks the edited project with the compiler and refuses with its message, or when a reference would retarget. `prepareRename` returns the reason for a refusal. Renaming an extern function is refused. (_#2304_, _#2314_)
- The add-visibility quick fix (V002/V003/V006) replaces an existing `pub`. `#[expected_fail(Index#Label)]` labels support references, definition, and rename. The outline nests DAG members and constructors. Untitled buffers are analyzed. (_#2314_)

### Reports and View

- Interactive reports bind multi-axis structured values, and the Reproduce command prints a canonical `table[...]` spelling. (_#2312_)
- A fatal engine message or crash replaces the report engine, and a repeat failure on the same bindings stops it. Every non-evaluated outcome rejects the pending submission. (_#2312_)
- Doc captions are keyed by declaration identity. Struct leaves of multi-axis values are listed field by field in reports and `graphcal eval`. The Markdown report escapes user text. (_#2312_)
- Plot datetime axes are titled "Datetime (UTC)" and use a UTC scale, and tooltip and text channels show the RFC 3339 UTC instant. Before, browsers showed local time with no indication. (_#2283_, _#2312_)
- In the view, an inline-DAG instance's editable cells come from its own body. A crafted view address can no longer inject lines into the copied patch. (_#2305_)
- The view can be deployed to its own origin, `view.graphcal.org`, with real HTTP security headers. `graphcal.org/view/` keeps serving the view, with a meta CSP generated from the same policy. (_#2305_)

### Plugins

- wasmi runs in deterministic mode, so NaN bits no longer follow the host CPU. (_#2304_)
- Extern copies across files follow the single-file structural merge rule, so binder-renamed copies compile and real conflicts report W001 in the declaring file. (_#2304_)
- The plugin manifest rejects unknown fields in parameter and result kinds, and an oversized browser package layout is rejected. (_#2304_)

### Diagnostics

- A cycle inside an included instance is reported at the include, not at an unrelated line or out of bounds. (_#2282_)
- `param x` together with `pub node x` reports N001 instead of panicking. A second module alias with the same name reports N001 at its span. (_#2331_)
- A private dimension or unit in a static definition is reported as private, not unknown. An instance dimension overflow is reported at the include. A map-literal entry whose type differs from the first entry is D001. (_#2331_)
- Parse errors name the token actually found, and messages no longer show internal slots, owner-qualified ids, or nested backticks. Help texts for S008, A007, N009, N001, and D013 match the language. (_#2331_)
- Each mistake has one code: `Fin(a*b)` overflow is I039/I040, an index applied like a type is N021, and an unknown module alias is M006. A002, which was never emitted, is removed from the docs. (_#2331_)
- M036 messages no longer leak internal instance ids such as `p5d.inst`. M005 is retired, because N001 already covers duplicate aliases. (_#2346_)
- A bug inside an assertion aborts the run as X001 instead of showing as an assertion ERROR. Recurrence-group domain violations name their step. G002 is retired. (_#2316_)
- N035 is retired. (_#2330_)

### Formatter

- Parentheses a quantity literal needs before `*`, `/`, or `^` are kept, so `(2.0 m) * PI` no longer becomes `2.0 m * PI`. (_#2278_)
- Doc comments stay on the declaration they document, trailing comments stay with their construct, a bare `Complex` formats as `Complex`, and CRLF files keep their line endings. (_#2315_)

### Documentation

- The `--deny-todo` paragraph of the CLI reference explains which unfinished declarations count, including constants. `computation-model.md` explains unfinished constants. (_#2228_, _#2230_)
- The arithmetic section and the complex arithmetic table state the `Int` scaling rule, and examples drop `to_float` calls that are no longer needed. (_#2237_, _#2240_)
- The datetime docs cover the UTC arithmetic rule, the `:60` limitation, `to_unix`/`from_unix` as a UTC label encoding, and plugin datetime exchange. (_#2283_)
- The docs say which Git credentials dependency fetches use: SSH (OpenSSH config, keys, agent; `GIT_SSH` honored) for private repositories. Credential helpers, `insteadOf`, and `GIT_SSH_COMMAND` never apply. (_#2335_)
- `grammar.ebnf` matches the parser for required type annotations, owner-qualified index labels, and comma-separated match arms. (_#2335_)
- "Name Universes", "Trust: Lockfile Pins", and the extern-function trust paragraph are corrected. (_#2278_, _#2304_)

### Dependency, Toolchain, and Workflow Maintenance

- The workspace toolchain is Rust 1.100.0 (from 1.99.1, after a 1.99.0 → 1.99.1 bump in _#2318_). The MSRV stays 1.99. Cargo now refuses registry versions published less than 7 days ago (`global-min-publish-age`). (_#2328_)
- Updated Rust crates: `hifitime` to v4.3.2, `jiff` to v0.2.43, `loom` to v0.7.3, `syn` to v3.0.10, and `toml` to v1.2.1. (_#2318_)
- Added `num-bigint` 0.4 for the exact accumulator, and Renovate holds it below 0.5 until `num-rational` supports 0.5. (_#2295_, _#2327_)
- Replaced the unmaintained `fs2` crate with std file locks. (_#2339_)
- Updated JavaScript tooling: pnpm to v12.17.0, and `vite` and `vite-plus` to v1.5.0. (_#2318_)
- Updated `zensical` to v0.0.74. (_#2318_)
- Updated GitHub Actions: `astral-sh/setup-uv` to v10.3.3, `crate-ci/typos` to v1.52.0, `hustcer/setup-nu` to v3.29, `leanprover/lean-action` to v1.7.0, and `taiki-e/install-action` to v2.90.1. (_#2318_)
- Updated pre-commit hooks: `crate-ci/typos` to v1 (_#2320_, then v1.52.0 in _#2318_) and `rvben/rumdl-pre-commit` to v0.3.2 (_#2319_).
- Rust-heavy CI jobs run on Namespace runners with one cache volume per job, and coverage runs on a 16x32 runner from a fresh volume. cargo-hawk builds under `target/`, so the cache keeps its build. (_#2321_, _#2323_, _#2324_, _#2325_, _#2329_)
- The `qa-corpus` checkout in CI is pinned to a commit SHA. (_#2339_)
- New fuzz targets cover model-protocol request decoding, exact-JSON parameter input, plugin result marshalling, and report-bundle decoding. (_#2339_)
- Release builds include a `deploy-view` job, gated on the `VIEW_DEPLOY` repository variable. (_#2305_)
- CodeRabbit reviews incrementally. (_#2229_)
- Mutation test findings from the daily campaign are tracked in the repository. (_#2326_)

### Internal

- Unit-scale and coordinate constants share the operator typing rules and checked kernels of node bodies (`tir/operator_typing.rs`, `semantic/int_arith.rs`). (_#2238_)
- Module handles and local DAG fact tables carry the identity of their issuer, so a mismatched pairing fails loudly. (_#2330_)
- Dead syntax, typing, TIR, LSP, and playground code is removed, including the jsDelivr CDN mode for Vega scripts, and `SymbolKey` is renamed to `SymbolId`. (_#2330_, _#2334_, _#2346_)
- TIR and evaluation logic is deduplicated: one type-resolution driver, one exact `f64 → i64` conversion, and shared diagnostic rendering. (_#2333_)
- `graphcal-lsp`'s `server.rs` and `symbol_table.rs` and `graphcal-project`'s 11k-line `tests.rs` are split into topic modules. (_#2339_)
- The Lean oracle's external-surface conformance scenarios run through the production pipeline. (_#2281_)
- Bumped the workspace version to v0.0.1-alpha.45 and added these notes to `CHANGELOG.md`. (_#2347_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.44...v0.0.1-alpha.45>

## v0.0.1-alpha.44

This release makes `/` on two `Int` operands a compile error and adds `div_trunc`, `div_floor`, and `div_ceil`, which name how the integer quotient is rounded.
It also updates dependencies and moves CI to a GitHub Actions workflow lockfile and `prek`.

**Some existing Graphcal source programs need to be modified.** A program that divides two `Int` values with `/` no longer compiles. See [Breaking Changes](#breaking-changes).

### Breaking Changes

- `/` is no longer defined for two `Int` operands, and such a division is rejected with D043. Before, `a / b` on `Int` silently truncated toward zero, which is often not the rounding a calculation needs. Every affected program fails to compile, so none changes its result silently. Replace `a / b` with one of the new integer division functions, or with `to_float(a) / to_float(b)` for the real quotient. `div_trunc(a, b)` gives the same result as the old `/`. The D043 help lists these choices. (_#2196_)

  ```text
  graphcal::D043
    × `/` is not defined for two `Int` operands
    help: integer division must name its rounding. `div_trunc(a, b)` rounds toward zero and reproduces the previous result of `a / b`, but it may not be the rounding you need: `div_floor(a, b)` or `div_ceil(a, b)` is often intended. For a real quotient write `to_float(a) / to_float(b)`. ...
  ```

### Integer Division Functions

- The new built-ins `div_trunc`, `div_floor`, and `div_ceil` divide two `Int` values and round the quotient toward zero, toward negative infinity, and toward positive infinity. They differ only when the division is inexact: `div_trunc(-7, 2)` is `-3`, `div_floor(-7, 2)` is `-4`, and `div_ceil(7, 2)` is `4`. (_#2196_)

  ```graphcal
  param items: Int = 10;
  param per_rack: Int = 4;

  // 3: a partly filled rack still needs a rack.
  node racks: Int = div_ceil(@items, @per_rack);
  // 2: only the completely filled racks.
  node full_racks: Int = div_floor(@items, @per_rack);
  ```

- The quotient is computed exactly. A zero divisor is an evaluation error, and so is `-9223372036854775808` divided by `-1`, whose quotient does not fit in `Int`. In a `const node`, both are compile errors. (_#2196_)
- Calls with constant arguments can be used where a compile-time `Int` is required, such as `2 ^ div_trunc(6, 2)` or `@v[div_floor(7, 2)]`. (_#2196_)
- The language server's signature help, hover, and completion include the new functions. (_#2196_)
- Unit scales such as `(1 / 3) m` still divide as real numbers. (_#2196_)

### Documentation

- The operator tables say that `/` is not defined for two `Int` operands, and the built-in reference has a new "Integer Division Functions" section. (_#2196_)
- The `round()` entry states that a tie rounds away from zero, so `round(2.5)` is `3.0` and `round(-2.5)` is `-3.0`. The behavior is unchanged. (_#2196_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated Rust crates: `arrow-array`, `arrow-ipc`, and `arrow-schema` to v60.1.0 (_#2197_), `wasm-bindgen` to v0.2.130 and `js-sys` to v0.3.107 (_#2204_), `unicode-width` to v0.2.3 (_#2194_), and `wat` to v1.263.0 (_#2209_).
- Updated JavaScript tooling: pnpm to v12.16.1 (_#2208_) and `@types/node` to v26.6.6 (_#2211_). CI now uses Node.js v26.13.0. (_#2221_)
- Updated `zensical` to v0.0.73. (_#2207_, _#2214_)
- Updated GitHub Actions: `taiki-e/install-action` to v2.87.31 (_#2200_, _#2210_), `astral-sh/setup-uv` to v10.3.2 (_#2199_, _#2202_), `actions/setup-python` to v7.1.0 (_#2201_), `voidzero-dev/setup-vp` to v1.22.0 (_#2205_), `rust-lang/crates-io-auth-action` to v1.0.6 (_#2198_), and `zizmorcore/zizmor-action` to v0.8.1 (_#2206_, _#2223_).
- Workflows pin their actions in a GitHub Actions workflow lockfile, `.github/workflows/actions.lock`, instead of inline commit hashes. A new CI job checks the lockfile, and Renovate updates it. (_#2223_)
- The Renovate configuration extends the shared preset in `graphcal-lang/renovate-config`. (_#2225_)
- Pre-commit hooks run in CI with `prek` instead of on pre-commit.ci. (_#2226_)
- Refreshed lock files. (_#2213_)

### Internal

- Exact power exponents such as `(3/2)` are lowered to the literal they denote, so they are not type-checked as an `Int` division. (_#2196_)
- One exact integer division kernel serves both evaluation and compile-time `Int` folding. (_#2196_)
- Bumped the workspace version to v0.0.1-alpha.44 and added these notes to `CHANGELOG.md`. (_#2227_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.43...v0.0.1-alpha.44>

## v0.0.1-alpha.43

This release adds two things for applications that serve models in batch: `graphcal model serve` can fix parameters it does not vary to given values, and the new `graphcal model unit` command resolves unit expressions for clients.
When a row fails only because a declaration it depends on failed, the row's diagnostic now names the declaration where the failure started.
The web viewer reads GitHub through the Graphcal browser extension and no longer needs a personal access token.
It contains no language changes.

No existing Graphcal source programs need to be modified.
The batch evaluation protocol's session schema and wire formats are unchanged.

### Batch Evaluation

- `graphcal model serve` accepts `--param`, `--params-json`, and `--params-json-file`, in the same formats as `graphcal eval`, and binds those values in every row. A session can now leave a parameter without a default unvaried, so a model whose parameters are records without defaults can be served without a wrapper module that bakes the values in. The values are type- and domain-checked once when the session starts. A parameter that is both varied and fixed, an unknown name, or a repeated name is an error. `--params-json-file -` is rejected, because stdin carries the request stream. Fixed parameters are not sent in requests and are not part of the session schema. (_#2180_)
- The new `graphcal model unit <FILE> <UNIT>...` command resolves unit expressions in the entry file's scope, as if they were written after `->`, and prints each one's dimension and SI scale as JSON. A client that reads a quantity inside a structured output, such as a field of a record result, gets it in SI units, and can use this command to convert it to a display unit without keeping its own unit table. Runtime units and unknown units are errors (exit code 2). (_#2181_)

  ```json
  [{ "unit": "h", "dimension": { "Time": 1 }, "si_scale": 3600.0 }]
  ```

- When a row fails only because a declaration it depends on failed, its error message now also names each declaration that failed on its own, with that declaration's message. Before, the message named only the immediate dependency, and finding the cause needed a separate `graphcal eval` with the same inputs. The protocol's row failure kinds are unchanged. (_#2179_)

  ```text
  result: dependency failed: inner::out; originating failure: inner::ratio (division by zero)
  ```

- Rust API additions: `SessionSpec::with_fixed` in `graphcal-model-protocol`, `ParameterBindingBuilder::into_fixed`, `FixedParameters`, and `PreparedProject::model_session_with` in `graphcal-project`, and `SessionBuilder::fix_from_json_file` in `graphcal-model-client` (_#2180_). `ResolvedUnit` in `graphcal-model-protocol`, `PreparedProject::resolve_model_unit` in `graphcal-project`, and `Model::resolve_units` in `graphcal-model-client` (_#2181_).

### View (Experimental)

- The web viewer at graphcal.org/view reads GitHub through the **Graphcal for GitHub** Chrome extension and uses the browser's GitHub sign-in, instead of a personal access token. Any repository you can open on github.com, private or public, opens in the viewer with nothing to configure. Without the extension, the viewer shows a notice and an install link. Firefox and Safari are not supported. The ref menu lists branches only, so open a pull request by its head branch. (_#2173_)

### Documentation

- The `@` sigil tables in the computation model page no longer say that `@` is not allowed in `const node` expressions. A `const node` expression may use `@` to reference other `const node` values. (_#2186_)
- The Batch Evaluation guide has a new section on fixing parameters with values, and the CLI reference documents the new `model serve` options and `graphcal model unit`. (_#2180_, _#2181_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated Rust crates `gix` to v0.90.0 (_#2172_) and `open` to v5.4.5 (_#2169_).
- Updated CodeMirror (_#2171_), Vite+ to v1.4.0 (_#2183_), pnpm to v12.15.0 (_#2177_), and Node.js to v24.22.1 (_#2184_).
- Updated `zensical` to v0.0.71. (_#2175_)
- Updated GitHub Actions: `zizmorcore/zizmor-action` to v0.7.1 (_#2170_, _#2174_), `taiki-e/install-action` to v2.87.29 (_#2176_), and `Swatinem/rust-cache` to v2.9.3 (_#2192_).
- Refreshed lock files. (_#2182_)

### Internal

- Added a CodeRabbit configuration for AI code review. (_#2178_)
- Bumped the workspace version to v0.0.1-alpha.43 and added these notes to `CHANGELOG.md`. (_#2193_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.42...v0.0.1-alpha.43>

## v0.0.1-alpha.42

This release shows interactive report inputs in the units their values are written in, instead of canonical SI units.
It also updates `rustls-webpki` and `webpki-root-certs` to fix security advisories. It contains no language changes.

No existing Graphcal source programs need to be modified.

### Interactive Reports

- A quantity input now shows its unit tag, range hint, and slider in the unit of its default or baseline literal. For example, a parameter written as `30.0 deg` shows `[deg]` and a range in degrees instead of `[rad]` and `0 to 1.0471975511965976 rad`. An input without a published value still uses its canonical SI unit. (_#2165_)
- Input fields start with the literal as written, such as `30.0 deg` instead of `29.999999999999996 deg`. Quantity and complex values are printed with the shortest number that converts back to exactly the same SI value. (_#2165_)
- Sliders are placed for values written in the display unit or in the SI unit, and dragging a slider writes the value in the display unit. Before, only literals in the canonical SI unit placed the slider, and dragging rewrote the field in SI units. Slider endpoints convert exactly to the declared bounds. (_#2165_)

### Security

- Updated `rustls-webpki` to v0.103.16 and `webpki-root-certs` to v1.0.10 to fix RUSTSEC-2026-0382 and RUSTSEC-2026-0383, which let certificate verification trust distrusted intermediates. Graphcal uses them when it fetches Git dependencies over HTTPS. (_#2159_)

### Documentation

- The CLI reference no longer says that quantity fields in reports show the SI unit. (_#2165_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated Rust crate `tokio` to v1.54.1, because v1.54.0 was yanked. (_#2166_)
- Updated Node.js to v24.22.0. (_#2158_)

### Internal

- Bumped the workspace version to v0.0.1-alpha.42 and added these notes to `CHANGELOG.md`. (_#2167_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.41...v0.0.1-alpha.42>

## v0.0.1-alpha.41

This release lets a batch evaluation session read every output of a model without naming each one.
It contains no language changes.

No existing Graphcal source programs need to be modified.
The batch evaluation protocol version stays 2, and the request and result wire formats are unchanged.

### Batch Evaluation

- `SessionBuilder::output_all_supported()` in `graphcal-model-client` selects every public output the protocol can carry, in declaration order, without typed output handles. An application that records every result of a session can now read indexed and algebraic outputs whose handle types it cannot choose in advance. Outputs listed as unsupported in the model's interface are skipped, and selecting an output twice is still rejected. (_#2154_)

### Internal

- Bumped the workspace version to v0.0.1-alpha.41 (_#2155_) and added these notes to `CHANGELOG.md` (_#2157_).

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.40...v0.0.1-alpha.41>

## v0.0.1-alpha.40

This release lets applications that store batch evaluations encode and decode output values and read a stored session back without the model.
It contains no language changes.

No existing Graphcal source programs need to be modified.
The batch evaluation protocol version stays 2, and the request and result wire formats are unchanged.

### Batch Evaluation

- Output values can be encoded and decoded on their own, without the protocol's row outcomes, so an application can store them with its own outcome vocabulary. `OutputValues` holds one nullable column per output, absent exactly in rows without values, and `OutputValues::to_struct_array` and `from_struct_array` convert it to and from an Arrow struct array. `RequestBatch` gains the same pair, and `input_fields` and `output_fields` are now public. (_#2146_)
- A session record, a stored stream of request rows joined with their output values, can be read back without the model. `record_inputs_field`, `record_outputs_field`, and `record_metadata` build its schema: an `inputs` struct and an `outputs` struct beside any application fields. `RecordSchema::decode` restores the recorded `SessionSchema` and checks its port names, domains, unit scales, and protocol version. A restored session keeps the record's port order and carries the recorded fingerprint. (_#2147_)
- Rust API changes in `graphcal-model-protocol`: `ResultBatchError` is replaced by `OutputValuesError`, `ResultBatch::columns()` and `column()` are replaced by `ResultBatch::values()`, and `ResultDecodeError` reports output decoding errors through `OutputDecodeError` and a status that disagrees with the values as `MissingValues` or `UnexpectedValues`. (_#2146_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated Rust crate `tokio` to v1.54.0. (_#2152_)
- Updated Vite+ to v1.3.0. (_#2145_)
- Updated `zensical` to v0.0.70. (_#2149_)
- Updated GitHub Actions: `taiki-e/install-action` to v2.87.28 and `leanprover/lean-action` to v1.6.2. (_#2144_, _#2150_)
- Refreshed lock files. (_#2143_)

### Internal

- Added the v0.0.1-alpha.40 notes to `CHANGELOG.md` and bumped the workspace version to v0.0.1-alpha.40. (_#2151_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.39...v0.0.1-alpha.40>

## v0.0.1-alpha.39

This release replaces whole-node `todo { … }` with typed `todo<T> { … }` holes, which can stand in for any part of a node formula, and rejects recursive algebraic types.
It also extends batch evaluation: sessions can carry quantities in chosen units, indexed, datetime, complex, and algebraic values, and keys over every index kind, and a wrapper file's re-exported include outputs become output ports.

**Some existing Graphcal source programs need to be modified.** Unfinished nodes must name the type of their hole, and recursive types no longer compile. See [Breaking Changes](#breaking-changes).
The batch evaluation protocol version is now 2, so clients built for alpha.38 must be updated. See [Batch Evaluation](#batch-evaluation).

### Breaking Changes

- `todo { … }` is replaced by the typed hole expression `todo<T> { … }`. Rewrite `node x: T = todo { … };` as `node x: T = todo<T> { … };`. (_#2138_)
- A nominal type can no longer refer to itself, directly, through other types, through generic arguments such as `Box<List>`, or through generic defaults. Such programs are rejected with S034, which gives the cycle path. Graphcal has no recursion, so recursive values could only be built and inspected to a depth written in the source. This reverses #1257, which made recursive values evaluate. (_#2137_)

  ```text
  graphcal::S034
    × type `Expr` refers to itself through `Add.lhs` -> `Term` -> `Wrap.expr`
  ```

### Typed `todo` Holes

- A hole `todo<T> { … }` can replace a whole formula or any part of one, such as `@mass * todo<Acceleration> { @g0 }`. Its type is written explicitly, and the rest of the formula is still type-checked. (_#2138_)

  ```graphcal
  node v_exhaust: Velocity = todo<Velocity> { @isp, @g0 };
  ```

- The braces list the possible direct dependencies of the missing part: a reference `@y`, an element `@y[t, …]` keyed by local variables in scope, or a local variable such as `s`, which documents intent without being a graph dependency. Element entries let an unfinished recurrence state which step it reads. (_#2138_)
- Holes are allowed only in `node` formulas (G010), and the hole type may not carry a domain constraint (C009). A node whose formula contains a hole is TODO, and its dependents are BLOCKED. (_#2138_)
- A recurrence group with an unfinished member is accepted when its reads satisfy the usual rules. A member that is entirely a hole is rejected, and a hole inside a `for` or `unfold` member is checked through its listed entries. The group is not evaluated: its unfinished members are TODO, and its finished members are BLOCKED. The Lean specification under `formal/Graphcal/Graph/RecurrenceGroup/` is updated accordingly. (_#2138_)
- The formatter and language server support holes. The language server reports one diagnostic per hole and finds references inside hole entries. (_#2138_)

### Batch Evaluation

- `pub` items of a selective include (`include m(...)::{ pub x, pub y as z }`) that are runtime nodes are now output ports, under their local names and in source order. A wrapper file that fixes parameters no longer has to re-declare every output. Private brace items, re-exported constants, and members of whole-instance includes (`include ... as alias`) are not ports. (_#2131_)
- `graphcal model serve --unit NAME=UNIT` carries a quantity port in a chosen unit instead of its SI unit. The option can be repeated. `UNIT` is a unit expression resolved in the served file, as if written after `->` there, and must have a fixed scale. A runtime unit, an unknown unit, or a syntax error stops the server at startup with exit code 2. The server publishes each chosen unit's SI scale in the stream metadata, and domains are converted to the chosen unit. In Rust, use `SessionBuilder::vary_in` and `output_in`. (_#2132_)
- Ports of the following types are now supported instead of listed as unsupported. `--unit` applies to complex values and indexed values of quantities as well as quantities, and a domain bounds every element of an indexed parameter. (_#2134_)

  | Graphcal type | Wire `kind` | Arrow |
  | --- | --- | --- |
  | `T[I]` (any depth) | `indexed` | `FixedSizeList` per axis |
  | `Datetime<S>` | `datetime` | `Float64` seconds since J2000, read in `S` |
  | `Complex<D>` | `complex` | `Struct{re, im}` in the port's unit |
  | Record type | `algebraic` | `Struct` of its fields |
  | Union type | `algebraic` | `Struct{constructor, variants}`, with only the active constructor non-null |

- `Key<I>` is now supported for every index kind. A key over a named index travels as the variant name, as before. A key over `Fin(N)` travels as its position from 0, and a key over a coordinate index as its SI coordinate, which must equal one of the index's coordinates exactly. In Rust, use `InputHandle::position` and `InputHandle::coordinate`. (_#2136_)
- A port whose dimension has no base unit stays unsupported, and the reason now names the base dimension that needs a `base unit`. (_#2136_)
- The protocol version is now 2. The describe document and the Arrow encoding changed with the unit, value, and key support above. (_#2136_)
- Rust API changes in `graphcal-model-protocol` and `graphcal-model-client`: `ModelInterface::configure` takes a unit resolver (`CanonicalUnitsOnly` for SI-only sessions), `KeyType` carries the `Axis` it selects from, `Value::Key` carries a `KeyValue`, `VariantPosition` is renamed `KeyPosition`, and on the client `KeyVariant` is renamed `KeyEntry` and `variants()` is renamed `entries()`. (_#2132_, _#2134_, _#2136_)

### Bug Fixes

- `type Holder<T: Type = Holder> { ... }` no longer crashes `graphcal check` with a stack overflow. It now reports S034. (_#2137_)

### Documentation

- The Batch Evaluation page and CLI reference describe `--unit`, the supported types and their wire layout, and the key forms. The wrapper example uses the `::{ pub failure, pub margin }` form instead of re-declaring outputs. (_#2131_, _#2132_, _#2134_, _#2136_)
- The "Unfinished nodes" section describes typed holes, and the "Recursive Algebraic Types" section is replaced by a short statement of the rule. (_#2137_, _#2138_)

### Editor Support

- The tree-sitter grammar, Zed extension, and VS Code extension support `todo<T> { … }` (graphcal-lang/tree-sitter-graphcal#99, graphcal-lang/zed-graphcal#42, graphcal-lang/vscode-graphcal#154). (_#2138_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated JavaScript tooling: pnpm to v12.14.0, `@playwright/test` to v1.64.1, and `@axe-core/playwright` to v4.14.0. (_#2140_, _#2129_, _#2139_)
- Updated the `voidzero-dev/setup-vp` action to v1.21.2. (_#2130_)

### Internal

- Recursive-type handling is removed: S030, the recursion guard in concrete obligations, and the cycle detection in the model schema graph. The new check runs when a module is added to the project type store, so every later phase sees an acyclic type graph. (_#2137_)
- A concrete (executable) body cannot contain a hole by construction, so the evaluator has no case for holes. (_#2138_)
- Added the v0.0.1-alpha.39 notes to `CHANGELOG.md` and bumped the workspace version to v0.0.1-alpha.39. (_#2141_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.38...v0.0.1-alpha.39>

## v0.0.1-alpha.38

This release replaces the Tenax-specific model server with a general batch evaluation protocol, so sampling studies, scenario discovery, and optimizers can evaluate a Graphcal model many times from another process.
The new `graphcal model describe` command prints a model's interface, `graphcal model serve` evaluates request batches against it, and the new `graphcal-model-client` crate drives both from Rust.

No existing Graphcal source programs need to be modified.
`graphcal model serve` has new required options. See [Batch Evaluation](#batch-evaluation).

### Batch Evaluation

- The new `graphcal model describe <FILE> [--root <ROOT>]` command prints the model's interface as JSON: every entry parameter and public node with its type, unit, domain, and whether it has a default, together with a protocol version and an interface fingerprint. (_#2121_, _#2124_)
  - Quantities, `Int`, `Bool`, and `Key<I>` over a named index are supported. A port of any other type is listed as unsupported with the reason, instead of being left out.
  - Every quantity travels in its canonical SI unit, and domains are given in the same unit.
  - The fingerprint changes when a port is added, removed, or retyped, but not when only formulas change.
- `graphcal model serve` now evaluates request batches against that interface. (_#2122_, _#2124_)

  ```bash
  graphcal model serve reliability.gcl \
      --input load --input mode --output failure --output margin \
      --expect-fingerprint sha256:…
  ```

  - `--input` names the parameters a session varies. Every other parameter keeps its default, so a parameter without a default must be varied.
  - `--output` may name any public node of a supported type. Before, only scalar `Bool` nodes could be selected.
  - Exactly one of `--expect-fingerprint <FINGERPRINT>` and `--skip-fingerprint-check` is required. With `--expect-fingerprint`, the server refuses to start if the model's interface changed since it was described.
  - Parameters no longer need both `min` and `max`. Before, every quantity and `Int` input needed finite bounds on both sides, and a `Bool` input was rejected.
- Each request row produces one outcome, in request order: success, assertion failed, input out of domain, or evaluation error. Only successful rows carry outputs. A failed `assert` is reported separately, so an optimizer can treat the row as infeasible. Malformed streams, a request schema that differs from the session, NaN or infinite values, and unknown key variants still stop the server with exit code 1. (_#2121_, _#2122_)
- The new `graphcal-model-client` crate runs `graphcal model describe` and `graphcal model serve` for Rust programs. Its handles are typed by value kind (`Quantity`, `Int`, `Bool`, `Key`), each session is pinned to the described fingerprint, and Arrow IPC stays internal. (_#2125_)
- The new `graphcal-model-protocol` crate defines the interface, session, batch, and outcome types without I/O or a compiler dependency, so clients and non-Graphcal models can share them. Its `arrow` feature maps them to Arrow schemas. (_#2121_)
- The `graphcal-tenax` crate is removed. [Tenax](https://github.com/shunichironomura/tenax) now uses `graphcal-model-client`. (_#2124_, _#2125_)

### Documentation

- The Tenax Integration page is replaced by a Batch Evaluation page. It shows how to describe a model, start a session, evaluate from Rust, fix parameters with a wrapper file, read row outcomes, and write a client in another language. The CLI reference documents `model describe` and the new `model serve` options. (_#2126_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated Vite+ to v1.2.0. (_#2127_)

### Internal

- The `graphcal model serve` transport is a separate crate, `graphcal-model-server`, around the pure `evaluate_request` of a prepared project. (_#2122_, _#2124_)
- End-to-end tests run `graphcal-model-client` against the built CLI. (_#2125_)
- Added the v0.0.1-alpha.38 notes to `CHANGELOG.md` and bumped the workspace version to v0.0.1-alpha.38. (_#2128_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.37...v0.0.1-alpha.38>

## v0.0.1-alpha.37

This release lets `for` and `unfold` declarations read each other when every cycle between them steps back along one axis, so coupled quantities such as a state and the action that drives it can be written as separate declarations.
It also releases version 1 of the experimental view model, lets Graphcal View show include instances with their own values, and makes the language server show values inside inline `dag` bodies.

No existing Graphcal source programs need to be modified.

### Recurrences Across Declarations

- A dependency cycle between `for` and `unfold` declarations is now accepted when it is acyclic element by element along one coordinate axis. Each step is evaluated in turn. (_#2101_)

  ```graphcal
  index Step = range(0.0 s, 3.0 s, step: 1.0 s);
  param k: Dimensionless = 0.5;

  node action: Dimensionless[Step] = for t: Step { -@k * @state[t] };
  node state: Dimensionless[Step] = unfold(Step, 1.0, |s, prev_t, t| s + @action[prev_t]);
  ```

  Here the order is `state[0]`, `action[0]`, `state[1]`, `action[1]`, and so on.
- Such a cycle is accepted when all of the following hold. Reads of declarations outside the cycle are unrestricted. (_#2101_)
  - Every declaration in it is a `for` or `unfold` over the same coordinate axis in the same DAG, and at least one is an `unfold`. A cycle across an `include` boundary is still rejected.
  - Every read between them uses the reader's own `for` variable or `unfold` binders as the first key: `@y[t, ...]` reads the same step and `@y[prev_t, ...]` the previous one. Whole-value reads such as `sum(@y)` and reads in an `unfold` initial value are rejected.
  - No declaration reads itself; an `unfold` reads its own previous value through its state binding.
  - Same-step reads alone form no cycle.
- A rejected cycle reports G001 with the reason, for example ``same-step cycle `action` -> `state` -> `action` ``, and a hint to read `prev_t`. A cycle without an `unfold` keeps the previous G001 message. (_#2101_)
- Whether a cycle is accepted does not depend on the order of the declarations. The rules are specified and proved in Lean under `formal/Graphcal/Graph/RecurrenceGroup/`, with a prose specification in `internals/recurrence-groups.md`. (_#2100_)

### Language Server

- Inline `dag` bodies are now treated like a standalone file with the same declarations. If every param has a default and no index is required, inlay hints show the computed values and runtime failures are reported on their declarations. If some input is required, inlay hints show types only, as in a library file. Previously an inline DAG body showed nothing. Hover inside a DAG body shows the checked type instead of `(unknown type)`. (_#2117_)
- Inlay hints inside an inline `dag` body no longer show the values of an include instance. For example, `param parking_alt: Length;` inside a DAG showed `= 200 [km]` because an `include` bound that value; values now attach to declarations by identity, not by display name. (_#2104_)
- Runtime failures inside an include instance are reported where they belong. Before, a failure landed on the included DAG's declaration only when the instance name happened to match. Otherwise, as with `include ... as i`, it was reported at the start of the file. It is now reported on the included DAG's declaration, with the include site as related information. When that DAG is written in another file, it is reported on the include site. A failure of a value the include exposes is reported on the include declaration. (_#2120_)
- In editors without inlay-hint refresh support, such as Helix, all inlay hints in a file could disappear after an edit until the view scrolled. The server now waits up to 2 s for the analysis of the latest edit before answering an inlay-hint request. Editors with refresh support behave as before. (_#2118_)

### Graphcal View (Experimental)

- The view model is released as `version` 1. Its format may still change in later releases, but the version number now changes with it. (_#2106_)
  - Node and index ids are opaque keys; the source spelling is in `label`. `kind` is separate from the new `origin` (`local`, `imported`, `included`, `called`, `includer`).
  - Cells over a coordinate index show their values instead of `—`, and `Fin` axes are listed.
  - Every written table cell is reported with its keys and source range, and nodes carry their element type.
  - `dimension` is the declaration's spelling, and values are displayed by the same rule as `graphcal eval`. A display unit that cannot be applied affects only that node, and `graphcal view` then exits 1 like `graphcal eval`.
- An include with bindings can be shown with its own values: `graphcal view --instance <id>`, the optional `target` of the language server's `graphcal/viewModel` request, and the page's pinned panes all select an include instance. A param that the include binds is shown and edited at its argument. (_#2111_)
- A node that reads `@dag(...)::out` now depends on the inline DAG call, both in the view and in `graphcal graph`, which draws the call as a cluster with an edge to the caller. (_#2110_)
- Editing on [graphcal.org/view](https://graphcal.org/view/) is more reliable. (_#2109_)
  - A cell is replaced at the range the compiler reports, or is read-only. An `Int` cell writes an integer literal such as `5`.
  - Edits are placed on the latest source, and typing in the code pane is never replaced by the host's copy.
  - Libraries with required params can be edited as long as the sources still check.
  - Fixed several sheet bugs, including stale "evaluating…" indicators, selection clearing, and engine load retries.
- Fixes for files opened from GitHub. (_#2107_)
  - Paths with special characters are encoded correctly, and malformed percent-encoding no longer hangs the page.
  - Branches, tags, and pull requests load independently, so a token without pull-request access still lists branches and tags.
  - The page checks the engine's limits before downloading sources.
  - In compare mode, both refs are resolved to commits first, and a base that cannot be read is reported instead of marking every node as new.

### Dependency, Toolchain, and Workflow Maintenance

- Updated JavaScript tooling: pnpm to v12.13.0, `zod` to v4.6.7, and `@types/node` to v26.6.5. (_#2094_, _#2105_, _#2091_, _#2098_, _#2093_)
- Updated Rust crate `libc` to v0.2.191. (_#2112_)
- Updated `zensical` to v0.0.69. (_#2097_)
- Updated GitHub Actions: `taiki-e/install-action` to v2.87.27, `astral-sh/setup-uv` to v10.3.0, and uv in CI to 0.13.x. (_#2113_, _#2114_, _#2116_)
- The repository's VS Code settings point `graphcal.lsp.path` at the workspace's debug build. (_#2115_)

### Internal

- The evaluator runs an accepted recurrence group as one schedule unit, step by step, and checks domains per element. Reading an element before it is computed is an internal error. (_#2101_)
- Model contract tests check `graphcal view` output for fixtures against the page's zod schemas. The view page's `main.ts` is split into modules, and their pure parts are tested. (_#2106_, _#2109_)
- The evaluation of the root now takes the entered callable instead of always using the plan root, so the language server can evaluate inline DAGs on their defaults. (_#2117_)
- Added the v0.0.1-alpha.37 notes to `CHANGELOG.md` and bumped the workspace version to v0.0.1-alpha.37. (_#2119_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.36...v0.0.1-alpha.37>

## v0.0.1-alpha.36

This release adds Graphcal View, an experimental per-file sheet that opens Graphcal files from GitHub at <https://graphcal.org/view/>, and the `graphcal view` command that exports its data.
It also lets `cargo binstall` install the prebuilt binaries, raises the minimum supported Rust version for source builds to 1.99, makes diagnostic codes unique, and updates `rustls` to fix security advisories.

No existing Graphcal source programs need to be modified.
Plugin errors and one module-resolution error have new diagnostic codes. See [Changed Diagnostic Codes](#changed-diagnostic-codes).

### Graphcal View (Experimental)

- [graphcal.org/view](https://graphcal.org/view/) shows a Graphcal file as a sheet. Columns follow the dependencies between declarations, and indexed values are laid out as tables. (_#2057_)
  - Open a file on GitHub with an address of the form `https://graphcal.org/view/#owner/repo@ref/path/to/file.gcl`. The page has no server: the browser reads the repository through the GitHub REST API and evaluates the project with the WebAssembly engine.
  - Edit a cell, the formula bar, or the code pane, and the project is re-evaluated. If an edit fails, it is rolled back and the reason is shown. Nothing is saved. **Copy changes** puts the edits on the clipboard as a unified diff that `git apply` accepts.
  - The code pane is shown beside the sheet, with values at the end of each declaring line. A toggle switches between code only, both, and sheet only.
  - An included or imported file can be pinned beside the sheet or opened through the breadcrumb. The explorer lists the repository's files and switches between branches, tags, and open pull requests.
  - `@base...head` compares two revisions: the code pane shows a diff, and the sheet frames changed nodes and shows the old values.
  - Public repositories need no token, but GitHub allows only 60 requests per hour without one. For private repositories or a higher limit, paste a fine-grained personal access token with read-only Contents access. The token is stored in the browser's local storage and sent only to `api.github.com`.
- The new `graphcal view <file.gcl> [--root <dir>]` command prints the file's view model as JSON. The language server provides the same model through the custom request `graphcal/viewModel` and, to clients that declare `experimental.graphcal.viewModelChanges`, the notification `graphcal/viewModelDidChange`. The view model format may change in later releases. (_#2057_, _#2059_)
- Other editors and the playground do not pay for the view: the language server and the browser engine build the view model only when it is requested. (_#2059_)

### Installation

- `cargo binstall graphcal@0.0.1-alpha.36` installs the prebuilt binary from the GitHub Release. It never falls back to third-party cargo-quickinstall binaries, and builds from source for targets without a prebuilt archive. (_#2027_)
- The installation guide is reorganized by task: the installer scripts come first, then `cargo binstall` and `cargo install`, then building from source. A new section shows how to install Graphcal in GitHub Actions with [`graphcal-lang/setup-graphcal@v1`](https://github.com/graphcal-lang/setup-graphcal). The README Quickstart uses the installer script. (_#2028_)
- Building Graphcal from source, depending on its Rust crates, and building plugins now require **Rust 1.99** or later (previously 1.95). Users of the prebuilt binaries are unaffected. (_#2033_)

### Changed Diagnostic Codes

Some diagnostic codes were assigned to two different errors. Each code now names exactly one error. If your scripts or CI match on these codes, update them.

- Plugin errors use W codes instead of P codes, keeping their numbers; for example, P001 is now W001. P codes now belong only to parse errors. (_#2032_)
- A module lookup failure that no more specific diagnostic covers now reports M036 instead of M033. M033 remains the code for a file-root `import` of its own file. (_#2032_)
- A test now checks that no two errors in the workspace share a code. (_#2032_)

### Security

- Updated `rustls` to v0.23.46 to fix RUSTSEC-2026-0346, RUSTSEC-2026-0347, RUSTSEC-2026-0348, and RUSTSEC-2026-0349. Graphcal uses `rustls` when it fetches Git dependencies over HTTPS. (_#2095_)

### Documentation

- The plugin documentation cites the new W codes. The plugin authoring guide named the wrong code for an extern signature that binds a bare name; it now says W001 or P016. (_#2032_)
- The installation guides state the Rust 1.99 requirement. (_#2033_)

### Dependency, Toolchain, and Workflow Maintenance

- GitHub Release titles start with `v` again, such as `v0.0.1-alpha.36`. dist takes the title from the `CHANGELOG.md` heading, so the headings are now `## v<version>`, and the release check rejects a heading without `v`. (_#2026_)
- The report engine build inside a source build ignores inherited dynamic-library search paths, so it cannot pick up another Rust installation's sysroot. The Miri and AddressSanitizer jobs use a nightly compatible with Rust 1.99. (_#2033_)
- Updated Rust crates: `proptest` to v1.12.0, `serde_json` to v1.0.152, `sha2` to v0.11.1, `syn` to v3.0.9, `tokio` to v1.53.3, and `wat` to v1.262.0. (_#2041_, _#2035_, _#2052_, _#2030_, _#2045_, _#2055_, _#2056_)
- Updated JavaScript tooling: pnpm to v12.11.2, Vite+ to v1.1.0, CodeMirror (`@codemirror/language` v6.13.1, `@codemirror/view` v6.43.14), `@playwright/test` to v1.64.0, and `@types/node` to v26.6.5. (_#2029_, _#2047_, _#2048_, _#2051_, _#2031_, _#2034_, _#2040_, _#2050_)
- Updated GitHub Actions: `taiki-e/install-action` to v2.87.26, `crate-ci/typos` to v1.51.1, `actions/download-artifact` to v8.0.2, and `actions/upload-artifact` to v7.0.2. (_#2036_, _#2043_, _#2049_, _#2053_, _#2054_, _#2087_, _#2089_)
- Refreshed lock files. (_#2046_)

### Internal

- `just lint`, `just test`, and `just docs-build` build and test the view's web app under `web/view/`, and the documentation site publishes it at `/view/`. The design notes are in `internals/view.md`. (_#2057_)
- Removed unused public accessors from `graphcal-eval`: `RuntimeEvaluation::result`, `Value::dimension`, and `HostArray::into_parts`. (_#2038_)
- Added a test for which values `@imported::name` and `@included::name` may read, with and without overlay bindings. (_#2042_)
- Added the v0.0.1-alpha.36 notes to `CHANGELOG.md` and bumped the workspace version to v0.0.1-alpha.36. (_#2096_)

### New Contributors

- @ogoogo made their first contribution in <https://github.com/graphcal-lang/graphcal/pull/2057>

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.35...v0.0.1-alpha.36>

## v0.0.1-alpha.35

This release is the first to ship prebuilt `graphcal` binaries, so you no longer need a Rust toolchain to install Graphcal.
It contains no language changes.

### Prebuilt Binaries

- Each GitHub Release now includes prebuilt `graphcal` archives for five targets, shell and PowerShell installers, a `sha256.sum` checksum file, and GitHub build attestations. (_#1982_)
  - Linux x86_64 and arm64 (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`), statically linked
  - macOS x86_64 and arm64 (`x86_64-apple-darwin`, `aarch64-apple-darwin`)
  - Windows x86_64 (`x86_64-pc-windows-msvc`)
- Alpha releases are marked as the latest release, so the installer URLs under `releases/latest/download/` always point to the newest version. For example, on Linux or macOS:

  ```sh
  curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graphcal-lang/graphcal/releases/latest/download/graphcal-installer.sh | sh
  ```

  (_#1982_)

- The Linux binaries use the mimalloc allocator. With musl's default allocator, heavy workloads ran 20–25% slower than a glibc build; with mimalloc the musl build matches or beats it. Other targets are unchanged. (_#1982_)

### Dependency, Toolchain, and Workflow Maintenance

- The release workflow builds the binaries with [dist](https://axodotdev.github.io/cargo-dist/) v0.33.0, publishes the crates to crates.io, and creates the GitHub Release only after publishing succeeds. A dry-run mode builds every artifact without publishing, and pull requests run only `dist plan`. `RELEASING.md` documents the procedure. (_#1982_)
- Release notes now come from the hand-written `CHANGELOG.md` section for the version. The release fails early when the section is missing. (_#1982_)

### Internal

- Added the v0.0.1-alpha.34 notes to `CHANGELOG.md` and bumped the workspace version to v0.0.1-alpha.35. (_#2025_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.34...v0.0.1-alpha.35>

## v0.0.1-alpha.34

This release redesigns the interactive report UI and fixes several defects in the Vega-Lite specs generated for plots.
It also fixes bugs around included DAGs and multi-declarations in reports, evaluation output, DAG calls, and the formatter, and it validates constructor field constraints on temporary values during constant evaluation.
The English documentation now lives under `/docs/en/`.

**Some existing Graphcal source programs may need to be modified.** The breaking change rejects programs that were previously accepted only because of a bug. See [Breaking Changes](#breaking-changes).

### Breaking Changes

- A constrained constructor field is now validated even when the constructed value is temporary in constant evaluation, in declaration bounds, or in field bounds.
  Before, a value discarded by projection or pattern matching was never checked, so `const node BAD: Mass = Spec(mass: 5000.0 kg).mass;` compiled even though `Spec.mass` is declared as `Mass(max: 2000.0 kg)`.
  Such programs are now rejected with C001, which names the field and points at the offending argument.
  (_#2014_)

### Interactive Report Redesign

- A header block in interactive reports summarizes checks, lists overridden inputs with a Reset action, and shows the engine status. It replaces the corner chip and the top banner. Clicking the check summary opens the Checks tab. (_#1989_)
- Result tabs show counts. Values, plots, and checks can be pinned to a resizable pinned area. (_#1989_)
- Values changed by the last run are marked, and stale results are dimmed while edits are pending or rejected. (_#1989_)
- Each input row shows its domain hint, a slider when the input is bounded, its description, and override and draft markers. Quantity fields show the canonical SI unit tag, such as `[kg]`. An Apply button appears when Auto run is off. (_#1989_)
- Sliders appear only for literals written in their canonical SI unit. Before, `2.0 km` on a 0–5000 m slider sat at 0, and touching the slider wrote `0.0 m`. (_#1989_)
- Reports have a dark mode, all text meets WCAG AA contrast, and charts follow the report theme. (_#1989_)
- The Advanced controls and the raw-literal entry are removed. The per-input `⋯` menu remains. (_#1989_)
- The same UI is used by `graphcal report` pages and the playground's Report tab. The playground loads the report scripts on first render. (_#1989_)

### Bug Fixes

- Plots:
  - Sample points are visible again. Boolean mark properties such as `filled: true` were emitted as strings, which Vega-Lite drew as transparent points. (_#1989_)
  - Index-label axes keep declaration order instead of being sorted alphabetically. (_#1989_)
  - Quantity axes without a `->` display unit show the canonical SI unit of the plotted values, for example "Time (s)" instead of "Time". Placeholder `x` and `y` titles are removed. (_#1989_)
  - Nominal x-axis labels stay horizontal when there is enough room. (_#1989_)
- Reports list only externally bindable entry parameters under Inputs and Baseline. Parameters of included DAGs that the caller binds now appear under Values. (_#1997_)
- Plots from included DAGs have their documentation captions again in HTML and Markdown reports, including aliased projections and nested file includes. (_#1998_)
- A `///` documentation comment on a slot of a multi-declaration now attaches only to that slot. Before, undocumented slots inherited the first slot's comment in hovers and report captions. (_#2005_)
- Categorized `graphcal eval` JSON output lists only externally bindable entry parameters under `param`. Included DAG input ports are listed under `node`. (_#2011_)
- A DAG call expression such as `@facade(x: 1.0 m)::y` can project a public alias that re-exports an include output (`include ...::{ pub y }`), as a selective include already could. Before, it was rejected with G005. (_#2000_)
- `graphcal format` no longer drops the leading `pub` of a multi-declaration slot that has a comment, and it keeps per-slot documentation directly above its slot while aligning headers and table cells. (_#1999_, _#2012_)

### Documentation

- The English documentation moved from `/docs/` to `/docs/en/`, alongside the Japanese edition at `/docs/ja/`. The site root and `/docs/` redirect to `/docs/en/`, but old English deep links no longer work. The language selector keeps you on the same page. (_#2016_)
- The Index Keys and `argmax` examples use `node` instead of `param`, so they compile with a private index. (_#2013_)
- The report guide in the tutorial and the CLI reference describe the redesigned report UI. (_#1989_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated the pinned Rust toolchain to v1.99.0. The minimum supported Rust version stays at 1.95. (_#2017_)
- `rust-toolchain.toml` declares the `wasm32-unknown-unknown` target, so rustup installs it automatically for source builds. (_#2020_)
- Updated Rust crates: `jiff` to v0.2.38, `jiff-tzdb` to v0.1.9 (bundled IANA time zone database 2026e), `gix` to v0.89.0, and `toml` to v1.1.8. (_#2008_, _#2007_, _#2019_, _#2018_, _#2021_)
- Updated Zensical to v0.0.67, pnpm to v12.10.0, and `@types/node` to v26.6.4. The documentation workflows pin Zensical v0.0.68. (_#1986_, _#2015_, _#2016_, _#1987_, _#2001_, _#2022_, _#1984_)
- Updated `taiki-e/install-action` to v2.87.22, `leanprover/lean-action` to v1.6.1, and the pre-commit hooks for rumdl, typos, and zizmor. (_#2006_, _#2023_, _#1988_)
- Refreshed lock files. (_#1985_)
- The report engine toolchain action supports Linux arm64, macOS arm64, and macOS x86_64 runners, as preparation for prebuilt binaries. Workflows use GitHub's self-repository `uses: $/...` syntax for local actions. (_#1981_)

### Internal

- Simplified the bilingual documentation build: removed the custom theme overrides, the localization checkers, and the translation manifest, and left language switching to Zensical. (_#2016_)
- Bumped the workspace version to v0.0.1-alpha.34. (_#2024_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.33...v0.0.1-alpha.34>

## v0.0.1-alpha.33

This release finishes the internal refactor of the compiler and evaluator started in v0.0.1-alpha.32.
It also accepts `Dimensionless` as the identity term of dimension expressions, fixes several soundness and runtime bugs in rebound and included DAGs, and gives many diagnostics their own error codes and more accurate locations.

**Some existing Graphcal source programs may need to be modified.** The breaking changes reject programs that were previously accepted only because of a bug or a missing check. See [Breaking Changes](#breaking-changes).

### Breaking Changes

- An import inside a `dag` body whose module path does not resolve is now rejected with the same error as a file-root import (M017, M013, M002, or M008).
  Before, an unused one compiled silently and a used one was reported as N002.
  (_#1934_)
- A non-`const` `unit` declared over a defaulted bindable dimension, and a `(min: …, max: …)` bound on a value of such a dimension, are now rejected with V007, as node bodies already were.
  Before, a rebound instance kept the default dimension for such a unit, so `1.0 inst::qu` was typed as `Length` after `dim Q: Mass`, and its bounds compared values across dimensions, such as 4 kg against 10 m.
  (_#1937_)

### Newly Accepted Programs

- `Dimensionless` is now accepted as the identity term of dimension expressions.
  `Dimensionless / Time` denotes the same dimension as `Time^-1`, and `dim Ratio = Dimensionless;` is valid.
  This works in type annotations, `dim` and `unit` declarations, generic arguments, record fields, indexed types, and extern signatures.
  (_#1967_)

### Bug Fixes

- Using an instance's dynamic unit from the including DAG no longer fails at runtime with "undefined graph reference". (_#1937_)
- `graphcal eval` no longer crashes with internal error X001 when an included DAG has an `#[assumes]` whose assumer or assertion is not exposed to the root. (_#1939_)
- Errors raised while resolving modules are now rendered against the file that contains them. Before, a bad import or duplicate in a non-root file had its labels drawn over the root file's text. (_#1978_)
- V001 for a private item in a selective import now points at the item instead of the whole file. (_#1978_)

### Clearer Diagnostics

- Many compile-time diagnostics that were reported under the catch-all code E001 now carry the code of their family: I011–I043 (indexes), S011–S033 (structure), N019–N036 (names), M033 and M034 (module resolution), G008 and G009 (DAG recursion), A024 (attributes), and D037–D042 (time scales, unit scales, and constant expressions). Messages and labels are unchanged. (_#1933_)
- New codes replace E001 in other places: C008 for a domain bound integer too large for exact quantity comparison, M035 for a module path naming an unlocked dependency, and O005–O012 for external parameter binding errors. (_#1933_, _#1934_)
- Binding a map literal to a non-indexed value now reports O007 instead of I003. (_#1934_)
- Diagnostics for `--param` values and editor fields point into the value text (`<--param NAME>`) instead of the entry file. (_#1934_)
- Module-resolution diagnostics use Graphcal category words instead of Rust type names. An unknown applied generic type is now N019 instead of M033 "unknown StructTypeName". A module path naming an undeclared inline DAG is now G008 at the path instead of M033 spanning the root file. (_#1935_, _#1939_)
- An overflowing closed part after a name in an include binding now reports I038 instead of I001. (_#1937_)
- Wording fixes: V003 says "private index" instead of "private cat/range", and D001 says "has type Int" for a type operand instead of "has dimension Int". (_#1978_)
- A024 (arguments on `#[hidden]`) has a precise label and help text, and attribute errors are reported in attribute order. (_#1964_)
- Unavailability reasons name values by their source names:
  - An assertion exposed through an include reports `dependency failed: other::bad` instead of `un1.<include@154>.bad`. (_#1939_)
  - Declarations of invoked modules are named by the module path importers write, for example `pipeline.lib.d::pending` instead of `src.pipeline.lib.d.pending`. (_#1966_)

### Documentation

- The documentation site uses the Graphcal icon as its favicon. (_#1976_)
- The minimum supported Rust version for building from source is now Rust 1.95. The installation guides are updated. (_#1958_)

### Dependency, Toolchain, and Workflow Maintenance

- Updated Rust crates: `insta` to v1.49.0, `libc` to v0.2.190, `tokio` to v1.53.2, and `wat` to v1.261.0. (_#1960_, _#1957_, _#1977_, _#1955_)
- Updated Zensical to v0.0.65 and `crate-ci/typos` to v1.50.3. (_#1927_, _#1956_)
- Fixed Cargo warnings from the deprecated `unsafe-code` lint key and the deprecated atomic `fetch_update` method. (_#1959_, _#1958_)

### Internal

- Completed the handle-redesign follow-up and the final sweep of the invariant/complexity refactor:
  - DAG check facts, declaration bodies, instances, constant pools, loaded modules, and plan DAGs are addressed by typed positions and handles instead of fallible lookups. (_#1928_, _#1932_, _#1934_, _#1935_, _#1939_)
  - Typed trees carry their axes and constant keys, and the evaluator reads operands, layouts, and extern results by their checked type. (_#1931_, _#1938_)
  - Diagnostic payloads, runtime failures, binding errors, and domain violations are typed and rendered only at the boundary. `EvaluationError::Failed` was deleted. (_#1931_, _#1933_, _#1934_, _#1938_)
  - Specializations resolve dimension targets once through `CompleteSubstitution`, and one closed-Nat evaluator replaces three. (_#1937_)
  - The refactor-metrics script lists every counted site, and small runtime checks were replaced by types. Internal-error construction sites went from 276 to 161. (_#1929_, _#1939_)
- Made reserved `#[lazy]` unrepresentable in the attribute model and centralized the `#[hidden]` arity check. (_#1964_)
- Keyed LSP extern signature help by a typed callee instead of a formatted string. (_#1963_)
- Removed a duplicated multi-declaration table axis parser. (_#1961_)
- Made the constructor rename regression test unconditional. (_#1962_)
- Bumped the workspace version to v0.0.1-alpha.33. (_#1979_)

**Full Changelog**: <https://github.com/graphcal-lang/graphcal/compare/v0.0.1-alpha.32...v0.0.1-alpha.33>
