# Git Workflow and Scenarios

> `.graph` files as source of truth; scenario overlays for parameter variations.

## Status

**Decision level:** Mostly settled. Core workflow is clear.

## Summary

`.graph` files are plain text, Git-tracked, diffable, and reviewable in PRs. Scenarios are small parameter overlay files that replace the `budget_v3_final_FINAL.xlsx` anti-pattern.

## Source of Truth

```gcl
// mission_budget.graph
param parking_alt = 200 km;
param isp = 320 s;
node transfer: TransferResult = { ... };
```

Git diff:

```diff
- param isp = 320 s;
+ param isp = 350 s;
```

## Scenarios (Parameter Overlays)

```yaml
# scenarios/high_isp.scenario
base: mission_budget.graph
overrides:
  isp: 450 s
  mass_initial: 4500 kg
```

The engine evaluates and compares multiple scenarios in parallel.

## Workflow

1. Engineer writes `.graph` files
2. Changes are committed and reviewed in PRs
3. CI runs type checking, evaluation, regression tests
4. Scenarios capture parameter variations without modifying source
5. Scenario comparison shows how outputs change across configurations

## Open Questions

- **Scenario format:** Is YAML the right format for `.scenario` files? Or should they be `.graph` files with a special syntax?
- **Scenario composition:** Can scenarios inherit from each other? E.g., `high_isp.scenario` extends `baseline.scenario` with one override?
- **Scenario diffing:** What does the CLI output look like when comparing two scenarios? A table? A diff?
- **CI integration:** What does a CI pipeline for Cellgraph look like? What checks can be automated?
- **Regression testing:** Can you assert that a node's value matches an expected result? E.g., `assert fuel_mass == 2847 kg +/- 1 kg`?
- **Locking / freezing:** Can you "lock" the output values (like a lock file) so that unexpected changes are detected?
- **Branch workflows:** How does Cellgraph handle merge conflicts in `.graph` files? Is the syntax designed to minimize conflicts?
- **Parameter input sources:** The TODO mentions parameters from files, CLI arguments, and environment variables. How do these interact with scenarios?
- **Audit trail:** Should Cellgraph track who changed which parameter and when (beyond Git blame)?
- **Scenario parameter validation:** Can scenarios reference parameters that don't exist in the base graph? Should that be an error?

## Dependencies on Other Aspects

- **Namespace** ([09](./09-namespace.md)): Multi-file projects are the reason for Git integration.
- **Computation Model** ([01](./01-computation-model.md)): Scenarios override `param` values.
- **Live View** ([13](./13-live-view.md)): Scenario comparison in the view.
- **Spreadsheet Compatibility** ([14](./14-spreadsheet-compatibility.md)): Export can generate versioned workbooks.
