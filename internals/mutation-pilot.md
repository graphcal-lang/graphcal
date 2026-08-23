# Mutation pilot results

Pilot run: 2026-08-14 with `cargo-mutants 27.1.0`.

| Safety target | Generated | Caught | Missed | Timeout | Unviable |
|---|---:|---:|---:|---:|---:|
| Exact rational | 46 | 40 | 1 | 0 | 5 |
| Dimension algebra | 106 | 75 | 10 | 0 | 21 |
| Nat algebra | 49 | 32 | 4 | 0 | 13 |
| Package validation | 171 | 120 | 7 | 0 | 44 |
| Plugin manifest | 60 | 49 | 3 | 0 | 8 |
| Plugin Wasm section | 61 | 56 | 2 | 2 | 1 |
| Runtime presentation instance | 12 | 9 | 0 | 0 | 3 |
| Presentation display projection | 48 | 39 | 2 | 0 | 7 |
| **Total** | **553** | **420** | **29** | **2** | **102** |

The viable mutation score was 420/451 (93.13%). The pilot deliberately keeps
missed and timed-out mutants visible rather than treating the score as the sole
success criterion.

Two missing assertions were fixed:

- an exact-rational display-body deletion is now killed by direct integer and
  fraction rendering assertions;
- both arithmetic mutations of a plugin custom-section error offset are now
  killed by a malformed non-empty-section assertion.

The exact-rational campaign was rerun after its test was added: 40 caught, one
missed equivalent boundary mutant, and five unviable. The two plugin offset
mutants were rerun independently and both were caught. Runtime presentation had
no survivors. This provides the acceptance proof that deliberately weakened
operators/bodies are killed.

Every remaining pilot finding, including two nonterminating plugin-section
mutants, is normalized and explained in `.cargo/mutants-baseline.txt`. Scheduled
shards reject findings not in that baseline, then an aggregation job records the
new findings in a pull request so failed campaigns do not lose their results.
Automatically added findings remain an unreviewed work queue until a follow-up
change resolves or explains them. Non-semantic debug-renderer exclusions and
known runner-terminating section mutations are narrowly named in
`.cargo/mutants.toml`; the latter remain recorded in the baseline as availability
findings.
