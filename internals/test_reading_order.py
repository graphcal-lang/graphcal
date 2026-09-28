from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path
from unittest import mock

_SPEC = importlib.util.spec_from_file_location(
    "reading_order", Path(__file__).with_name("reading-order.py")
)
assert _SPEC is not None and _SPEC.loader is not None
reading_order = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = reading_order
_SPEC.loader.exec_module(reading_order)

A = "crates/a/src/lib.rs"
B = "crates/a/build_support/b.rs"
C = "crates/a/build.rs"
D = "crates/a/src/d.rs"
GROUP = reading_order.CuratedGroup("Build shell", (B, C))


def guide(body: str) -> str:
    return (
        "intro `crates/x/src/outside.rs`\n\n"
        f"{reading_order.BEGIN_MARKER}{body}{reading_order.END_MARKER}\n\ntail\n"
    )


class RenderListTest(unittest.TestCase):
    def test_numbers_items_and_indents_group_continuations(self) -> None:
        order = [A] + [f"crates/a/src/m{i}.rs" for i in range(8)] + [B, D, C]
        with (
            mock.patch.object(reading_order, "CURATED_GROUPS", (GROUP,)),
            mock.patch.object(reading_order, "NOTES", {D: "why it exists"}),
        ):
            rendered = reading_order.render_list(order)
            presented = reading_order.rendered_order(order)
        lines = rendered.strip("\n").splitlines()
        self.assertEqual(lines[0], f"1. `{A}`")
        self.assertEqual(
            lines[9:],
            [
                "10. Build shell:",
                f"    `{B}` →",
                f"    `{C}`",
                f"11. `{D}` (why it exists)",
            ],
        )
        self.assertTrue(rendered.startswith("\n\n") and rendered.endswith("\n\n"))
        self.assertEqual(presented[-3:], [B, C, D])
        self.assertEqual(sorted(presented), sorted(order))


class GeneratedRegionTest(unittest.TestCase):
    def test_replace_touches_only_the_marked_region(self) -> None:
        updated = reading_order.replace_generated(guide("\n\nold\n\n"), "\n\nnew\n\n")
        self.assertEqual(updated, guide("\n\nnew\n\n"))

    def test_existing_positions_ignore_paths_outside_markers(self) -> None:
        body = f"\n\n1. `{D}`\n2. `{A}`\n3. `{D}`\n\n"
        self.assertEqual(reading_order.existing_positions(guide(body)), {D: 0, A: 1})

    def test_missing_or_duplicate_markers_are_rejected(self) -> None:
        with self.assertRaises(ValueError):
            reading_order.generated_region("no markers")
        with self.assertRaises(ValueError):
            reading_order.generated_region(guide("") + reading_order.END_MARKER)


if __name__ == "__main__":
    unittest.main()
