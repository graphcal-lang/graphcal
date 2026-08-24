#!/usr/bin/env -S uv run --script
"""Run cargo-mutants and apply Graphcal's tracked survivor ratchet on completion."""

from __future__ import annotations

import subprocess
from pathlib import Path
import sys


REPORT = Path("mutants.out/outcomes.json")


def main(
    arguments: list[str] | None = None,
    report: Path = REPORT,
) -> int:
    cargo_arguments = sys.argv[1:] if arguments is None else arguments
    command = ["cargo", "mutants", *cargo_arguments]
    completed = subprocess.run(command, check=False)
    if completed.returncode in {0, 2, 3}:
        if not report.is_file():
            print("cargo-mutants produced no outcomes report", file=sys.stderr)
            return 1
        return subprocess.run(
            ["./internals/check-mutants-ratchet.py", str(report)],
            check=False,
        ).returncode
    return completed.returncode


if __name__ == "__main__":
    raise SystemExit(main())
