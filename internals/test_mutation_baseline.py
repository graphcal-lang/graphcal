from __future__ import annotations

from pathlib import Path
import tempfile
import unittest

from internals.mutation_baseline import (
    BaselineFinding,
    FindingStatus,
    MutationId,
    Resolution,
    ReviewStatus,
    SourcePosition,
    SourceSpan,
    load_baseline,
    load_candidates_json,
    render_baseline,
)


class MutationBaselineTest(unittest.TestCase):
    def test_candidate_identity_uses_function_relative_span(self) -> None:
        candidates = load_candidates_json(
            """[
              {
                "name": "crate.rs:42:9: replace && with || in example",
                "file": "crate.rs",
                "function": {
                  "function_name": "example",
                  "span": {
                    "start": {"line": 40, "column": 1},
                    "end": {"line": 50, "column": 2}
                  }
                },
                "genre": "BinaryOperator",
                "replacement": "||",
                "span": {
                  "start": {"line": 42, "column": 9},
                  "end": {"line": 42, "column": 11}
                }
              }
            ]"""
        )

        self.assertEqual(
            candidates[0].mutation_id.span,
            SourceSpan(SourcePosition(2, 9), SourcePosition(2, 11)),
        )

    def test_structured_baseline_round_trips_lifecycle_states(self) -> None:
        identity = MutationId(
            file="crate.rs",
            function="example",
            genre="FnValue",
            replacement="false",
            span=SourceSpan(SourcePosition(1, 5), SourcePosition(3, 6)),
        )
        finding = BaselineFinding(
            mutation_id=identity,
            status=FindingStatus.RESOLVED,
            review=ReviewStatus.REVIEWED,
            rationale="Covered by a direct assertion.",
            resolution=Resolution.CAUGHT,
        )

        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / "baseline.toml"
            baseline.write_text(render_baseline([finding]), encoding="utf-8")

            self.assertEqual(load_baseline(baseline), {identity: finding})

    def test_excluded_findings_require_a_rationale(self) -> None:
        identity = MutationId(
            file="crate.rs",
            function="example",
            genre="FnValue",
            replacement="false",
            span=SourceSpan(SourcePosition(1, 5), SourcePosition(3, 6)),
        )

        with self.assertRaisesRegex(ValueError, "require a rationale"):
            BaselineFinding(
                mutation_id=identity,
                status=FindingStatus.EXCLUDED,
                review=ReviewStatus.REVIEWED,
            )


if __name__ == "__main__":
    unittest.main()
