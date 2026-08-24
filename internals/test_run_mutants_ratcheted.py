from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


SCRIPT = Path(__file__).with_name("run-mutants-ratcheted.py")
SPEC = importlib.util.spec_from_file_location("run_mutants_ratcheted", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class MutationRunnerTest(unittest.TestCase):
    def test_completed_campaign_checks_ratchet(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "outcomes.json"
            report.write_text('{"outcomes": []}\n', encoding="utf-8")
            completed = [
                subprocess.CompletedProcess([], 2),
                subprocess.CompletedProcess([], 1),
            ]

            with mock.patch.object(runner.subprocess, "run", side_effect=completed) as run:
                result = runner.main([], report)

            self.assertEqual(result, 1)
            self.assertEqual(run.call_count, 2)

    def test_interrupted_campaign_does_not_check_ratchet(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            completed = subprocess.CompletedProcess([], 130)

            with mock.patch.object(runner.subprocess, "run", return_value=completed) as run:
                result = runner.main(
                    [],
                    Path(directory) / "outcomes.json",
                )

            self.assertEqual(result, 130)
            run.assert_called_once_with(["cargo", "mutants"], check=False)


if __name__ == "__main__":
    unittest.main()
