#!/usr/bin/env python3
"""Guaranteed-breaking mutations of the repo's own valid fixtures.

Each mutation is chosen so that the mutant MUST fail `graphcal check`
regardless of file contents:
  ref_rename   : rewrite one `@name` reference to `@name_zzqx<i>` (undefined)
  append_const : append a const node with a Dimensionless/Mass mismatch
  junk_line    : insert an unlexable token line
  dup_decl     : duplicate one single-line param/node/const decl verbatim
"""

import json
import os
import random
import re
import sys

SEED = int(os.environ.get("GEN_SEED", "20260817"))
_HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(
    os.path.join(_HERE, "..", "..", "tests", "fixtures", "valid"))
OUT = os.path.join(_HERE, "corpus", "mutants")

AT_REF = re.compile(r"@([A-Za-z_][A-Za-z0-9_]*)")
SIMPLE_DECL = re.compile(
    r"^(?:pub\s+)?(?:param|node|const node)\s+[A-Za-z_][A-Za-z0-9_]*\s*:"
    r"[^;{]*=[^;{]*;\s*$")


def code_spans(text):
    """Byte ranges of text outside `//` line comments."""
    spans = []
    pos = 0
    for line in text.splitlines(keepends=True):
        cut = line.find("//")
        end = pos + (cut if cut != -1 else len(line))
        spans.append((pos, end))
        pos += len(line)
    return spans


def mutants_for(text, rng):
    out = []
    spans = code_spans(text)
    refs = [m for m in AT_REF.finditer(text)
            if any(s <= m.start() < e for s, e in spans)]
    if refs:
        m = rng.choice(refs)
        nm = m.group(1)
        mutated = text[:m.start()] + f"@{nm}_zzqx{rng.randint(0, 999)}" + text[m.end():]
        out.append(("ref_rename", mutated,
                    f"one @{nm} reference renamed to an undefined name"))
    out.append(("append_const",
                text + f"\nconst node zz_probe_{rng.randint(0, 9999)}: "
                       f"Dimensionless = 1.0 kg;\n",
                "appended const node with Dimensionless/Mass mismatch"))
    lines = text.split("\n")
    pos = rng.randrange(len(lines) + 1)
    junk = lines[:pos] + ["⟁⟁ unlexable ⟁⟁ ;;;"] + lines[pos:]
    out.append(("junk_line", "\n".join(junk),
                f"unlexable token line inserted at line {pos + 1}"))
    simple = [ln for ln in lines if SIMPLE_DECL.match(ln)]
    if simple:
        dup = rng.choice(simple).replace("pub ", "", 1)
        out.append(("dup_decl", text + "\n" + dup + "\n",
                    f"duplicated declaration: {dup[:60]!r}"))
    return out


def main():
    rng = random.Random(SEED + 1)
    os.makedirs(OUT, exist_ok=True)
    manifest = []
    sources = sorted(
        f for f in os.listdir(ROOT)
        if f.endswith(".gcl") and os.path.isfile(os.path.join(ROOT, f)))
    for src in sources:
        with open(os.path.join(ROOT, src), encoding="utf-8") as f:
            text = f.read()
        for kind, mutated, why in mutants_for(text, rng):
            stem = f"{os.path.splitext(src)[0]}__{kind}.gcl"
            with open(os.path.join(OUT, stem), "w", encoding="utf-8") as f:
                f.write(f"// MUTANT of {src}: {why}\n" + mutated)
            manifest.append(dict(
                file=os.path.join("mutants", stem), family=f"mutant_{kind}",
                expected="fail", reason=f"{src}: {why}", confidence="high"))
    mpath = os.path.join(os.path.dirname(OUT), "manifest.jsonl")
    with open(mpath, "a") as f:
        for m in manifest:
            f.write(json.dumps(m) + "\n")
    print(f"{len(manifest)} mutants from {len(sources)} valid fixtures")


if __name__ == "__main__":
    main()
