#!/usr/bin/env -S uv run --script
"""Update mutation finding lifecycle states from campaign artifacts."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from mutation_baseline import (
    BaselineFinding,
    FINDING_SUMMARIES,
    FindingStatus,
    MutationId,
    Resolution,
    ReviewStatus,
    load_baseline,
    load_outcomes,
    mutation_id_from_json,
    write_baseline,
)


BASELINE = Path(".cargo/mutants-baseline.toml")
PLAN_SCHEMA_VERSION = 1


def _required_relative_path(value: object, field: str) -> Path:
    if not isinstance(value, str):
        raise ValueError(f"{field} must be a string")
    path = Path(value)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"{field} must be a relative path")
    return path


def _identity_list(value: object, field: str) -> list[MutationId]:
    if not isinstance(value, list):
        raise ValueError(f"{field} must be an array")
    return [mutation_id_from_json(item) for item in value]


def _new_finding(mutation_id: MutationId) -> BaselineFinding:
    return BaselineFinding(
        mutation_id=mutation_id,
        status=FindingStatus.OPEN,
        review=ReviewStatus.UNREVIEWED,
    )


def _open_finding(
    findings: dict[MutationId, BaselineFinding],
    mutation_id: MutationId,
) -> None:
    existing = findings.get(mutation_id)
    match existing:
        case None:
            findings[mutation_id] = _new_finding(mutation_id)
        case BaselineFinding(status=FindingStatus.EXCLUDED):
            print(
                f"warning: explicitly excluded mutation appeared in discovery: {mutation_id}",
                file=sys.stderr,
            )
        case BaselineFinding(status=FindingStatus.OPEN):
            pass
        case BaselineFinding():
            findings[mutation_id] = existing.reopen()


def apply_campaign_plan(
    plan_path: Path,
    findings: dict[MutationId, BaselineFinding],
) -> dict[MutationId, BaselineFinding]:
    value = json.loads(plan_path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or value.get("schema_version") != PLAN_SCHEMA_VERSION:
        raise ValueError("unsupported campaign plan schema")
    artifact_root = plan_path.parent.parent
    updated = findings.copy()

    for mutation_id in _identity_list(value.get("obsolete"), "plan.obsolete"):
        existing = updated.get(mutation_id)
        if existing is not None and existing.status is not FindingStatus.OBSOLETE:
            updated[mutation_id] = existing.mark_obsolete()

    audited = _identity_list(value.get("audited"), "plan.audited")
    audit_complete = value.get("audit_complete")
    if not isinstance(audit_complete, bool):
        raise ValueError("plan.audit_complete must be a boolean")
    if audit_complete and audited:
        audit_report = artifact_root / _required_relative_path(
            value.get("audit_report"), "plan.audit_report"
        )
        try:
            outcomes = {
                outcome.mutation_id: outcome.summary
                for outcome in load_outcomes(audit_report)
            }
        except (json.JSONDecodeError, OSError, ValueError) as error:
            print(
                f"::warning file={audit_report}::Skipping baseline audit report: {error}",
                file=sys.stderr,
            )
        else:
            for mutation_id in audited:
                existing = updated.get(mutation_id)
                if existing is None:
                    raise ValueError(
                        f"audited mutation is absent from baseline: {mutation_id}"
                    )
                match outcomes.get(mutation_id):
                    case "MissedMutant" | "Timeout":
                        _open_finding(updated, mutation_id)
                    case "CaughtMutant":
                        updated[mutation_id] = existing.resolve(Resolution.CAUGHT)
                    case "Unviable":
                        updated[mutation_id] = existing.resolve(Resolution.UNVIABLE)
                    case None:
                        print(
                            f"warning: completed audit has no outcome for {mutation_id}",
                            file=sys.stderr,
                        )
                    case summary:
                        print(
                            f"warning: audit returned unexpected summary {summary!r} for {mutation_id}",
                            file=sys.stderr,
                        )

    discovery_report = artifact_root / _required_relative_path(
        value.get("discovery_report"), "plan.discovery_report"
    )
    if discovery_report.is_file():
        try:
            discovery_outcomes = load_outcomes(discovery_report)
        except (json.JSONDecodeError, OSError, ValueError) as error:
            print(
                f"::warning file={discovery_report}::Skipping discovery report: {error}",
                file=sys.stderr,
            )
        else:
            for outcome in discovery_outcomes:
                if outcome.summary in FINDING_SUMMARIES:
                    _open_finding(updated, outcome.mutation_id)
    return updated


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("artifacts", type=Path)
    parser.add_argument("--baseline", default=BASELINE, type=Path)
    args = parser.parse_args()

    findings = load_baseline(args.baseline)
    original = findings.copy()
    plans = sorted(args.artifacts.glob("**/mutation-campaign/plan.json"))
    if not plans:
        print("No mutation campaign plans were available; the baseline is unchanged.")
        return 0

    processed = 0
    for plan in plans:
        try:
            findings = apply_campaign_plan(plan, findings)
        except (json.JSONDecodeError, OSError, ValueError) as error:
            print(
                f"::warning file={plan}::Skipping mutation campaign artifact: {error}",
                file=sys.stderr,
            )
        else:
            processed += 1
    write_baseline(args.baseline, findings.values())
    changed = sum(original.get(key) != value for key, value in findings.items())
    print(
        f"Processed {processed}/{len(plans)} mutation campaign plan(s); "
        f"updated {changed} finding record(s)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
