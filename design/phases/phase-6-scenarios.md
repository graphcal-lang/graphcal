# Phase 6: Scenarios and CLI Workflow

> `.scenario` files for parameter overrides. `cellgraph check` for
> single-command validation. Regression testing via assertions.

## Goal

Prove the Git-based engineering workflow: scenarios replace
`budget_v3_final_FINAL.xlsx`. Engineers commit `.graph` files,
create scenario overlays, run regression tests in CI.

This phase completes the MVP.

## Prerequisites

Phases 0-5 must be complete. The full single-file and multi-file
language works with scalars, dimensions, structs, functions, and tables.

## Design Decisions to Lock

### Scenario Format

- [ ] **File format:** YAML, TOML, or a custom DSL?
      YAML is familiar; TOML is simpler; a custom DSL could reuse
      the `.graph` parser. Recommendation: YAML for simplicity and
      familiarity (engineers already know it).
- [ ] **File extension:** `.scenario`, `.yaml`, `.toml`?
- [ ] **Scenario structure:**
      ```yaml
      base: mission_budget.graph     # or project root
      overrides:
        isp: 450 s
        mass_initial: 4500 kg
      ```
      What is `base`? A file path? A project root? Can it be omitted
      (default to project root)?
- [ ] **Scenario inheritance:** Can scenario B extend scenario A?
      ```yaml
      extends: baseline.scenario
      overrides:
        isp: 450 s
      ```
      Or is this deferred?
- [ ] **Override syntax:** Values use the same literal syntax as `.graph` files
      (e.g., `450 s` with unit). How is this parsed within YAML?
      May need quoting: `isp: "450 s"`.
- [ ] **Override validation:** Overriding a non-existent param is an error.
      Overriding a `node` or `const` is an error (only `param` can be overridden).
- [ ] **Scenario discovery:** Are scenarios in a `scenarios/` directory by convention?
      Or can they be anywhere?

### Assertions (Regression Testing)

- [ ] **Assertion syntax:**
      ```yaml
      assertions:
        fuel_mass: 2847.3 kg +/- 0.1 kg
        transfer.total_dv: 3935 m/s +/- 1 m/s
      ```
      Tolerance syntax: `+/- value`, `+/- percentage%`?
- [ ] **Assertion on table values:** Can assertions reference table cells?
      E.g., `maneuvers[0].fuel: 1523.7 kg +/- 1 kg`?
- [ ] **Assertion failure output:** What does a failed assertion look like?
      Expected vs actual, with tolerance shown.

### CLI Commands

- [ ] **`cellgraph eval <path>`:** Evaluate a single file or project. Print results.
      Already exists from Phase 0; now works with projects.
- [ ] **`cellgraph check <path>`:** Parse + type-check + evaluate + assert.
      Single command, single exit code. 0 = all good.
      This is the CI command.
- [ ] **`cellgraph eval --scenario <file>`:** Evaluate with parameter overrides.
- [ ] **`cellgraph diff <scenario1> <scenario2>`:** Compare outputs of two scenarios
      side by side. Or defer this?
- [ ] **Output format:** Plain text (default), JSON (`--format json`),
      CSV (`--format csv`)?

### Parameter Input Sources

- [ ] **CLI overrides:** `cellgraph eval --set isp="450 s" mission/`
      Override individual params from the command line.
- [ ] **Environment variables:** `CELLGRAPH_PARAM_ISP="450 s" cellgraph eval mission/`?
      Or is this too implicit?
- [ ] **Priority order:** CLI args > scenario file > `.graph` default values.

## Syntax Supported in Phase 6

No new `.graph` syntax. Phase 6 is about tooling and workflow files.

New file formats:

```yaml
# .scenario file (YAML)
base: <path>                    # optional, defaults to project root
extends: <scenario-path>        # optional, single inheritance
overrides:
  <param_name>: <value>         # value uses .graph literal syntax
assertions:
  <node_name>: <value> +/- <tolerance>
```

New CLI commands:

```
cellgraph eval <path>                      # evaluate, print results
cellgraph eval <path> --scenario <file>    # evaluate with overrides
cellgraph eval <path> --set <param>=<val>  # evaluate with CLI override
cellgraph check <path>                     # parse + check + eval + assert
cellgraph check <path> --scenario <file>   # check with scenario
cellgraph fmt <path>                       # format .graph files (if formatter exists)
```

## Implementation Scope

| Component | Description |
| --- | --- |
| **Scenario parser** | Parse `.scenario` YAML files |
| **Override applicator** | Apply param overrides to the graph before evaluation |
| **Override validator** | Check that overridden names exist and are `param`s |
| **Assertion checker** | Compare evaluated values against expected values with tolerance |
| **`cellgraph check`** | Unified command: parse + type-check + evaluate + assert |
| **`--set` flag** | Parse CLI param overrides |
| **Output formatter** | JSON and CSV output options |
| **Exit codes** | 0 = success, 1 = assertion failure, 2 = compile error |

## Out of Scope

- Scenario comparison / diff view
- CI integration templates
- Lock files / output freezing
- Audit trail

## Milestone Test

```yaml
# scenarios/baseline.scenario
overrides:
  isp: "320 s"
  dry_mass: "1200 kg"
assertions:
  fuel_mass: "2847 kg +/- 1 kg"
  transfer.total_dv: "3935 m/s +/- 1 m/s"
```

```yaml
# scenarios/high_isp.scenario
overrides:
  isp: "450 s"
  dry_mass: "1200 kg"
```

```
$ cellgraph check mission/ --scenario scenarios/baseline.scenario
Parsing... OK
Type checking... OK
Evaluating... OK
Assertions:
  fuel_mass:          2847.3 kg  (expected 2847 kg +/- 1 kg)  PASS
  transfer.total_dv:  3935.2 m/s (expected 3935 m/s +/- 1 m/s) PASS

All checks passed.
```

```
$ cellgraph eval mission/ --scenario scenarios/high_isp.scenario
...
fuel_mass = 1623 kg
```

```
$ cellgraph eval mission/ --set isp="400 s"
...
fuel_mass = 2105 kg
```

### Error cases that must work

```yaml
# error: overriding a non-existent param
overrides:
  nonexistent_param: "100 kg"
# -> error: `nonexistent_param` is not a param in the project

# error: overriding a node (not a param)
overrides:
  fuel_mass: "100 kg"
# -> error: `fuel_mass` is a node, not a param. Only params can be overridden.

# error: assertion failure
assertions:
  fuel_mass: "9999 kg +/- 1 kg"
# -> FAIL: fuel_mass = 2847.3 kg (expected 9999 kg +/- 1 kg)
```

## Open Questions

- [ ] Should `cellgraph check` without `--scenario` still evaluate and print results?
      Or does it only check parsing + types (no evaluation)?
- [ ] Should scenario files support comments?
- [ ] Should there be a `cellgraph init` command to scaffold a new project?
- [ ] Naming: is the CLI command `cellgraph` or `kasuri`?
