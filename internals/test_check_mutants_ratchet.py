from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check-mutants-ratchet.py")
SPEC = importlib.util.spec_from_file_location("check_mutants_ratchet", SCRIPT)
assert SPEC is not None
assert SPEC.loader is not None
ratchet = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ratchet)


def outcome(summary: str, replacement: str) -> dict[str, object]:
    return {
        "summary": summary,
        "scenario": {
            "Mutant": {
                "file": "crates/example/src/lib.rs",
                "function": {"function_name": "example"},
                "genre": "FnValue",
                "replacement": replacement,
            }
        },
    }


class MutationRatchetTest(unittest.TestCase):
    def test_load_findings_includes_missed_and_timeout_only(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "outcomes.json"
            report.write_text(
                json.dumps(
                    {
                        "outcomes": [
                            outcome("CaughtMutant", "false"),
                            outcome("MissedMutant", "true"),
                            outcome("Timeout", "loop {}"),
                        ]
                    }
                ),
                encoding="utf-8",
            )

            self.assertEqual(
                ratchet.load_findings([report]),
                {
                    "crates/example/src/lib.rs\texample\tFnValue\ttrue",
                    "crates/example/src/lib.rs\texample\tFnValue\tloop {}",
                },
            )

    def test_append_findings_adds_sorted_automated_section_once(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / "baseline.txt"
            existing = "crate.rs\texisting\tFnValue\tfalse"
            first = "crate.rs\ta_first\tFnValue\ttrue"
            second = "crate.rs\tz_second\tFnValue\ttrue"
            baseline.write_text(f"# Reviewed findings.\n{existing}\n", encoding="utf-8")

            added = ratchet.append_findings(baseline, {second, existing, first})
            added_again = ratchet.append_findings(baseline, {second, existing, first})

            self.assertEqual(added, {first, second})
            self.assertEqual(added_again, set())
            self.assertEqual(
                baseline.read_text(encoding="utf-8"),
                "# Reviewed findings.\n"
                f"{existing}\n\n"
                f"{ratchet.AUTOMATED_SECTION_HEADER}\n"
                f"{first}\n"
                f"{second}\n",
            )


if __name__ == "__main__":
    unittest.main()
