# check-oracle: negative-oracle fuzzing for `graphcal check`

> [!WARNING]
> This content was written by an AI agent and must be verified by a human
> developer. After human verification, this alert may be removed.

Unlike the libFuzzer targets in the parent directory (which hunt panics),
this harness hunts **false accepts**: `.gcl` files that `graphcal check`
accepts even though the language documentation says they must be
compile-time errors.

## Method

- `gen.py` emits high-entropy files from ~185 **probe families**. Each
  probe embeds exactly one documented-invalid construct (dimension
  mismatches, kind mismatches, unbound references, cycles, index/ADT/
  generic/datetime/complex misuse, misplaced conversions, malformed
  attributes, parse-level junk, stress shapes, …) inside randomized valid
  filler with shuffled declaration order. Most probes have a
  minimally-different **control** twin that must pass — a failing control
  means the scaffold, not the probed rule, is broken, which keeps the
  oracle honest.
- `mutate.py` applies guaranteed-breaking mutations (undefined-reference
  rename outside comments, mismatched-dimension append, unlexable junk
  line, verbatim declaration duplication) to every file in
  `tests/fixtures/valid/`.
- `run_check.py` runs `graphcal check` on every generated file and reports
  **SUSPECTS** (expected-fail probes that passed — candidate compiler bugs)
  and **BROKEN CONTROLS** (expected-pass twins that failed).

## Usage

```sh
cargo build --release                      # builds target/release/graphcal
cd fuzz/check-oracle
GEN_SEED=20260818 GEN_PER_FAMILY=8 GEN_CHAOS=150 python3 gen.py
GEN_SEED=20260818 python3 mutate.py
python3 run_check.py                       # GRAPHCAL_BIN=… to override
```

The corpus is regenerated deterministically from `GEN_SEED`, so generated
files are not checked in (`corpus/` and `results.jsonl` are ignored).

## Findings

Confirmed false accepts and doc divergences from the initial campaign are
written up in `findings/FINDINGS.md`, with minimal reproducers alongside it
in `findings/`.
