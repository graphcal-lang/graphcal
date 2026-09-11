# Standalone playground

The application in `web/playground/` is a static single-file UI for the existing
`graphcal-wasm` adapter. It does not change Graphcal grammar, semantics, LSP, or
editor-extension protocols. The shared adapter continues to support multi-file
projects; this UI constructs exactly one virtual source file.

Use `vp` with the pinned pnpm package manager (`vp install --frozen-lockfile` in
`web/playground/`). `just playground-check` and `just playground-test` are also
part of `just lint` and `just test`.

## Contracts and layering

- `document.ts`: validate a Unicode document and identifier-stem `.gcl` basename;
  UTF-8 source limit 256 KiB, matching the Rust browser boundary. Filename matters
  for self-imports. Validation never modifies source or silently renames files.
- `share-codec.ts`: boundary serialization, version 2 gzip/base64url JSON in the
  fragment, a 16 KiB URL limit (warning above 8 KiB), incremental 2 MiB JSON-envelope
  decode limit, five-second stream deadline. The envelope allows JSON escaping
  overhead; decoded source is separately validated. The payload contains a source
  document and applied `{name, expr}` bindings, never evaluated values or HTML.
  Existing version 1 source-only links still decode. `?view=report` selects the
  full-width report without changing document identity or reloading source.
- `bindings.ts`: at most 256 unique binding names (1 KiB each), with closed-literal
  expressions limited to 4 KiB UTF-8 each. Rust validates their syntax and units.
- `report.ts` and `report-frame.js`: an opaque-origin sandboxed iframe hosts the
  shared report controls and rendering runtime. The parent checks sending window,
  session identity, request IDs, and binding limits. Fixed local Vega assets,
  restrictive CSP, and rejecting plot loaders prevent source-selected resources.
  The host worker alone prepares/evaluates the model and enforces cancellation.
- `example-catalog.ts`: allowlisted metadata lookup, never user-selected URLs.
- `graphcal-language.ts`: stateless CodeMirror stream tokenizer following the lexical
  rules in `grammar.ebnf`. Reserved keywords, booleans, numbers, strings, comments,
  operators, and punctuation receive standard highlight tags; contextual words and
  all identifier roles remain neutral. Strings have no escapes and unfinished
  strings recover at the next physical line. This is coloring, not validation.
- `editor.ts` installs the tokenizer and Lezer's standard CSS-class highlighter
  on every state creation; `styles.css` owns the light/dark token palette. Neither
  highlighting nor source restoration requires loading or running the Wasm engine.
- Browser effects belong in editor, worker, and app shells, not document state.

The supported baseline is current Chrome, Firefox, and Safari with WebAssembly,
module workers, and gzip Compression Streams. Shared links restore source, not a
historical compiler. Untrusted shared source must await an explicit Run. Fragments
are not transmitted in HTTP requests but are not secret or encrypted.

The standalone app replaces embedded documentation playgrounds. Multi-file
lessons remain static/CLI-based; their backend regression tests remain. Download
is optional and is not required for release. Oversized source remains available
for manual copying. Context-aware/semantic highlighting and full LSP integration
are deferred; basic lexical highlighting is available without compilation.

## Development and publishing

- `just playground-serve`: build Wasm/stage assets, then serve the frontend with vp.
- `just playground-build`: frontend lint/types, pure and real-Wasm tests, production build.
- `just docs-build`: build the frontend, build Zensical, then assemble and verify
  the combined `site/` artifact. GitHub Pages CI uses this exact recipe.
- `just site-serve`: build and preview the whole site at localhost:4173.
- `just docs-serve`: documentation only; it no longer builds or embeds Wasm.
- `vp -C web/playground exec playwright install chromium firefox webkit`: install
  browser engines once (CI uses `--with-deps` for Linux system libraries).
- `just playground-browser-test`: rebuild the entire site and run Playwright
  against the production artifact in Chromium, Firefox, and WebKit. This also
  runs in the Pages workflow before artifact upload. Browser binaries are not
  required for the ordinary `just lint` / `just test` commands.

`examples/catalog.json` names canonical repository sources and expected outputs.
`stage-examples.mjs` copies them into ignored public assets. The native catalog
integration test and TypeScript real-Wasm tests evaluate every entry. The functions
example deliberately keeps `main.gcl` because its self-imports refer to `main`.
Existing tutorial assets remain static downloads and multi-file regression inputs.

`internals/site-assemble.mjs` assembles the standalone app after Zensical's clean
build. `site-verify.mjs` checks the CNAME, root redirect, both apps, source asset
identity, a 6.5 MiB raw Wasm budget (including the shared metered plugin interpreter), and a 600 KiB raw entry-JavaScript budget.
The HTML report renderer and SHA-256 provenance add about 70 KiB to the engine;
the host-rendering API avoids linking the standalone chart bundles into Wasm.
Vega bundles are vendored from `graphcal-report` and loaded only for figures.
`protocol.ts` validates the rendered Rust output fields; real-Wasm tests cover its
value, report-body, assertion, diagnostic, and error variants. The playground uses
`graphcal-report`'s shared `ValueBody` projection: one indexed axis remains a
key/value list, two axes render as a grid, and three or more axes render as ordered
grid slices. Plot loaders deny external resources and embed metadata cannot override
this policy. `output-budget.ts`
rejects projected results, including HTML, above 8 MiB inside the worker before
they reach the UI. Results and reports derive from the same evaluation. Pending
or rejected controls leave the last successful values and shared bindings intact;
source edits clear overrides. Shared links require Run before any evaluation.

## Offline report bundles

Reports use `prepareReportBundle` separately from text-only `prepareProject`.
The CLI embeds verified dependency snapshots, root metadata, and plugin bytes;
`project_bundle.rs` validates the bounded wire format and mounts each package in
its own filesystem. `DependencySources::Embedded` cannot fall back to native
cache reads. The ordinary loader revalidates the complete lock graph and exact
artifact closure, then the ordinary metered plugin host registers the binaries.
Opaque package identities remain typed map keys; numeric mount directories are
only virtual filesystem locations, never encoded identities.

`just report-plugin-browser-test` covers a real SDK plugin's scalar/array/record
calls. `just report-package-browser-test` covers deleted-checkout and cache replay,
direct/transitive dependencies, two versions of the same plugin path, byte-identical
report generation, malformed closure rejection, and Chromium/Firefox/WebKit
controls/reset. Its blocked-plugin recovery test shortens only the browser shell's
deadline to avoid depending on how fast each engine spends a fuel budget; actual
worker teardown/repreparation and metered plugin execution remain unchanged.
No grammar or editor protocol changes are needed; native LSP invalidation follows
captured dependency manifests, sources, and binaries.

## Release verification

Frontend checks include Unicode/CRLF and empty-source sharing, malformed and
oversized payloads, a fixed v1 decode fixture, incremental decompression limits,
worker deadlines/cancellation, UTF-16 diagnostic positions, and real-Wasm
transport variants. The real-Wasm suite compares the engine's reported compiler
version with Cargo metadata to catch a stale local build artifact. If that check
fails despite a successful build, clean the `graphcal-wasm` package for the
`wasm32-unknown-unknown` target and `wasm-release` profile, then rebuild. This is
separate from the embedded report engine, which CLI builds automatically generate
and verify in Cargo's `OUT_DIR`; there are no engine files or digests to commit.

Browser tests cover the full catalog/plots, source-only network behavior,
shared-link restoration in a fresh context without auto-execution, clipboard
denial, source replacement/history confirmation, racing loads, worker failure
and Stop/retry, unit-aware parameter edits, desktop/mobile layout, slash redirects,
keyboard navigation, and automated light/dark WCAG accessibility scans. These
checks do not replace a human screen-reader audit or testing on physical mobile
devices. Actual GitHub Pages deployment and its redirects still require a
post-merge smoke check; local tests use the assembled artifact and preview server.

No production grammar, Rust compiler dependency, or LSP/editor-extension protocol
changed. Context-aware/semantic highlighting, full LSP, and the optional Download
feature remain deferred.
