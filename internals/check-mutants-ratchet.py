#!/usr/bin/env -S uv run --script
"""Check or update missed and timed-out mutants against a checked-in baseline."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

FINDING_SUMMARIES = {"MissedMutant", "Timeout"}
AUTOMATED_SECTION_HEADER = """\
# Automatically discovered by scheduled mutation testing.
# These entries are an unreviewed work queue; resolve or explain them in follow-up PRs."""


def normalized_finding(outcome: dict[str, object]) -> str:
    scenario = outcome["scenario"]
    assert isinstance(scenario, dict)
    mutant = scenario["Mutant"]
    assert isinstance(mutant, dict)
    file = mutant["file"]
    function = mutant["function"]
    assert isinstance(file, str)
    assert isinstance(function, dict)
    function_name = function["function_name"]
    replacement = mutant.get("replacement", "")
    assert isinstance(function_name, str)
    assert isinstance(replacement, str)
    return f"{file}\t{function_name}\t{mutant['genre']}\t{replacement}".rstrip()


def load_baseline(path: Path) -> set[str]:
    return {
        raw_line.rstrip()
        for raw_line in path.read_text(encoding="utf-8").splitlines()
        if raw_line.strip() and not raw_line.lstrip().startswith("#")
    }


def load_findings(paths: list[Path]) -> set[str]:
    findings: set[str] = set()
    for path in paths:
        report = json.loads(path.read_text(encoding="utf-8"))
        findings.update(
            normalized_finding(outcome)
            for outcome in report["outcomes"]
            if outcome["summary"] in FINDING_SUMMARIES
        )
    return findings


def append_findings(path: Path, findings: set[str]) -> set[str]:
    new_findings = findings - load_baseline(path)
    if not new_findings:
        return set()

    contents = path.read_text(encoding="utf-8").rstrip()
    addition = "\n".join(sorted(new_findings))
    if AUTOMATED_SECTION_HEADER not in contents:
        addition = f"{AUTOMATED_SECTION_HEADER}\n{addition}"
    path.write_text(f"{contents}\n\n{addition}\n", encoding="utf-8")
    return new_findings


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("reports", nargs="+", type=Path)
    parser.add_argument(
        "--baseline",
        default=Path(".cargo/mutants-baseline.txt"),
        type=Path,
    )
    parser.add_argument(
        "--update",
        action="store_true",
        help="append new findings to the baseline instead of rejecting them",
    )
    args = parser.parse_args()

    baseline = load_baseline(args.baseline)
    findings = load_findings(args.reports)
    unexpected = findings - baseline
    if unexpected:
        if args.update:
            added = append_findings(args.baseline, findings)
            print(f"Added {len(added)} new mutation finding(s) to {args.baseline}.")
            return 0

        print("New mutation findings:", file=sys.stderr)
        for finding in sorted(unexpected):
            print(f"  {finding}", file=sys.stderr)
        return 1

    print(
        f"Mutation ratchet passed: {len(findings)} finding(s), "
        f"all in the {len(baseline)}-entry tracked baseline."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
