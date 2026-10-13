import { describe, expect, test } from "vite-plus/test";
import { fileTree, visibleRows } from "./explorer.ts";

describe("fileTree", () => {
  test("lists directories before files at each level, in name order", () => {
    const rows = fileTree([
      "src/power_project/power_max.gcl",
      "graphcal.toml",
      "src/power_project/eps.gcl",
      "README.md",
      "src/lib/orbit.gcl",
    ]);
    expect(
      rows.map((r) => `${"  ".repeat(r.depth)}${r.kind === "dir" ? r.name + "/" : r.name}`),
    ).toEqual([
      "src/",
      "  lib/",
      "    orbit.gcl",
      "  power_project/",
      "    eps.gcl",
      "    power_max.gcl",
      "README.md",
      "graphcal.toml",
    ]);
    expect(rows.find((r) => r.name === "eps.gcl")?.path).toBe("src/power_project/eps.gcl");
    expect(rows.find((r) => r.name === "lib")?.path).toBe("src/lib");
  });

  test("is empty for no files", () => {
    expect(fileTree([])).toEqual([]);
  });

  test("visibleRows hides everything under a folded directory", () => {
    const rows = fileTree(["src/a/x.gcl", "src/b/y.gcl", "src/z.gcl", "README.md"]);
    const shown = visibleRows(rows, new Set(["src/a"])).map((r) => r.path);
    expect(shown).toEqual(["src", "src/a", "src/b", "src/b/y.gcl", "src/z.gcl", "README.md"]);
    expect(visibleRows(rows, new Set(["src"])).map((r) => r.path)).toEqual(["src", "README.md"]);
  });
});
