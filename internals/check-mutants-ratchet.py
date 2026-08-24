#!/usr/bin/env -S uv run --script
"""Reject untracked missed or timed-out mutants in discovery reports."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from mutation_baseline import (
    FindingStatus,
    MutationId,
    display_mutation_id,
    load_baseline,
    load_finding_ids,
)


BASELINE = Path(".cargo/mutants-baseline.toml")
PLAN_SCHEMA_VERSION = 1


def _validate_plan(path: Path) -> None:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or value.get("schema_version") != PLAN_SCHEMA_VERSION:
        raise ValueError(f"unsupported mutation campaign plan: {path}")


def unexpected_findings(
    reports: list[Path],
    baseline_path: Path,
) -> set[MutationId]:
    baseline = load_baseline(baseline_path)
    tracked = {
        mutation_id
        for mutation_id, finding in baseline.items()
        if finding.status in {FindingStatus.OPEN, FindingStatus.EXCLUDED}
    }
    return load_finding_ids(reports) - tracked


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("reports", nargs="+", type=Path)
    parser.add_argument("--plan", required=True, type=Path)
    parser.add_argument("--baseline", default=BASELINE, type=Path)
    args = parser.parse_args()

    try:
        _validate_plan(args.plan)
        unexpected = unexpected_findings(args.reports, args.baseline)
    except (json.JSONDecodeError, OSError, ValueError) as error:
        print(f"Could not check mutation findings: {error}", file=sys.stderr)
        return 1
    if unexpected:
        print("New mutation findings:", file=sys.stderr)
        for mutation_id in sorted(unexpected):
            print(f"  {display_mutation_id(mutation_id)}", file=sys.stderr)
        return 1

    print("Mutation discovery produced no untracked findings.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
