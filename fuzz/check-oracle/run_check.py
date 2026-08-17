#!/usr/bin/env python3
"""Run `graphcal check` on every manifest entry; report mismatches.

Outputs results.jsonl next to the corpus and prints:
  SUSPECT  — expected-fail probe that PASSED check  (candidate compiler bug)
  BROKEN   — expected-pass control that FAILED check (broken scaffold or
             over-strict compiler; invalidates its probe twin's oracle)
"""

import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.join(HERE, "corpus")
BIN = os.environ.get(
    "GRAPHCAL_BIN",
    os.path.normpath(os.path.join(HERE, "..", "..", "target", "release",
                                  "graphcal")))


def check_one(entry):
    path = os.path.join(CORPUS, entry["file"])
    try:
        proc = subprocess.run(
            [BIN, "check", path], capture_output=True, text=True, timeout=60)
        actual = "pass" if proc.returncode == 0 else "fail"
        err_head = (proc.stderr or "").strip().split("\n")[:6]
    except subprocess.TimeoutExpired:
        actual = "timeout"
        err_head = ["<timeout after 60s>"]
    return dict(entry, actual=actual, err_head=err_head)


def main():
    entries = []
    with open(os.path.join(CORPUS, "manifest.jsonl")) as f:
        for line in f:
            entries.append(json.loads(line))
    with ThreadPoolExecutor(max_workers=int(os.environ.get("JOBS", "16"))) as ex:
        results = list(ex.map(check_one, entries))
    with open(os.path.join(HERE, "results.jsonl"), "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")

    suspects = [r for r in results
                if r["expected"] == "fail" and r["actual"] == "pass"]
    broken = [r for r in results
              if r["expected"] == "pass" and r["actual"] == "fail"]
    timeouts = [r for r in results if r["actual"] == "timeout"]

    n_probe = sum(1 for r in results if r["expected"] == "fail")
    n_ctrl = sum(1 for r in results if r["expected"] == "pass")
    print(f"checked {len(results)} files: {n_probe} probes, {n_ctrl} controls")
    print(f"probes correctly rejected : {n_probe - len(suspects)}")
    print(f"controls correctly passed : {n_ctrl - len(broken)}")
    print(f"SUSPECTS (probe passed)   : {len(suspects)}")
    print(f"BROKEN CONTROLS           : {len(broken)}")
    print(f"timeouts                  : {len(timeouts)}")
    if suspects:
        print("\n--- SUSPECTS ---")
        for r in suspects:
            print(f"  [{r['confidence']}] {r['family']:36s} {r['file']}")
            print(f"        {r['reason']}")
    if broken:
        print("\n--- BROKEN CONTROLS ---")
        for r in broken:
            print(f"  {r['family']:36s} {r['file']}")
            for ln in r["err_head"][:3]:
                print(f"        {ln}")


if __name__ == "__main__":
    main()
