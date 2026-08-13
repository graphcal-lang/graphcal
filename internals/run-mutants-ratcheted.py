#!/usr/bin/env -S uv run --script
"""Run cargo-mutants and apply Graphcal's reviewed survivor ratchet."""

from __future__ import annotations

import subprocess
from pathlib import Path
import sys


def main() -> int:
    command = ["cargo", "mutants", *sys.argv[1:]]
    completed = subprocess.run(command, check=False)
    if completed.returncode in {0, 2, 3}:
        report = Path("mutants.out/outcomes.json")
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
