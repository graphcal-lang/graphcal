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
- `share-codec.ts`: boundary serialization, version 1 gzip/base64url JSON in the
  fragment, a 16 KiB URL limit (warning above 8 KiB), incremental 2 MiB JSON-envelope
  decode limit, five-second stream deadline. The envelope allows JSON escaping
  overhead; decoded source is separately validated. No evaluation data is shared.
- `example-catalog.ts`: allowlisted metadata lookup, never user-selected URLs.
- Browser effects belong in editor, worker, and app shells, not document state.

The supported baseline is current Chrome, Firefox, and Safari with WebAssembly,
module workers, and gzip Compression Streams. Shared links restore source, not a
historical compiler. Untrusted shared source must await an explicit Run. Fragments
are not transmitted in HTTP requests but are not secret or encrypted.

The standalone app replaces embedded documentation playgrounds. Multi-file
lessons remain static/CLI-based; their backend regression tests remain. Download
is optional and is not required for release. Oversized source remains available
for manual copying. Syntax highlighting and full LSP integration are deferred.

## Development and publishing

- `just playground-serve`: build Wasm/stage assets, then serve the frontend with vp.
- `just playground-build`: frontend lint/types, pure and real-Wasm tests, production build.
- `just docs-build`: build the frontend, build Zensical, then assemble and verify
  the combined `site/` artifact. GitHub Pages CI uses this exact recipe.
- `just site-serve`: build and preview the whole site at localhost:4173.
- `just docs-serve`: documentation only; it no longer builds or embeds Wasm.

`examples/catalog.json` names canonical repository sources and expected outputs.
`stage-examples.mjs` copies them into ignored public assets. The native catalog
integration test and TypeScript real-Wasm tests evaluate every entry. The functions
example deliberately keeps `main.gcl` because its self-imports refer to `main`.
Existing tutorial assets remain static downloads and multi-file regression inputs.

`internals/site-assemble.mjs` assembles the standalone app after Zensical's clean
build. `site-verify.mjs` checks the CNAME, root redirect, both apps, source asset
identity, a 5 MiB raw Wasm budget, and a 600 KiB raw entry-JavaScript budget.
Vega bundles are vendored from `graphcal-report` and loaded only for figures.
`protocol.ts` validates the rendered Rust output fields; real-Wasm tests cover its
value, assertion, diagnostic, and error variants. Plot loaders deny external
resources and embed metadata cannot override this policy.
