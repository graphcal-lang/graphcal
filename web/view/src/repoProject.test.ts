import { describe, expect, test } from "vite-plus/test";
import type { TreeEntry } from "./github.ts";
import { planProject, ProjectPlanError } from "./repoProject.ts";

function tree(paths: Record<string, number>): TreeEntry[] {
  const entries: TreeEntry[] = [];
  for (const [path, size] of Object.entries(paths)) {
    entries.push({ path, type: "blob", size });
  }
  entries.push({ path: "src", type: "tree", size: null });
  return entries;
}

describe("planProject", () => {
  const repo = tree({
    "README.md": 10,
    "graphcal.toml": 30,
    "src/power_project/power_max.gcl": 100,
    "src/power_project/eps.gcl": 80,
    "src/power_project/notes.txt": 5,
    "examples/other/graphcal.toml": 20,
    "examples/other/src/other/main.gcl": 40,
    "scratch/alone.gcl": 12,
  });

  test("the nearest manifest above the file is the root; nested projects are left out", () => {
    const plan = planProject(repo, "src/power_project/power_max.gcl");
    expect(plan.root).toBe("");
    expect(plan.entry).toBe("src/power_project/power_max.gcl");
    expect(plan.files.map((f) => f.relative)).toEqual([
      "graphcal.toml",
      "scratch/alone.gcl",
      "src/power_project/eps.gcl",
      "src/power_project/power_max.gcl",
    ]);
  });

  test("a file in a nested project gets that project", () => {
    const plan = planProject(repo, "examples/other/src/other/main.gcl");
    expect(plan.root).toBe("examples/other");
    expect(plan.entry).toBe("src/other/main.gcl");
    expect(plan.files).toEqual([
      { path: "examples/other/graphcal.toml", relative: "graphcal.toml" },
      { path: "examples/other/src/other/main.gcl", relative: "src/other/main.gcl" },
    ]);
  });

  test("without a manifest the file's directory is the root", () => {
    const plan = planProject(
      tree({ "a/b/one.gcl": 1, "a/b/two.gcl": 1, "a/c.gcl": 1 }),
      "a/b/one.gcl",
    );
    expect(plan.root).toBe("a/b");
    expect(plan.entry).toBe("one.gcl");
    expect(plan.files.map((f) => f.relative)).toEqual(["one.gcl", "two.gcl"]);
  });

  test("a file at the repository root without a manifest", () => {
    const plan = planProject(
      tree({ "rocket.gcl": 1, "other.gcl": 1, "sub/x.gcl": 1 }),
      "rocket.gcl",
    );
    expect(plan.root).toBe("");
    expect(plan.files.map((f) => f.relative)).toEqual(["other.gcl", "rocket.gcl", "sub/x.gcl"]);
  });

  test("rejects a missing file, a non-.gcl file, and oversize sources", () => {
    expect(() => planProject(repo, "src/missing.gcl")).toThrow(ProjectPlanError);
    expect(() => planProject(repo, "README.md")).toThrow(/not a \.gcl file/);
    expect(() => planProject(tree({ "big.gcl": 300 * 1024 }), "big.gcl")).toThrow(/256 KiB/);
    const many: Record<string, number> = {};
    for (let i = 0; i < 5; i++) many[`f${i}.gcl`] = 250 * 1024;
    expect(() => planProject(tree(many), "f0.gcl")).toThrow(/1024 KiB/);
  });
});
