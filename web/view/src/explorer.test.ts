import { describe, expect, test } from "vite-plus/test";
import { fileTree, refsFailureNote, visibleRows } from "./explorer.ts";

test("a missing ref list says why", () => {
  expect(
    refsFailureNote({ list: "pulls", kind: null, message: "the extension lists branches only" }),
  ).toBe("pull requests are not listed: the extension lists branches only");
  expect(refsFailureNote({ list: "tags", kind: "network", message: "GitHub did not answer" })).toBe(
    "tags are not listed: GitHub did not answer",
  );
});

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

  test("compacts a directory that holds one directory and nothing else", () => {
    const rows = fileTree([
      "power_project/src/power_project/eps.gcl",
      "power_project/graphcal.toml",
      "docs/a/b/c.md",
    ]);
    expect(rows.map((r) => `${r.depth}:${r.kind}:${r.name}=${r.path}`)).toEqual([
      "0:dir:docs/a/b=docs/a/b",
      "1:file:c.md=docs/a/b/c.md",
      "0:dir:power_project=power_project",
      "1:dir:src/power_project=power_project/src/power_project",
      "2:file:eps.gcl=power_project/src/power_project/eps.gcl",
      "1:file:graphcal.toml=power_project/graphcal.toml",
    ]);
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
