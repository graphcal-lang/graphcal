#!/usr/bin/env -S uv run --script
"""Run cargo-mutants and apply Graphcal's tracked survivor ratchet."""

from __future__ import annotations

import subprocess
from pathlib import Path
import sys


REPORT = Path("mutants.out/outcomes.json")
COMPLETION_MARKER = Path("mutants.out/graphcal-campaign-complete")


def main(
    arguments: list[str] | None = None,
    report: Path = REPORT,
    completion_marker: Path = COMPLETION_MARKER,
) -> int:
    cargo_arguments = sys.argv[1:] if arguments is None else arguments
    command = ["cargo", "mutants", *cargo_arguments]
    completed = subprocess.run(command, check=False)
    if completed.returncode in {0, 2, 3}:
        if not report.is_file():
            print("cargo-mutants produced no outcomes report", file=sys.stderr)
            return 1
        completion_marker.write_text("completed\n", encoding="utf-8")
        return subprocess.run(
            ["./internals/check-mutants-ratchet.py", str(report)],
            check=False,
        ).returncode
    return completed.returncode


if __name__ == "__main__":
    raise SystemExit(main())
