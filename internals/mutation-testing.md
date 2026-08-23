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
`.cargo/mutants-baseline.txt`, which is checked against campaign output. The
local ratchet and each scheduled shard reject findings outside that baseline.
After all scheduled shards finish, a final workflow job aggregates their reports,
appends new findings to the work queue, and creates or updates the fixed
`automation/mutants-baseline` pull request. This keeps a failed shard from losing
its findings while leaving explanation and resolution to reviewable follow-up
work.

Automatically appended entries are explicitly unreviewed. A baseline entry is a
review queue, not a permanent exemption. Equivalent boundary mutants should be
documented inline when reviewed. The automation only adds entries; it never
removes resolved entries automatically.

The PR job uses the workflow `GITHUB_TOKEN` with narrowly scoped `contents: write`
and `pull-requests: write` permissions. A repository or organization
administrator must also enable **Settings → Actions → General → Allow GitHub
Actions to create and approve pull requests**. Without that setting, mutation
reports still run and upload, but GitHub rejects PR creation.

## Result classification

For every campaign:

1. Treat `MISSED` and `TIMEOUT` as findings.
2. Add a behavioral assertion or simplify/delete unconstrained code.
3. Record a truly equivalent mutant with a specific rationale; never add broad
   file exclusions to improve a score.
4. Leave compiler-rejected mutants as `unviable`; they are outside the viable
   denominator.
5. Exclude a known nonterminating mutant only when it prevents the CI runner
   from producing a report; keep the regex mutation-specific and retain the
   finding in the baseline work queue.
6. Re-run the smallest affected file campaign, then the scheduled safety-core
   shard.

Mutation reports under `mutants.out/` are CI artifacts, not source inputs. The
ratcheted wrapper preserves infrastructure/baseline failures but handles the
expected cargo-mutants exit codes for recorded missed and timed-out mutants.
The scheduled aggregator requires one `outcomes.json` report and one completion
marker from every shard. The wrapper writes the marker only after cargo-mutants
returns a documented result code, so an interrupted campaign cannot silently
produce a partial baseline update from cargo-mutants' incrementally written
report. To exercise the aggregation behavior locally, pass all reports at once:

```sh
./internals/check-mutants-ratchet.py --update \
  path/to/shard-a/outcomes.json path/to/shard-b/outcomes.json
```
