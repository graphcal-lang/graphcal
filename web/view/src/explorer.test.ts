import { describe, expect, test } from "vite-plus/test";
import { fileTree } from "./explorer.ts";

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
});
