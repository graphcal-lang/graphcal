from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


INTERNALS = Path(__file__).parent
sys.path.insert(0, str(INTERNALS))
from mutation_baseline import (  # noqa: E402
    BaselineFinding,
    FindingStatus,
    ReviewStatus,
    candidate_from_json,
    write_baseline,
)

SCRIPT = INTERNALS / "run-mutants-ratcheted.py"
SPEC = importlib.util.spec_from_file_location("run_mutants_ratcheted", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def candidate_json(function: str = "example", line: int = 12) -> dict[str, object]:
    return {
        "name": f"crate.rs:{line}:9: replace && with || in {function}",
        "file": "crate.rs",
        "function": {
            "function_name": function,
            "span": {
                "start": {"line": 10, "column": 1},
                "end": {"line": 20, "column": 2},
            },
        },
        "genre": "BinaryOperator",
        "replacement": "||",
        "span": {
            "start": {"line": line, "column": 9},
            "end": {"line": line, "column": 11},
        },
    }


class MutationRunnerTest(unittest.TestCase):
    def run_campaign(self, audit_requested: bool) -> tuple[int, list[list[str]]]:
        candidate_value = candidate_json()
        discovery_candidate_value = candidate_json("new_example", 15)
        candidate = candidate_from_json(candidate_value)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            baseline = root / "baseline.toml"
            plan = root / "mutation-campaign/plan.json"
            discovery_report = root / "mutants.out/outcomes.json"
            audit_report = root / "mutants-audit/mutants.out/outcomes.json"
            write_baseline(
                baseline,
                [
                    BaselineFinding(
                        candidate.mutation_id,
                        FindingStatus.OPEN,
                        ReviewStatus.REVIEWED,
                    )
                ],
            )
            commands: list[list[str]] = []

            def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
                commands.append(command)
                if "--list-files" in command:
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        stdout=json.dumps([{"package": "example", "path": "crate.rs"}]),
                        stderr="",
                    )
                if "--list" in command:
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        stdout=json.dumps([candidate_value, discovery_candidate_value]),
                        stderr="",
                    )
                if command[0] == "./internals/check-mutants-ratchet.py":
                    return subprocess.CompletedProcess(command, 0)
                if "--output" in command:
                    audit_report.parent.mkdir(parents=True, exist_ok=True)
                    audit_report.write_text('{"outcomes": []}\n', encoding="utf-8")
                    return subprocess.CompletedProcess(command, 2)
                discovery_report.parent.mkdir(parents=True, exist_ok=True)
                discovery_report.write_text('{"outcomes": []}\n', encoding="utf-8")
                return subprocess.CompletedProcess(command, 0)

            with mock.patch.object(runner.subprocess, "run", side_effect=run):
                result = runner.main(
                    ["--in-place", "-p", "example"],
                    audit_requested=audit_requested,
                    baseline_path=baseline,
                    plan_path=plan,
                    discovery_report=discovery_report,
                    audit_report=audit_report,
                )
            plan_value = json.loads(plan.read_text(encoding="utf-8"))
            self.assertTrue(plan_value["audit_complete"])
            return result, commands

    def test_daily_campaign_excludes_open_findings_before_execution(self) -> None:
        result, commands = self.run_campaign(audit_requested=False)

        execution_commands = [
            command
            for command in commands
            if "--list" not in command
            and "--list-files" not in command
            and command[0] == "cargo"
        ]
        self.assertEqual(result, 0)
        self.assertEqual(len(execution_commands), 1)
        self.assertIn("--exclude-re", execution_commands[0])
        self.assertNotIn("--re", execution_commands[0])

    def test_weekly_campaign_audits_then_excludes_open_findings(self) -> None:
        result, commands = self.run_campaign(audit_requested=True)

        execution_commands = [
            command
            for command in commands
            if "--list" not in command
            and "--list-files" not in command
            and command[0] == "cargo"
        ]
        self.assertEqual(result, 0)
        self.assertEqual(len(execution_commands), 2)
        audit, discovery = execution_commands
        self.assertIn("--re", audit)
        self.assertIn("--output", audit)
        self.assertIn("--exclude-re", discovery)
        self.assertEqual(
            discovery[discovery.index("--baseline") + 1],
            "skip",
        )


if __name__ == "__main__":
    unittest.main()
