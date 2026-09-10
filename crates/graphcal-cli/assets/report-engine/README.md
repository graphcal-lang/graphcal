# Embedded report engine

Source-checkout CLI builds automatically compile `graphcal-wasm` with the pinned
repository Rust toolchain, generate no-modules JavaScript with the locked
`wasm-bindgen` CLI, and optimize the Wasm with Binaryen's `wasm-opt -Oz`.
See `docs/en/installation.md` for prerequisites. Build scripts never install tools.

The bundle and its integrity manifest live under Cargo's `OUT_DIR/report-engine`.
A private nested Cargo target directory preserves compilation artifacts across
source edits without contending with the native build's target lock. Cargo
serializes each `OUT_DIR`; different native profiles/targets have independent
caches. Native instrumentation, wrappers, and jobserver flags do not leak into
the engine. The nested build uses one worker from the build script's existing
job slot. This favors safe concurrency over maximum cold-build throughput.

Cache identity includes the local production dependency closure's sources,
assets, manifests and build scripts, workspace manifest/lockfile, build-support
implementation, Cargo configuration, and compiler/bundling tool versions.
Bundles have checksums for both JS and Wasm. Stale, incomplete or corrupt cached
bundles are regenerated; errors never fall back to an old engine. Generation
uses a temporary directory and publishes only after every step succeeds and
a second input check confirms the sources did not change during compilation.

`just wasm-report` exports the current embedded bundle into
`target/wasm-report/pkg` for Node tests. It does not rebuild an independent copy.

## Release packaging

`just wasm-report-package` stages the verified bundle **in this directory** for
`cargo package` / `cargo publish`. These generated files are ignored by Git but
explicitly included in the CLI's Cargo package. Do not commit them.

Published crates verify the packaged engine's Graphcal version and both output
checksums without requiring workspace sources, rustup, wasm-bindgen, wasm-opt,
or network access for engine generation. Missing or corrupt packaged assets
fail the build. Release CI must generate the bundle from the exact release tree
before packaging; consumers cannot independently recompute absent source inputs.
