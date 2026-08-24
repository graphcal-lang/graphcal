#!/usr/bin/env -S uv run --script
"""Run a baseline-aware cargo-mutants discovery campaign."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import subprocess
import sys

from mutation_baseline import (
    FindingStatus,
    MutationCandidate,
    MutationId,
    load_baseline,
    load_candidates_json,
    mutation_id_to_json,
)


BASELINE = Path(".cargo/mutants-baseline.toml")
PLAN = Path("mutation-campaign/plan.json")
DISCOVERY_REPORT = Path("mutants.out/outcomes.json")
AUDIT_OUTPUT_PARENT = Path("mutants-audit")
AUDIT_REPORT = AUDIT_OUTPUT_PARENT / "mutants.out/outcomes.json"
ACCEPTED_CARGO_MUTANTS_CODES = frozenset({0, 2, 3})
PLAN_SCHEMA_VERSION = 1


def _insert_options(arguments: list[str], options: list[str]) -> list[str]:
    try:
        separator = arguments.index("--")
    except ValueError:
        return [*arguments, *options]
    return [*arguments[:separator], *options, *arguments[separator:]]


def _cargo_command(arguments: list[str], options: list[str]) -> list[str]:
    return ["cargo", "mutants", *_insert_options(arguments, options)]


def _run_listing(arguments: list[str], options: list[str]) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        _cargo_command(arguments, options),
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        sys.stdout.write(completed.stdout)
        sys.stderr.write(completed.stderr)
    return completed


def _selected_files(contents: str) -> set[str]:
    value = json.loads(contents)
    if not isinstance(value, list):
        raise ValueError("cargo-mutants file listing must be an array")
    files: set[str] = set()
    for entry_value in value:
        if not isinstance(entry_value, dict) or not isinstance(entry_value.get("path"), str):
            raise ValueError("cargo-mutants file listing contains an invalid entry")
        files.add(entry_value["path"])
    return files


def _candidate_map(candidates: list[MutationCandidate]) -> dict[MutationId, MutationCandidate]:
    result = {candidate.mutation_id: candidate for candidate in candidates}
    if len(result) != len(candidates):
        raise ValueError("cargo-mutants produced duplicate mutation identities")
    return result


def _exact_name_regex(candidates: list[MutationCandidate]) -> str:
    names = sorted({candidate.name for candidate in candidates})
    if not names:
        raise ValueError("cannot build an exact regex for no mutation candidates")
    return "^(" + "|".join(re.escape(name) for name in names) + ")$"


def _write_plan(
    *,
    path: Path,
    audit_requested: bool,
    audit_complete: bool,
    selected_files: set[str],
    obsolete: set[MutationId],
    audited: set[MutationId],
) -> None:
    value = {
        "schema_version": PLAN_SCHEMA_VERSION,
        "audit_requested": audit_requested,
        "audit_complete": audit_complete,
        "selected_files": sorted(selected_files),
        "obsolete": [mutation_id_to_json(item) for item in sorted(obsolete)],
        "audited": [mutation_id_to_json(item) for item in sorted(audited)],
        "discovery_report": str(DISCOVERY_REPORT),
        "audit_report": str(AUDIT_REPORT),
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def _audit_enabled_from_environment() -> bool:
    value = os.environ.get("GRAPHCAL_MUTATION_AUDIT", "false")
    match value:
        case "true":
            return True
        case "false":
            return False
        case _:
            raise ValueError("GRAPHCAL_MUTATION_AUDIT must be 'true' or 'false'")


def main(
    arguments: list[str] | None = None,
    *,
    audit_requested: bool | None = None,
    baseline_path: Path = BASELINE,
    plan_path: Path = PLAN,
    discovery_report: Path = DISCOVERY_REPORT,
    audit_report: Path = AUDIT_REPORT,
) -> int:
    cargo_arguments = sys.argv[1:] if arguments is None else arguments
    should_audit = (
        _audit_enabled_from_environment()
        if audit_requested is None
        else audit_requested
    )
    baseline = load_baseline(baseline_path)

    files_listing = _run_listing(
        cargo_arguments,
        ["--list-files", "--json", "--no-config"],
    )
    if files_listing.returncode != 0:
        return files_listing.returncode
    all_listing = _run_listing(
        cargo_arguments,
        ["--list", "--json", "--no-config"],
    )
    if all_listing.returncode != 0:
        return all_listing.returncode
    runnable_listing = _run_listing(cargo_arguments, ["--list", "--json"])
    if runnable_listing.returncode != 0:
        return runnable_listing.returncode

    try:
        selected_files = _selected_files(files_listing.stdout)
        all_candidates = _candidate_map(load_candidates_json(all_listing.stdout))
        runnable_candidates = _candidate_map(load_candidates_json(runnable_listing.stdout))
    except (json.JSONDecodeError, ValueError) as error:
        print(f"Invalid cargo-mutants listing: {error}", file=sys.stderr)
        return 1

    scoped_baseline = {
        mutation_id: finding
        for mutation_id, finding in baseline.items()
        if mutation_id.file in selected_files
    }
    obsolete = set(scoped_baseline) - set(all_candidates)
    open_candidates = [
        all_candidates[mutation_id]
        for mutation_id, finding in scoped_baseline.items()
        if mutation_id in all_candidates and finding.status is FindingStatus.OPEN
    ]
    excluded_candidates = [
        all_candidates[mutation_id]
        for mutation_id, finding in scoped_baseline.items()
        if mutation_id in all_candidates and finding.status is FindingStatus.EXCLUDED
    ]
    audit_candidates = [
        runnable_candidates[candidate.mutation_id]
        for candidate in open_candidates
        if candidate.mutation_id in runnable_candidates
    ] if should_audit else []
    audited = {candidate.mutation_id for candidate in audit_candidates}

    _write_plan(
        path=plan_path,
        audit_requested=should_audit,
        audit_complete=False,
        selected_files=selected_files,
        obsolete=obsolete,
        audited=audited,
    )

    audit_ran = False
    if audit_candidates:
        audit_command = _cargo_command(
            cargo_arguments,
            [
                "--output",
                str(AUDIT_OUTPUT_PARENT),
                "--re",
                _exact_name_regex(audit_candidates),
            ],
        )
        audit_completed = subprocess.run(audit_command, check=False)
        if audit_completed.returncode not in ACCEPTED_CARGO_MUTANTS_CODES:
            return audit_completed.returncode
        if not audit_report.is_file():
            print("cargo-mutants produced no baseline-audit report", file=sys.stderr)
            return 1
        audit_ran = True

    _write_plan(
        path=plan_path,
        audit_requested=should_audit,
        audit_complete=True,
        selected_files=selected_files,
        obsolete=obsolete,
        audited=audited,
    )

    skipped_candidates = [*open_candidates, *excluded_candidates]
    skipped_ids = {candidate.mutation_id for candidate in skipped_candidates}
    discovery_candidates = set(runnable_candidates) - skipped_ids
    if discovery_candidates:
        discovery_options: list[str] = []
        if skipped_candidates:
            discovery_options.extend(
                ["--exclude-re", _exact_name_regex(skipped_candidates)]
            )
        if audit_ran:
            discovery_options.extend(["--baseline", "skip"])
        discovery_completed = subprocess.run(
            _cargo_command(cargo_arguments, discovery_options),
            check=False,
        )
        if discovery_completed.returncode not in ACCEPTED_CARGO_MUTANTS_CODES:
            return discovery_completed.returncode
        if not discovery_report.is_file():
            print("cargo-mutants produced no discovery report", file=sys.stderr)
            return 1
    else:
        discovery_report.parent.mkdir(parents=True, exist_ok=True)
        discovery_report.write_text('{"outcomes": []}\n', encoding="utf-8")
        print("All selected mutants are already tracked; discovery has no work.")
    return subprocess.run(
        [
            "./internals/check-mutants-ratchet.py",
            "--plan",
            str(plan_path),
            str(discovery_report),
        ],
        check=False,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
