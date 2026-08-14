# Mutation testing

Graphcal uses `cargo-mutants` to measure whether tests detect seeded semantic
faults. The checked-in configuration runs downstream workspace tests because
compiler and evaluator contracts are often asserted by consumer crates.

## Safety-core pilot

The initial pilot covers:

- exact rational, dimension, and Nat algebra;
- package manifest/lockfile parsing and graph validation;
- plugin manifest and Wasm custom-section codecs;
- runtime presentation instance projection and display projection.

Run a target before broadening a campaign:

```sh
cargo mutants --list -p graphcal-compiler \
  -f crates/graphcal-compiler/src/exact_rational.rs
./internals/run-mutants-ratcheted.py --in-place --no-shuffle \
  -p graphcal-compiler \
  -f crates/graphcal-compiler/src/exact_rational.rs
```

The pilot found and closed missing assertions for exact-rational rendering and
plugin custom-section error offsets. `runtime_presentation.rs` has no unexplained
survivors: its 12 generated mutants yielded 9 caught and 3 unviable mutants.

Other pilot files currently retain known survivors. They are recorded in
`.cargo/mutants-baseline.txt`, which is checked against scheduled campaign output;
new survivors fail the ratchet. A known survivor is a review queue, not a
permanent exemption. Equivalent boundary mutants are documented inline in that
file.

## Result classification

For every campaign:

1. Treat `MISSED` and `TIMEOUT` as findings.
2. Add a behavioral assertion or simplify/delete unconstrained code.
3. Record a truly equivalent mutant with a specific rationale; never add broad
   file exclusions to improve a score.
4. Leave compiler-rejected mutants as `unviable`; they are outside the viable
   denominator.
5. Re-run the smallest affected file campaign, then the scheduled safety-core
   shard.

Mutation reports under `mutants.out/` are CI artifacts, not source inputs. The
ratcheted wrapper preserves infrastructure/baseline failures but handles the
expected cargo-mutants exit codes for reviewed missed and timed-out mutants.
