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

Other pilot files currently retain known survivors. They are recorded as
structured lifecycle records in `.cargo/mutants-baseline.toml`. Each identity
includes a function-relative source span, so separate mutations in the same
function cannot collapse into one work item.

A discovery campaign runs daily. Before cargo-mutants executes tests, the wrapper
lists the generated candidates and passes exact `--exclude-re` filters for
`open` and `excluded` baseline records. Candidate enumeration is cheap; skipping
known survivors avoids their comparatively expensive full-suite test runs.
Monday's campaign first audits every runnable `open` record with an exact `--re`
filter, then excludes those already-tested candidates from discovery. Manual
dispatches can request the same audit.

The final workflow job aggregates every available shard artifact and creates or
updates the mutation baseline pull request. New findings become `open` and
`unreviewed`; caught or unviable audit outcomes become `resolved`; identities no
longer generated become `obsolete`; and deliberately nonterminating mutations
remain `excluded`. Records are retained as history rather than deleted. A later
missed outcome reopens a resolved or obsolete record.

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

Mutation reports under `mutants.out/` and `mutants-audit/mutants.out/` are CI
artifacts, not source inputs. The ratcheted wrapper preserves infrastructure
failures but handles cargo-mutants' documented result codes. It writes a campaign
plan before testing and marks an audit complete only after cargo-mutants returns
a documented result and produces its report. Consequently, an interrupted audit
can never resolve a finding. Incrementally written discovery reports remain
useful: the aggregator records every finding available before interruption and
skips missing or malformed shard artifacts with a workflow warning.

To apply downloaded campaign artifacts locally, run:

```sh
./internals/update-mutants-baseline.py path/to/artifact-directory
```
