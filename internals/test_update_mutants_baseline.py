from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest


INTERNALS = Path(__file__).parent
sys.path.insert(0, str(INTERNALS))
from mutation_baseline import (  # noqa: E402
    BaselineFinding,
    FindingStatus,
    MutationId,
    Resolution,
    ReviewStatus,
    SourcePosition,
    SourceSpan,
    mutation_id_to_json,
)

SCRIPT = INTERNALS / "update-mutants-baseline.py"
SPEC = importlib.util.spec_from_file_location("update_mutants_baseline", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
updater = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(updater)


def mutation_id(function: str, line: int = 1) -> MutationId:
    return MutationId(
        file="crate.rs",
        function=function,
        genre="FnValue",
        replacement="false",
        span=SourceSpan(
            SourcePosition(line, 5),
            SourcePosition(line + 1, 6),
        ),
    )


def outcome(identity: MutationId, summary: str) -> dict[str, object]:
    start_line = 10
    return {
        "summary": summary,
        "scenario": {
            "Mutant": {
                "name": f"crate.rs:{start_line + identity.span.start.line}:5: example",
                "file": identity.file,
                "function": {
                    "function_name": identity.function,
                    "span": {
                        "start": {"line": start_line, "column": 1},
                        "end": {"line": start_line + 10, "column": 2},
                    },
                },
                "genre": identity.genre,
                "replacement": identity.replacement,
                "span": {
                    "start": {
                        "line": start_line + identity.span.start.line,
                        "column": identity.span.start.column,
                    },
                    "end": {
                        "line": start_line + identity.span.end.line,
                        "column": identity.span.end.column,
                    },
                },
            }
        },
    }


def write_report(path: Path, outcomes: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"outcomes": outcomes}), encoding="utf-8")


class MutationBaselineUpdaterTest(unittest.TestCase):
    def test_campaign_resolves_obsoletes_and_reopens_findings(self) -> None:
        audited = mutation_id("audited")
        obsolete = mutation_id("obsolete")
        regressed = mutation_id("regressed")
        findings = {
            audited: BaselineFinding(
                audited,
                FindingStatus.OPEN,
                ReviewStatus.REVIEWED,
            ),
            obsolete: BaselineFinding(
                obsolete,
                FindingStatus.OPEN,
                ReviewStatus.REVIEWED,
            ),
            regressed: BaselineFinding(
                regressed,
                FindingStatus.RESOLVED,
                ReviewStatus.REVIEWED,
                resolution=Resolution.CAUGHT,
            ),
        }

        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "artifact"
            plan = artifact / "mutation-campaign/plan.json"
            plan.parent.mkdir(parents=True)
            plan.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "audit_complete": True,
                        "audited": [mutation_id_to_json(audited)],
                        "obsolete": [mutation_id_to_json(obsolete)],
                        "audit_report": "mutants-audit/mutants.out/outcomes.json",
                        "discovery_report": "mutants.out/outcomes.json",
                    }
                ),
                encoding="utf-8",
            )
            write_report(
                artifact / "mutants-audit/mutants.out/outcomes.json",
                [outcome(audited, "CaughtMutant")],
            )
            write_report(
                artifact / "mutants.out/outcomes.json",
                [outcome(regressed, "MissedMutant")],
            )

            findings = updater.apply_campaign_plan(plan, findings)

        self.assertEqual(findings[audited].status, FindingStatus.RESOLVED)
        self.assertEqual(findings[audited].resolution, Resolution.CAUGHT)
        self.assertEqual(findings[obsolete].status, FindingStatus.OBSOLETE)
        self.assertEqual(findings[regressed].status, FindingStatus.OPEN)
        self.assertEqual(findings[regressed].review, ReviewStatus.UNREVIEWED)

    def test_incomplete_audit_never_resolves_a_finding(self) -> None:
        audited = mutation_id("audited")
        finding = BaselineFinding(
            audited,
            FindingStatus.OPEN,
            ReviewStatus.REVIEWED,
        )
        findings = {audited: finding}

        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "artifact"
            plan = artifact / "mutation-campaign/plan.json"
            plan.parent.mkdir(parents=True)
            plan.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "audit_complete": False,
                        "audited": [mutation_id_to_json(audited)],
                        "obsolete": [],
                        "audit_report": "mutants-audit/mutants.out/outcomes.json",
                        "discovery_report": "mutants.out/outcomes.json",
                    }
                ),
                encoding="utf-8",
            )
            write_report(
                artifact / "mutants-audit/mutants.out/outcomes.json",
                [outcome(audited, "CaughtMutant")],
            )

            findings = updater.apply_campaign_plan(plan, findings)

        self.assertEqual(findings[audited], finding)


if __name__ == "__main__":
    unittest.main()
