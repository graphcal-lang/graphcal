#!/usr/bin/env python3
"""Generate the libFuzzer token dictionary from grammar.ebnf terminals."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GRAMMAR = ROOT / "grammar.ebnf"
OUTPUT = ROOT / "fuzz" / "graphcal.dict"


def strip_ebnf_comments(source: str) -> str:
    """Remove ISO EBNF comments before extracting grammar terminals."""
    output: list[str] = []
    index = 0
    in_comment = False
    while index < len(source):
        if not in_comment and source.startswith("(*", index):
            in_comment = True
            index += 2
        elif in_comment and source.startswith("*)", index):
            in_comment = False
            index += 2
        elif in_comment:
            index += 1
        else:
            output.append(source[index])
            index += 1
    if in_comment:
        raise ValueError("unterminated EBNF comment")
    return "".join(output)


def dictionary_lines(source: str) -> list[str]:
    grammar = strip_ebnf_comments(source)
    terminals = set(re.findall(r'(?<!\')"([^"\n]*)"(?!\')', grammar))
    return [
        f'"{terminal.replace(chr(92), chr(92) * 2).replace(chr(34), chr(92) + chr(34))}"'
        for terminal in sorted(terminals)
        if terminal
    ]


def main() -> None:
    lines = dictionary_lines(GRAMMAR.read_text())
    OUTPUT.write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
