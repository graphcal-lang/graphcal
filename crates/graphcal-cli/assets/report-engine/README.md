# Embedded report engine

`graphcal_wasm.js` and `graphcal_wasm_bg.wasm` are generated from the
workspace's `graphcal-wasm` crate and embedded in the published CLI. Keeping
the engine in the CLI makes `graphcal report build` work outside the Graphcal
source checkout while ensuring that browser evaluation uses the same Graphcal
release as the native baseline.

Do not edit the generated files directly. Regenerate them with:

```sh
just wasm-report-update
```

Every CLI build from a Graphcal source checkout verifies the host-independent
source fingerprint in `source-tree.digest` and fails with regeneration guidance
when the engine is stale. `just wasm-report-check` exposes that automatic check
explicitly; `just report-smoke` additionally rebuilds the adapter and exercises
the checked-in engine end to end. The raw Wasm bytes are not compared because
otherwise equivalent wasm-pack output differs across build hosts. CI runs both
checks with wasm-pack 0.15.0.
