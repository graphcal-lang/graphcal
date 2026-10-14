import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

  test("handles added and removed lines and a dropped final newline", () => {
    // `b` loses its newline, so it is changed, and the marker follows the new `b`.
    const diff = unifiedDiff({ path: "f", before: "a\nb\n", after: "a\nx\ny\nb" });
    expect(diff).toBe(
      [
        "diff --git a/f b/f",
        "--- a/f",
        "+++ b/f",
        "@@ -1,2 +1,4 @@",
        " a",
        "-b",
        "+x",
        "+y",
        "+b",
        "\\ No newline at end of file",
        "",
      ].join("\n"),
    );
    expect(unifiedDiff({ path: "f", before: "a\n", after: "a\n" })).toBe("");
  });

  test("marks the old and the new last line separately", () => {
    expect(unifiedDiff({ path: "f", before: "a", after: "b" })).toBe(
      [
        "diff --git a/f b/f",
        "--- a/f",
        "+++ b/f",
        "@@ -1,1 +1,1 @@",
        "-a",
        "\\ No newline at end of file",
        "+b",
        "\\ No newline at end of file",
        "",
      ].join("\n"),
    );
  });

  test("a change far from a last line without a newline leaves it unmarked", () => {
    const lines = Array.from({ length: 20 }, (_, i) => `param p${i}: Length = ${i}.0 m;`);
    const before = lines.join("\n");
    const diff = unifiedDiff({ path: "f", before, after: before.replace("1.0 m", "9.0 m") });
    expect(diff).not.toContain("No newline");
  });
});

/** What `git apply` makes of `before` with the patch: the bytes a user ends up with. */
function applied(before: string, after: string): string {
  const dir = mkdtempSync(join(tmpdir(), "graphcal-view-diff-"));
  try {
    writeFileSync(join(dir, "f.gcl"), before);
    writeFileSync(join(dir, "p.patch"), unifiedDiff({ path: "f.gcl", before, after }));
    execFileSync("git", ["apply", "p.patch"], { cwd: dir, stdio: "pipe" });
    return readFileSync(join(dir, "f.gcl"), "utf8");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe("git apply round trip", () => {
  const lines = Array.from({ length: 20 }, (_, i) => `param p${i}: Length = ${i}.0 m;`);
  const text = lines.join("\n");
  test.each([
    ["an edit far from a last line without a newline", text, text.replace("1.0 m", "9.0 m")],
    ["an edit that also drops the final newline", `${text}\n`, text.replace("1.0 m", "9.0 m")],
    ["a changed last line on both sides without one", text, text.replace("19.0 m", "9.0 m")],
    ["only the final newline added", "a\nb", "a\nb\n"],
    ["only the final newline dropped", "a\nb\n", "a\nb"],
    ["a line without a newline replaced by one with", "a", "b\n"],
    ["a line with a newline replaced by one without", "a\n", "b"],
    ["an empty file filled", "", "a\nb\n"],
    ["a file emptied", "a\nb\n", ""],
    ["CRLF lines", "a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n"],
    ["two hunks, the last at the end", `${text}\n`, `${text.replace("1.0 m", "9.0 m")}\nx`],
  ])("%s", (_name, before, after) => {
    expect(applied(before, after)).toBe(after);
  });
});

describe("patchOf", () => {
  test("puts a heading above the changed files only", () => {
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
