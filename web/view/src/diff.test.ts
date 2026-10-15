import { describe, expect, test } from "vite-plus/test";
import { patchOf, unifiedDiff } from "./diff.ts";

describe("unifiedDiff", () => {
  test("writes one hunk with three lines of context, as git diff does", () => {
    const before = ["a", "b", "c", "d", "e", "f", "g", "h"].join("\n") + "\n";
    const after = ["a", "b", "c", "d", "E", "f", "g", "h"].join("\n") + "\n";
    expect(unifiedDiff({ path: "src/x.gcl", before, after })).toBe(
      [
        "diff --git a/src/x.gcl b/src/x.gcl",
        "--- a/src/x.gcl",
        "+++ b/src/x.gcl",
        "@@ -2,7 +2,7 @@",
        " b",
        " c",
        " d",
        "-e",
        "+E",
        " f",
        " g",
        " h",
        "",
      ].join("\n"),
    );
  });

  test("splits distant changes into hunks and merges near ones", () => {
    const lines = Array.from({ length: 30 }, (_, i) => `l${i + 1}`);
    const edited = [...lines];
    edited[1] = "L2";
    edited[5] = "L6";
    edited[25] = "L26";
    const diff = unifiedDiff({
      path: "f",
      before: lines.join("\n") + "\n",
      after: edited.join("\n") + "\n",
    });
    const heads = diff.split("\n").filter((l) => l.startsWith("@@"));
    expect(heads).toEqual(["@@ -1,9 +1,9 @@", "@@ -23,7 +23,7 @@"]);
  });

  test("handles added and removed lines and a missing final newline", () => {
    const diff = unifiedDiff({ path: "f", before: "a\nb\n", after: "a\nx\ny\nb" });
    expect(diff).toBe(
      [
        "diff --git a/f b/f",
        "--- a/f",
        "+++ b/f",
        "@@ -1,2 +1,4 @@",
        " a",
        "+x",
        "+y",
        " b",
        "\\ No newline at end of file",
        "",
      ].join("\n"),
    );
    expect(unifiedDiff({ path: "f", before: "a\n", after: "a\n" })).toBe("");
  });

  test("patchOf puts a heading above the changed files only", () => {
    const patch = patchOf(
      [
        { path: "a", before: "1\n", after: "1\n" },
        { path: "b", before: "1\n", after: "2\n" },
      ],
      "graphcal what-if: o/r @ main",
    );
    expect(patch.startsWith("# graphcal what-if: o/r @ main\n\ndiff --git a/b b/b\n")).toBe(true);
    expect(patch).not.toContain("a/a");
    expect(patchOf([{ path: "a", before: "1\n", after: "1\n" }], "x")).toBe("");
  });
});
