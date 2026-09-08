# LSP capability-baseline performance snapshot

Captured: 2026-09-08

This snapshot predates the planned workspace index and richer evaluation UI. It
is a comparison point, not a performance budget or a cross-machine benchmark.
The workload uses an in-process JSON-RPC `LspService`, one small four-declaration
virtual document, and the release profile.

## Reproduce

Warm the release build, then run the ignored protocol benchmark:

```sh
cargo test --release -p graphcal-lsp --lib protocol_latency_baseline --no-run
/usr/bin/time -l cargo test --release -p graphcal-lsp --lib \
  protocol_latency_baseline -- --ignored --nocapture
```

`/usr/bin/time -l` is the macOS spelling. On Linux, use `/usr/bin/time -v` and
compare its maximum-resident-set field. Keep compilation out of the timed run.
The test reports initialization, open-to-diagnostics latency, and p50/p95 for
100 sequential hover and completion requests. It intentionally has no pass/fail
latency thresholds because shared CI and developer machines are not stable
benchmark environments.

## Captured result

Environment:

- macOS 26.6.2, arm64
- Apple M3 Max, 128 GiB RAM
- rustc/cargo 1.98.0
- warm release build

| Measurement | Result |
| --- | ---: |
| Initialize | 428 µs |
| Open to diagnostics | 2,310 µs |
| Hover p50 / p95 | 2 / 4 µs |
| Completion p50 / p95 | 23 / 30 µs |
| Iterations per request kind | 100 |
| Maximum resident set (`time -l`) | 106,594,304 bytes (~101.7 MiB) |
| Peak memory footprint (`time -l`) | 81,723,992 bytes (~77.9 MiB) |

The RSS measurement includes the Cargo test runner and Rust test harness, so it
is useful only for detecting large regressions under the same warmed command.
For production process memory, add an out-of-process stdio benchmark before
making memory-budget claims.

## Existing resource envelope

These are enforced server limits rather than measured performance:

- analysis debounce: 300 ms
- analysis deadline: 10 s
- global concurrent analyses: 2
- formatting deadline: 10 s
- concurrent formatting workers: 1
- maximum source size accepted by editor formatting: 16 MiB

Record future workspace-index measurements separately for discovery time,
steady-state index memory, invalidation latency, and query latency. Do not fold
eager evaluation/plugin execution into a static-index benchmark.
