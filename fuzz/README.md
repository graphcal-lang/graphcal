# Graphcal fuzz targets

This independent nightly workspace contains bounded libFuzzer targets for
untrusted syntax and serialization boundaries.

## Targets and oracles

| Target | Maximum input | Oracle |
|---|---:|---|
| `parser` | 64 KiB UTF-8 | Parsing is total and does not panic. |
| `formatter` | 64 KiB UTF-8 | Parse errors are accepted; successful output reparses and is idempotent; internal formatter failures crash the target. |
| `package_manifest` | 64 KiB UTF-8 | Manifest parsing is total. |
| `package_lockfile` | 128 KiB UTF-8, 128 packages | Accepted lockfiles serialize deterministically and reparse under the same limit. |
| `plugin_manifest` | protocol manifest limit | Accepted manifests encode/decode identically. |
| `plugin_section` | 512 KiB | Extracted manifests embed/extract identically from a minimal Wasm module. |
| `generated_project` | 4 KiB mapped to a typed model | Valid projects reach checking and evaluation without an internal error. |

## Local workflow

Install `cargo-fuzz` and use a nightly compiler:

```sh
cargo install cargo-fuzz --locked
cargo +nightly fuzz check --fuzz-dir fuzz
cargo +nightly fuzz run parser fuzz/corpus/parser --fuzz-dir fuzz -- \
  -dict=fuzz/graphcal.dict -max_len=65536 -timeout=10
```

The checked-in corpus is deliberately small and human-readable. Regenerate the
Graphcal token dictionary after grammar changes:

```sh
./internals/generate-fuzz-dictionary.py
git diff --exit-code fuzz/graphcal.dict
```

## Triage and regression promotion

1. Reproduce an artifact with `cargo +nightly fuzz run TARGET ARTIFACT --fuzz-dir fuzz`.
2. Minimize it with `cargo +nightly fuzz tmin TARGET ARTIFACT --fuzz-dir fuzz`.
3. Fix the root cause and promote the minimized input to a named ordinary unit,
   integration, or fixture regression test. Do not rely only on the mutable fuzz
   corpus.
4. Run that ordinary test, the target against the artifact, and a bounded fuzz
   campaign before deleting the artifact.
5. Commit only compact seeds that add distinct behavior; use `cargo fuzz cmin`
   before accepting corpus growth.

CI builds all targets and performs bounded smoke runs. Crash artifacts are
uploaded on failure. Longer continuous campaigns are introduced separately once
triage ownership and stable targets exist.
