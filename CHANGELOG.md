# Changelog

Release notes for Graphcal. Each release section becomes the body of its GitHub Release.
Add a `## v<version>` section before running the release workflow.
Earlier releases are listed on [GitHub Releases](https://github.com/graphcal-lang/graphcal/releases).

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
