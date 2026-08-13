# Dynamic safety and concurrency checks

Graphcal's dynamic-safety layer targets the boundaries that ordinary tests and
coverage cannot characterize completely.

## Plugin ABI harness

`crates/graphcal-plugin/tests/abi_memory.rs` directly executes every SDK raw
memory operation with native allocations:

- allocator/free pairing and ABI alignment;
- host-written array views;
- array-result and struct-slot writes;
- shape and slot-count rejection before copying;
- null/misaligned pointer rejection before constructing references.

The same harness runs under a pinned nightly Miri with Tree Borrows and symbolic
alignment, and under AddressSanitizer with an instrumented standard library.
Keep deliberate invalid cases at checks that execute before dereference; never
invoke an unsafe function with an out-of-bounds or dangling pointer merely to
"test" undefined behavior.

Local equivalents on Linux x86_64 are:

```sh
cargo +nightly-2026-02-21 miri setup
MIRIFLAGS="-Zmiri-symbolic-alignment-check -Zmiri-tree-borrows" \
  cargo +nightly-2026-02-21 miri test \
  -p graphcal-plugin --test abi_memory

RUSTFLAGS="-Zsanitizer=address" RUSTDOCFLAGS="-Zsanitizer=address" \
  cargo +nightly-2026-02-21 test -Zbuild-std \
  --target x86_64-unknown-linux-gnu \
  -p graphcal-plugin --test abi_memory
```

## LSP scheduling model

`analysis_schedule_state.rs` is the pure state core for one document's open,
register, cancel, close, and finish transitions. `server.rs` retains URI lookup,
Tokio semaphores, and worker orchestration as the imperative shell.

Two Loom models use replacement `Arc`, `Mutex`, threads, and atomic cancellation
probes to exhaust:

- close racing with worker completion;
- cancellation racing with duplicate-generation replacement.

Run them separately in release mode:

```sh
RUSTFLAGS="--cfg loom --check-cfg=cfg(loom)" \
  cargo test -p graphcal-lsp --release \
  analysis_schedule_state::loom_tests -- --test-threads=1
```

Loom does not model the entire Tokio server. The ordinary scheduler and timeout
tests remain responsible for permit ownership, bounded concurrency, and async
cancellation behavior.

## Cancellation sweep

The `graphcal-compiler/test-cancellation` feature exposes a deterministic token
only for downstream tests. The evaluator sweep first measures the number of
successful checkpoints needed by a bounded project, then cancels at every
preceding checkpoint and requires the typed cancelled outcome. This catches a
stage that swallows cancellation or continues into evaluation.

## Platform and optimization coverage

Required CI runs the full workspace on native Windows and installs the Wasm
plugin target needed by end-to-end SDK tests. A scheduled workflow
also runs the workspace under the release profile; Graphcal deliberately keeps
overflow checks enabled in that profile.

When adding or changing first-party unsafe code, extend the focused ABI harness
and keep both Miri and ASan green. When changing scheduler synchronization,
extend the small state core and its Loom model rather than attempting to wrap
the complete async server.
