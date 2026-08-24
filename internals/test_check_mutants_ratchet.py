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
    write_baseline,
)

SCRIPT = INTERNALS / "check-mutants-ratchet.py"
SPEC = importlib.util.spec_from_file_location("check_mutants_ratchet", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
ratchet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ratchet)


def mutation_id(line: int) -> MutationId:
    return MutationId(
        file="crates/example/src/lib.rs",
        function="example",
        genre="BinaryOperator",
        replacement="||",
        span=SourceSpan(
            start=SourcePosition(line, 9),
            end=SourcePosition(line, 11),
        ),
    )


def outcome(summary: str, line: int) -> dict[str, object]:
    return {
        "summary": summary,
        "scenario": {
            "Mutant": {
                "name": f"crate.rs:{line + 10}:9: replace && with || in example",
                "file": "crates/example/src/lib.rs",
                "function": {
                    "function_name": "example",
                    "span": {
                        "start": {"line": 10, "column": 1},
                        "end": {"line": 20, "column": 2},
                    },
                },
                "genre": "BinaryOperator",
                "replacement": "||",
                "span": {
                    "start": {"line": line + 10, "column": 9},
                    "end": {"line": line + 10, "column": 11},
                },
            }
        },
    }


class MutationRatchetTest(unittest.TestCase):
    def test_distinct_locations_do_not_collapse(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "outcomes.json"
            report.write_text(
                json.dumps(
                    {
                        "outcomes": [
                            outcome("MissedMutant", 2),
                            outcome("MissedMutant", 3),
                        ]
                    }
                ),
                encoding="utf-8",
            )
            baseline = Path(directory) / "baseline.toml"
            write_baseline(
                baseline,
                [
                    BaselineFinding(
                        mutation_id=mutation_id(2),
                        status=FindingStatus.OPEN,
                        review=ReviewStatus.REVIEWED,
                    )
                ],
            )

            self.assertEqual(
                ratchet.unexpected_findings([report], baseline),
                {mutation_id(3)},
            )

    def test_resolved_finding_is_unexpected_when_it_regresses(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "outcomes.json"
            report.write_text(
                json.dumps({"outcomes": [outcome("MissedMutant", 2)]}),
                encoding="utf-8",
            )
            baseline = Path(directory) / "baseline.toml"
            write_baseline(
                baseline,
                [
                    BaselineFinding(
                        mutation_id=mutation_id(2),
                        status=FindingStatus.RESOLVED,
                        review=ReviewStatus.REVIEWED,
                        resolution=Resolution.CAUGHT,
                    )
                ],
            )

            self.assertEqual(
                ratchet.unexpected_findings([report], baseline),
                {mutation_id(2)},
            )


if __name__ == "__main__":
    unittest.main()
